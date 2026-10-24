"""Build one immutable Planet LAN candidate; never installs or starts the game."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
from planet_install import digest, inventory, safe_path, write_json, InstallError
ROOT=Path(__file__).resolve().parents[1]
W=ROOT/'.superpowers/sdd/2026-10-22-planet-lan'
RUST_WRAPPER=Path(os.environ.get('PLANET_RUST_WRAPPER',str(W/'run-rust.ps1')))

def cargo_command(*args):
    return ['pwsh','-NoProfile','-File',RUST_WRAPPER,*args] if RUST_WRAPPER.is_file() else ['cargo',*args]


def sources():
    result={}
    for base in ['addon','compat','companion','vendor/earth154',
                 'vendor/planet-kissmp','tools']:
        for p in sorted((ROOT/base).rglob('*')):
            rel=p.relative_to(ROOT)
            if any(x in {'target','.git','__pycache__','.pytest_cache'} for x in rel.parts): continue
            if p.is_file(): result[rel.as_posix()]=digest(p)
    return result


def run(args,env=None):
    subprocess.run(list(map(str,args)),cwd=ROOT,env=env,check=True)


def build(output,source,game):
    output=safe_path(output)
    if output.exists() or not output.is_relative_to(ROOT/'artifacts'):
        raise InstallError('candidate must be a fresh directory inside artifacts')
    output.mkdir()
    before=sources();write_json(output/'sources-before.json',before)
    run([sys.executable,ROOT/'tools/build_planet_backend.py','--output',output/'backend-build'])
    backend=output/'backend-build/dist/BeamNG.Earth.exe'
    backend_pin=digest(backend)
    run([sys.executable,'-m','PyInstaller','--noconfirm','--clean','--onefile','--windowed',
         '--name','planet-update','--distpath',output/'updater-dist','--workpath',output/'updater-work',
         '--specpath',output,'--paths',ROOT/'tools',ROOT/'tools/planet_update.py'])
    updater=output/'updater-dist/planet-update.exe'
    helper_env={**os.environ,'PLANET_BACKEND_SHA256':backend_pin,'PLANET_UPDATER_SHA256':digest(updater)}
    run(cargo_command('build','--offline','--release','--locked',
         '--manifest-path',ROOT/'companion/Cargo.toml'),env=helper_env)
    run(cargo_command('build','--offline','--release','--locked',
         '--manifest-path',ROOT/'vendor/planet-kissmp/Cargo.toml','-p','kissmp-bridge','-p','kissmp-server'))
    after=sources();write_json(output/'sources-after.json',after)
    if before!=after: raise InstallError('sources changed during build; retain this failed candidate and create a fresh superseding candidate')
    images={'backend':backend,
            'helper':ROOT/'companion/target/release/beamng-earth-companion.exe',
            'bridge':ROOT/'vendor/planet-kissmp/target/release/kissmp-bridge.exe',
            'server':ROOT/'vendor/planet-kissmp/target/release/kissmp-server.exe', 'updater':updater}
    toolchains={'python':sys.version,'cargo':subprocess.check_output(list(map(str,cargo_command('--version'))),cwd=ROOT,text=True).strip(),
                'rustEnvironmentSha256':digest(RUST_WRAPPER) if RUST_WRAPPER.is_file() else None}
    record={'formatVersion':1,'toolchains':toolchains,'sources':before,'helperBackendPinSha256':backend_pin,'images':{k:{'path':str(p),'sha256':digest(p),'size':p.stat().st_size} for k,p in images.items()},
            'ipcVersion':3,'networkVersion':1,'worldFormatVersion':1,'releaseAccepted':False}
    write_json(output/'build-record.json',record)
    from package_planet import package_lan
    return package_lan(output/'package',source,game,output/'build-record.json')

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output',required=True,type=Path);p.add_argument('--source',required=True,type=Path);p.add_argument('--game',required=True,type=Path)
    try: print(json.dumps(build(**vars(p.parse_args())),indent=2))
    except (OSError,ValueError,RuntimeError,subprocess.CalledProcessError) as e: p.exit(1,f'Planet LAN candidate refused: {e}\n')
