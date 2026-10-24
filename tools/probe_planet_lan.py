"""Verify immutable LAN candidate, optionally run the production two-proxy fixture."""
import argparse
import json
import os
import re
import shutil
import socket
import struct
import time
import secrets
from pathlib import Path
import subprocess
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
from planet_install import verify_package,digest,safe_path,InstallError,write_json
ROOT=Path(__file__).resolve().parents[1]
W=ROOT/'.superpowers/sdd/2026-10-22-planet-lan'

def entrypoints(package,manifest,game,fixture_root):
    """Exercise the actual release images at their closed managed CLI boundary."""
    root=package.parent/'entrypoint-probe'
    if root.exists(): raise InstallError('entrypoint probe directory already exists; use a fresh candidate')
    # Refuse occupied endpoints. Never stop an existing process to make room.
    reservations=[]
    try:
        for port in [47613,47615,7894]:
            sock=socket.socket();sock.bind(('127.0.0.1',port));reservations.append(sock)
    finally:
        for sock in reservations: sock.close()
    root.mkdir();profile=root/'profile';profile.mkdir()
    for source,name in [('client','beamng_earth'),('addon','beamng_planet')]:
        shutil.copytree(package/source,profile/'mods/unpacked'/name)
    udp=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);udp.bind(('127.0.0.1',0));port=udp.getsockname()[1];udp.close()
    cap=secrets.token_hex(16)
    cfg={'formatVersion':1,'requestSequence':1,'profileRoot':str(profile),'cacheRoot':str(fixture_root/'cache'),
         'gameRoot':str(game),'mode':'host','intent':{'operationId':secrets.token_hex(16),'generation':'1','state':'start',
         'endpoint':{'address':'127.0.0.1','port':port},'playerName':'release probe','ticket':None},'capability':cap,
         'statusPath':str(root/'bridge-status.json'),'bridgeAddress':'127.0.0.1:7894','earthProxyAddress':'127.0.0.1:47615',
         'backendAddress':'127.0.0.1:47613','maxPlayers':2,'maxVehiclesPerClient':1,'upnp':False,'publicListing':False,
         'map':'/levels/beamng_earth/info.json'}
    write_json(root/'launch.json',cfg)
    children=[];logs=[];result={'images':{},'localCapabilityAccepted':False,'wrongCapabilityRejected':False,'cleanup':False,
                                  'helperConfigAccepted':False,'wrongBackendPinRejected':False}
    def start(role,args):
        name={'backend':manifest['backendPath'],'bridge':manifest['runtime']['bridgePath'],'server':manifest['runtime']['serverPath']}[role]
        image=package/name;log=(root/(role+'.log')).open('wb');logs.append(log)
        child=subprocess.Popen([str(image),*map(str,args)],cwd=root,env={**os.environ,'BNGEARTH_PARENT':str(os.getpid())},stdout=log,stderr=log)
        children.append(child);result['images'][role]={'sha256':digest(image),'path':str(image),'pid':child.pid};return child
    def wait_for(predicate,label,seconds=30):
        end=time.monotonic()+seconds
        while time.monotonic()<end:
            if predicate(): return
            dead=[c for c in children if c.poll() is not None]
            if dead: raise InstallError(f'release process exited during {label}; see {root}')
            time.sleep(.05)
        raise InstallError(f'release entrypoint timeout: {label}; see {root}')
    def open_backend():
        try:
            with socket.create_connection(('127.0.0.1',47613),timeout=.2): return True
        except OSError: return False
    def receive(sock):
        def exact(n):
            b=b''
            while len(b)<n:
                part=sock.recv(n-len(b))
                if not part: raise InstallError('release local frame truncated')
                b+=part
            return b
        n=struct.unpack('>I',exact(4))[0]
        if not 0<n<=1048576: raise InstallError('release local frame limit')
        return json.loads(exact(n))
    def handshake(sock,capability):
        b=json.dumps({'version':1,'capability':capability},separators=(',',':')).encode();sock.sendall(struct.pack('>I',len(b))+b)
    try:
        helper_dir=root/'helper-preflight';helper_dir.mkdir()
        helper=helper_dir/'beamng-earth-companion.exe'
        shutil.copyfile(package/'beamng-earth-companion.exe',helper)
        helper_config={'gameRoot':str(game),'profile':str(profile),'cacheDir':str(fixture_root/'cache'),
                       'backendExe':str(package/manifest['backendPath']),
                       'backendSha256':manifest['files'][manifest['backendPath']]['sha256']}
        write_json(helper_dir/'config.json',helper_config)
        result['helperConfigAccepted']=subprocess.run([str(helper),'--check-config'],cwd=helper_dir,timeout=30).returncode==0
        wrong_config={**helper_config,'backendSha256':'0'*64}
        write_json(helper_dir/'config.json',wrong_config)
        result['wrongBackendPinRejected']=subprocess.run([str(helper),'--check-config'],cwd=helper_dir,timeout=30).returncode!=0
        if not result['helperConfigAccepted'] or not result['wrongBackendPinRejected']:
            raise InstallError('release helper backend pin preflight regression')
        start('backend',['--server','--port','47613','--user-dir',profile,'--cache-dir',fixture_root/'cache','--offline',
             '--imagery','off','--workers','1','--road-processes','0','--block-process','off','--block','512',
             '--sidewalks','off','--buildings','off','--airports','off','--sea','off','--trees','off','--road-widths','off'])
        wait_for(open_backend,'backend listen')
        start('server',['--planet-config',root/'launch.json'])
        wait_for(lambda:(root/'server-ready.json').is_file(),'private server ticket')
        start('bridge',['--planet-config',root/'launch.json'])
        def lobby():
            try: return json.loads((root/'bridge-status.json').read_text())['state']=='lobby'
            except (OSError,ValueError,KeyError): return False
        wait_for(lobby,'authenticated release bridge lobby')
        with socket.create_connection(('127.0.0.1',7894),timeout=3) as sock:
            handshake(sock,'0'*32);result['wrongCapabilityRejected']=sock.recv(1)==b''
        with socket.create_connection(('127.0.0.1',7894),timeout=3) as sock:
            handshake(sock,cap);welcome=receive(sock)
            result['localCapabilityAccepted']=welcome.get('kind')=='Control' and welcome.get('message',{}).get('command',{}).get('type')=='Welcome'
        if not result['wrongCapabilityRejected'] or not result['localCapabilityAccepted']:
            raise InstallError('release capability handshake regression')
        if (root/'mods').exists() or (root/'config.json').exists(): raise InstallError('legacy release startup produced config/mods')
        result['managedLobby']=True
    finally:
        for child in reversed(children):
            if child.poll() is None:
                subprocess.run(['taskkill.exe','/PID',str(child.pid),'/T','/F'],capture_output=True,check=False)
            try: child.wait(timeout=10)
            except subprocess.TimeoutExpired: child.kill();child.wait(timeout=10)
        for log in logs: log.close()
        result['cleanup']=all(c.poll() is not None for c in children)
        write_json(package.parent/'entrypoint-record.json',result)
    return result


