"""Publish a fresh full Planet package after compile-only helper build."""
import argparse
import json
from pathlib import Path
import re
import shutil
import struct
import tempfile
import sys

# Embedded Python distributions may omit the script directory from sys.path.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_earth_client import build
from planet_install import InstallError, compatibility, digest, inventory, safe_path, write_json

ROOT = Path(__file__).resolve().parents[1]
BACKEND_PIN = 'f1187d7decaf37e3b7a62b637921c972bd311def3ee4229d509f67024ec2dabd'
DEFAULT_SOURCE = ROOT.parent / 'beamng-earth-audit/1.5.3-beta/source/BeamNG.Earth-011d6b54e42407b2f3a6deaddc47305cf2325a47'
DEFAULT_BACKEND = ROOT / 'artifacts/backend-planet6/dist/BeamNG.Earth.exe'


def package(output, backend, source, game):
    output, backend, source, game = map(safe_path, [output, backend, source, game])
    if output.exists(): raise InstallError('OutputDirectory already exists; choose a fresh directory')
    if not output.parent.is_dir(): raise InstallError('OutputDirectory parent must already exist')
    for immutable in [source, game, backend.parent, ROOT / 'addon']:
        if output.is_relative_to(immutable) or immutable.is_relative_to(output):
            raise InstallError('OutputDirectory overlaps immutable inputs')
    if digest(backend) != BACKEND_PIN:
        raise InstallError('Backend is not the approved BeamNG.Earth 1.5.4-planet6 executable')
    compat = json.loads((ROOT / 'compat/earth153/manifest.json').read_text())
    compatibility(game, compat['compatibility'])
    runtime_pins = json.loads((ROOT / 'addon/lua/ge/extensions/beamngPlanet.compatibility.json').read_text())
    if runtime_pins != {'gameFiles': {name: sha for name, sha in compat['compatibility']['gameFiles'].items() if not name.endswith('.exe')}}:
        raise InstallError('runtime source compatibility pins differ from package compatibility manifest')
    for name, sha in compat['addonAssets'].items():
        if digest(ROOT / name) != sha:
            raise InstallError(f'addon asset hash mismatch: {name}')
    protocol = (ROOT / 'companion/src/protocol.rs').read_text()
    producer = (ROOT / 'addon/lua/ge/extensions/beamngEarthAutostart.lua').read_text()
    if not re.search(r'FORMAT_VERSION: u32 = 2;', protocol) or 'formatVersion = 2' not in producer or 'status.formatVersion ~= 2' not in producer:
        raise InstallError('mixed helper/addon IPC versions')
    helper = ROOT / 'companion/target/release/beamng-earth-companion.exe'
    image = helper.read_bytes()
    pe = struct.unpack_from('<I', image, 0x3c)[0]
    if image[pe:pe+4] != b'PE\0\0' or struct.unpack_from('<H', image, pe+4)[0] != 0x8664 or struct.unpack_from('<H', image, pe+24+68)[0] != 2:
        raise InstallError('Release executable must be Windows x64 with GUI subsystem')
    with tempfile.TemporaryDirectory(prefix='.planet-package-', dir=output.parent) as temporary:
        stage = Path(temporary) / 'package'
        stage.mkdir()
        client = build(source, stage / 'client')
        addon = stage / 'addon'
        for name, facts in inventory(ROOT / 'addon').items():
            if name.startswith('lua/ge/extensions/beamngEarth/'):
                # Every shared Earth module is mounted once, inside the explicit client.
                if name not in client['files'] or digest(ROOT / 'addon' / name) != client['files'][name]['sha256']:
                    raise InstallError(f'shared Earth module missing/mismatched in client: {name}')
                continue
            target = addon / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / 'addon' / name, target)
            if digest(target) != facts['sha256']:
                raise InstallError(f'copied addon hash mismatch: {name}')
        for name, path in {'beamng-earth-companion.exe': helper, 'backend/BeamNG.Earth-1.5.4-planet6.exe': backend,
                           'README.md': ROOT / 'README.md', 'tools/install.ps1': ROOT / 'tools/install.ps1',
                           'tools/uninstall.ps1': ROOT / 'tools/uninstall.ps1', 'tools/planet_install.py': ROOT / 'tools/planet_install.py'}.items():
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            expected = BACKEND_PIN if name.startswith('backend/') else digest(path)
            shutil.copyfile(path, target)
            if digest(target) != expected:
                raise InstallError(f'copied package input hash mismatch: {name}')
        files = inventory(stage)
        record = {'formatVersion': 2, 'owner': 'beamng-planet', 'ipcVersion': 2, 'helperIpcVersion': 2,
                  'addonIpcVersion': 2, 'backendVersion': '1.5.4-planet6', 'backendPath': 'backend/BeamNG.Earth-1.5.4-planet6.exe',
                  'compatibility': compat['compatibility'], 'sourceCommit': compat['sourceCommit'],
                  'compatManifestSha256': digest(ROOT / 'compat/earth153/manifest.json'),
                  'originalClientFiles': compat['originalFiles'], 'files': files}
        write_json(stage / 'manifest.json', record)
        if output.exists(): raise InstallError('OutputDirectory appeared during packaging')
        stage.rename(output)
    return {'output': str(output), 'manifestSha256': digest(output / 'manifest.json'), 'files': len(files)}


