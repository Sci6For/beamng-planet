"""Prepare a clean, reviewable GitHub release locally. Does not publish it."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import zipfile

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'tools'))
from planet_install import digest, verify_package

SKIP={'.git','target','build','dist','__pycache__','.pytest_cache','.superpowers',
      'node_modules','.venv','venv','.github','tests','tests_lua','testdata','fixtures',
      'cache','logs','artifacts','screenshots'}
EXCLUDE={'.exe','.dll','.pyd','.so','.pyc','.log','.sqlite','.db','.pem','.key','.pfx'}

def archive(folder, output):
    with zipfile.ZipFile(output,'w',zipfile.ZIP_DEFLATED,compresslevel=6) as z:
        for path in sorted(folder.rglob('*')):
            if path.is_file():z.write(path,path.relative_to(folder).as_posix())

def prepare(package,output):
    package,output=Path(package).resolve(),Path(output).resolve()
    if output.exists() or not output.is_relative_to(ROOT/'artifacts'):
        raise ValueError('Use a fresh release directory inside artifacts')
    record=verify_package(package);output.mkdir(parents=True)
    source=output/'source';source.mkdir()
    copied=[]
    for base in ['addon','companion','compat','tools','vendor']:
        for path in sorted((ROOT/base).rglob('*')):
            rel=path.relative_to(ROOT)
            if not path.is_file() or set(rel.parts)&SKIP or path.suffix.lower() in EXCLUDE or path.name.startswith('.env'):continue
            if path.is_symlink() or path.is_junction():raise ValueError('Source links are unsupported')
            target=source/rel;target.parent.mkdir(parents=True,exist_ok=True)
            shutil.copyfile(path,target);copied.append(rel.as_posix())
    shutil.copyfile(ROOT/'docs/PUBLIC-README.md',source/'README.md')
    (source/'docs').mkdir()
    shutil.copyfile(ROOT/'docs/PUBLIC-README.md',source/'docs/PUBLIC-README.md')
    (source/'.gitignore').write_text('target/\nartifacts/\nbuild/\ndist/\n__pycache__/\n*.pyc\n*.pyd\n*.exe\n.env*\n',encoding='utf8')
    archive(source,output/'Planet-Source.zip');archive(package,output/'Planet-Package.zip')
    (output/'payload.sha256').write_text(digest(package/'manifest.json')+'\n')
    subprocess.run([sys.executable,'-m','PyInstaller','--noconfirm','--clean','--onefile','--windowed',
        '--name','Planet-Setup','--distpath',str(output),'--workpath',str(output/'setup-build'),
        '--specpath',str(output),'--paths',str(ROOT/'tools'),
        '--add-data',str(package)+';payload','--add-data',str(ROOT/'tools/planet_setup_ui.ps1')+';.',
        '--add-data',str(output/'payload.sha256')+';.',str(ROOT/'tools/planet_setup.py')],check=True,cwd=ROOT)
    assets=['Planet-Setup.exe','Planet-Package.zip','Planet-Source.zip']
    checks=''.join(digest(output/name)+'  '+name+'\n' for name in assets)
    (output/'SHA256SUMS').write_text(checks,encoding='ascii')
    summary={'version':record['version'],'sourceFiles':len(copied)+2,'assets':{
        n:{'size':(output/n).stat().st_size,'sha256':digest(output/n)} for n in assets},
        'excluded':['profiles','map caches','session codes','private work notes','game installation','third-party vehicle ZIPs']}
    (output/'publication-review.json').write_text(json.dumps(summary,indent=2),encoding='utf8')
    print(json.dumps(summary,indent=2))
    return summary

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--package',required=True);p.add_argument('--output',required=True)
    prepare(**vars(p.parse_args()))