def probe(package,game=None,fixture_root=None,run_integration=False,run_entrypoints=False):
    package=safe_path(package); manifest=verify_package(package)
    if manifest['formatVersion']!=3: raise InstallError('expected LAN candidate')
    record=json.loads((package/'build-record.json').read_text())
    paths={'backend':manifest['backendPath'],'helper':'beamng-earth-companion.exe',
           'bridge':manifest['runtime']['bridgePath'],'server':manifest['runtime']['serverPath']}
    for role,name in paths.items():
        if digest(package/name)!=record['images'][role]['sha256']: raise InstallError(f'candidate build-image mismatch: {role}')
    from build_planet_lan import sources
    if record['sources']!=sources(): raise InstallError('current sources differ from built candidate')
    result={'package':str(package),'manifestSha256':digest(package/'manifest.json'),'buildRecordSha256':digest(package/'build-record.json'),
            'softwareIntegration':'notRun','twoGamePhysicsAcceptance':False,'releaseAccepted':False}
    if run_entrypoints:
        if game is None or fixture_root is None: raise InstallError('entrypoint checks require explicit game and prepared fixture root')
        result['releaseEntrypoints']=entrypoints(package,manifest,safe_path(game),safe_path(fixture_root))
    if run_integration:
        if game is None or fixture_root is None: raise InstallError('integration requires explicit game and isolated prepared fixture root')
        env={**os.environ,'PLANET_TEST_GAME_ROOT':str(safe_path(game)),
             'PLANET_WORLD_SMOKE_ROOT':str(safe_path(fixture_root)),
             'PLANET_WORLD_BACKEND_EXE':str(package/manifest['backendPath']), 'PLANET_TASK9_LIFECYCLE':'1'}
        command=['pwsh','-NoProfile','-File',str(W/'run-rust.ps1'),'test','--offline','--locked',
                 '--manifest-path',str(ROOT/'vendor/planet-kissmp/Cargo.toml'),'-p','kissmp-server',
                 '--test','planet_world_stream','compiled_backend_two_production_proxies','--','--exact','--nocapture']
        completed=subprocess.run(command,cwd=ROOT,env=env,capture_output=True,text=True)
        used_tests=[]
        for match in re.finditer(r'Running[^\n]*\(([^\n]+\.exe)\)',completed.stderr):
            image=Path(match.group(1)); image=image if image.is_absolute() else ROOT/image
            if image.is_file(): used_tests.append({'path':str(image.resolve()),'sha256':digest(image)})
        result['actualIntegrationBinaries']={'backend':{'path':str(package/manifest['backendPath']),'sha256':digest(package/manifest['backendPath'])},'productionRustTestExecutables':used_tests}
        result['releaseBridgeServerCliExercisedByThisFixture']=False
        result.update(softwareIntegration='passed' if completed.returncode==0 else 'failed',
                      fixture='compiled_backend_two_production_proxies',command=command,returncode=completed.returncode,
                      log=completed.stdout+'\n'+completed.stderr)
        # Record stays outside the immutable package inventory.
        write_json(package.parent/'probe-record.json',result)
        if completed.returncode: raise InstallError('production two-proxy fixture failed; see probe-record.json')
    return result

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--package',required=True,type=Path)
    p.add_argument('--game',type=Path);p.add_argument('--fixture-root',type=Path);p.add_argument('--run-integration',action='store_true');p.add_argument('--run-entrypoints',action='store_true')
    try: print(json.dumps(probe(**vars(p.parse_args())),indent=2))
    except (OSError,ValueError,KeyError,RuntimeError) as e: p.exit(1,f'Planet LAN probe refused: {e}\n')