def package_lan(output, source, game, build_record):
    """Stage complete reviewed source payload and binaries pinned by a fresh build."""
    from build_planet_lan import sources
    output, source, game, build_record = map(safe_path, [output, source, game, build_record])
    if output.exists() or not output.parent.is_dir():
        raise InstallError('OutputDirectory must be fresh with an existing parent')
    for immutable in [source, game, ROOT/'addon', ROOT/'vendor', ROOT/'companion']:
        if output.is_relative_to(immutable) or immutable.is_relative_to(output):
            raise InstallError('OutputDirectory overlaps immutable inputs')
    record = json.loads(build_record.read_text(encoding='utf-8-sig'))
    if record.get('sources') != sources():
        raise InstallError('build record does not match current sources; create a fresh candidate')
    if any(record.get(k) != v for k,v in {'formatVersion':1,'ipcVersion':3,'networkVersion':1,'worldFormatVersion':1}.items()):
        raise InstallError('build record contract mismatch')
    for role in ['backend','helper','bridge','server','updater']:
        entry=record['images'][role]; image=safe_path(entry['path'])
        if digest(image)!=entry['sha256'] or image.stat().st_size!=entry['size']:
            raise InstallError(f'build image mismatch: {role}')
    helper = Path(record['images']['helper']['path'])
    image=helper.read_bytes(); pe=struct.unpack_from('<I',image,0x3c)[0]
    if image[pe:pe+4]!=b'PE\0\0' or struct.unpack_from('<H',image,pe+4)[0]!=0x8664 or struct.unpack_from('<H',image,pe+24+68)[0]!=2:
        raise InstallError('Release helper must be Windows x64 GUI subsystem')
    compat=json.loads((ROOT/'compat/earth153/manifest.json').read_text())
    compatibility(game,compat['compatibility'])
    runtime=json.loads((ROOT/'compat/lan-runtime.json').read_text(encoding='utf-8-sig'))
    protocol=(ROOT/'companion/src/protocol.rs').read_text()
    producer=(ROOT/'addon/lua/ge/extensions/beamngEarthAutostart.lua').read_text()
    if not re.search(r'FORMAT_VERSION: u32 = 3;',protocol) or 'formatVersion = 3' not in producer or 'status.formatVersion ~= 3' not in producer:
        raise InstallError('mixed helper/addon IPC versions')
    pins=json.loads((ROOT/'addon/lua/ge/extensions/beamngPlanet.compatibility.json').read_text())
    if pins != {'gameFiles':{n:h for n,h in compat['compatibility']['gameFiles'].items() if not n.endswith('.exe')}}:
        raise InstallError('runtime compatibility pins differ from package')
    for name,sha in compat['addonAssets'].items():
        if digest(ROOT/name)!=sha: raise InstallError(f'addon asset hash mismatch: {name}')
    with tempfile.TemporaryDirectory(prefix='.planet-lan-package-',dir=output.parent) as temp:
        stage=Path(temp)/'package';stage.mkdir()
        client=build(source,stage/'client')
        def copy(src,name):
            target=stage/name
            if target.exists(): raise InstallError(f'duplicate packaged path: {name}')
            target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(src,target)
            if digest(src)!=digest(target): raise InstallError(f'copy changed: {name}')
        for name in inventory(ROOT/'addon'):
            if name.startswith('lua/ge/extensions/beamngEarth/'):
                if name not in client['files'] or digest(ROOT/'addon'/name)!=client['files'][name]['sha256']:
                    raise InstallError(f'shared Earth module missing/mismatched: {name}')
            else: copy(ROOT/'addon'/name,'addon/'+name)
        kiss=ROOT/'vendor/planet-kissmp'
        # Preserve every GE/VE module at its original VFS path, but never mount
        # the legacy modScript auto-start hook or default public server settings.
        for folder in ['lua','art']:
            for name in inventory(kiss/'KISSMultiplayer'/folder):
                mounted=folder+'/'+name
                if (stage/'client'/mounted).exists(): raise InstallError(f'duplicate VFS path: {mounted}')
                copy(kiss/'KISSMultiplayer'/mounted,'addon/'+mounted)
        for role,name in {'backend':'backend/BeamNG.Earth-planet-lan.exe','helper':'beamng-earth-companion.exe',
                          'bridge':'network/kissmp-bridge.exe','server':'network/kissmp-server.exe',
                          'updater':'planet-update.exe'}.items():
            copy(Path(record['images'][role]['path']),name)
        for src,name in [(kiss/'LICENSE','licenses/KissMP-LICENSE'),(kiss/'PLANET-MODIFICATIONS.md','licenses/KissMP-MODIFICATIONS.md'),
                         (ROOT/'compat/lan-source.json','licenses/KissMP-source.json'),
                         (ROOT/'vendor/earth154/LICENSE','licenses/BeamNG.Earth-LICENSE'),
                         (ROOT/'vendor/earth154/CREDITS.md','licenses/BeamNG.Earth-CREDITS.md'),
                         (ROOT/'compat/update-channel.json','update-channel.json'),
                         (ROOT/'compat/lan-runtime.json','lan-runtime.json')]: copy(src,name)
        # The release records immutable inputs without publishing build-machine paths.
        public_record=json.loads(json.dumps(record))
        for entry in public_record['images'].values(): entry['path']=Path(entry['path']).name
        write_json(stage/'build-record.json',public_record)
        copy(ROOT/'docs/PUBLIC-README.md','README.md')
        for name in ['tools/install.ps1','tools/uninstall.ps1','tools/planet_install.py','tools/planet_mod_import.py']:
            copy(ROOT/name,name)
        files=inventory(stage)
        manifest={'formatVersion':3,'owner':'beamng-planet','version':json.loads((ROOT/'compat/update-channel.json').read_text())['version'],'ipcVersion':3,'helperIpcVersion':3,'addonIpcVersion':3,
                  'networkVersion':1,'worldFormatVersion':1,'backendVersion':'planet-lan-candidate',
                  'backendPath':'backend/BeamNG.Earth-planet-lan.exe','runtime':runtime['runtime'],
                  'releaseAccepted':False,'compatibility':compat['compatibility'],'sourceCommit':compat['sourceCommit'],
                  'compatManifestSha256':digest(ROOT/'compat/earth153/manifest.json'),
                  'originalClientFiles':compat['originalFiles'],'files':files}
        from planet_install import validate_lan_runtime, verify_package
        validate_lan_runtime(manifest);write_json(stage/'manifest.json',manifest);verify_package(stage)
        if sources()!=record['sources']: raise InstallError('sources changed during packaging')
        if output.exists(): raise InstallError('OutputDirectory appeared during packaging')
        stage.rename(output)
    return {'output':str(output),'manifestSha256':digest(output/'manifest.json'),'files':len(files),'releaseAccepted':False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--backend', default=DEFAULT_BACKEND, type=Path)
    parser.add_argument('--source', default=DEFAULT_SOURCE, type=Path)
    parser.add_argument('--game', default=Path('C:/GameDev/Steam/steamapps/common/BeamNG.drive'), type=Path)
    args = parser.parse_args()
    try: result = package(**vars(args))
    except (RuntimeError, OSError, ValueError, KeyError, struct.error) as exc:
        parser.exit(1, f'Planet package refused: {exc}\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__': main()
