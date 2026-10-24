"""Preview/copy enabled local vehicle mod ZIPs into a Planet profile; never overwrite.

Run without --apply first. Close BeamNG before applying. Both players need the
same enabled ZIPs and saved configurations; this tool never transfers over LAN.
"""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import uuid
import zipfile

RESOURCE_ROOTS={'vehicles'}

def safe_path(value):
    path=Path(value).absolute()
    for part in [path,*path.parents]:
        if part.is_symlink() or part.is_junction():
            raise RuntimeError(f'Linked mod/profile paths are unsupported: {part}')
    return path.resolve()

def digest(path):
    with path.open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()

def discover_source_profile():
    local=Path(os.environ.get('LOCALAPPDATA',Path.home()/'AppData/Local'))
    modern=local/'BeamNG/BeamNG.drive/current'
    if (modern/'mods').is_dir():return modern
    legacy=local/'BeamNG.drive'
    if legacy.is_dir():
        candidates=[p for p in legacy.iterdir() if all(n.isdigit() for n in p.name.split('.'))]
        for candidate in sorted(candidates,key=lambda p:tuple(map(int,p.name.split('.'))),reverse=True):
            if (candidate/'mods').is_dir():return candidate
    return None

def disabled_paths(profile):
    db=profile/'mods/db.json'
    if not db.is_file():return set()
    data=json.loads(db.read_text(encoding='utf-8-sig'))
    return {str(m.get('fullpath','')).strip('/').lower() for m in data.get('mods',{}).values() if m.get('active') is False}

def resource_zip(path):
    found=False
    with zipfile.ZipFile(path) as archive:
        for item in archive.infolist():
            name=item.filename;p=PurePosixPath(name)
            if p.is_absolute() or '..' in p.parts or '\\' in name or ':' in name or item.flag_bits&1:
                raise RuntimeError(f'unsafe mod archive entry: {name}')
            if not item.is_dir() and p.parts and p.parts[0] in RESOURCE_ROOTS:found=True
    return found

def plan_import(source_profile, profile, include_disabled=False):
    source=safe_path(source_profile);target=safe_path(profile)
    if source==target:raise RuntimeError('Source and Planet profile must differ')
    if not target.is_dir():raise RuntimeError('Planet profile does not exist')
    disabled=disabled_paths(source);plan=[]
    for path in sorted((source/'mods').rglob('*.zip')):
        safe_path(path)
        relative=path.relative_to(source/'mods')
        if 'unpacked' in relative.parts or (not include_disabled and ('mods/'+relative.as_posix()).lower() in disabled):continue
        if not resource_zip(path):continue
        destination=safe_path(target/'mods'/relative);sha=digest(path)
        if destination.exists():
            if not destination.is_file() or digest(destination)!=sha:
                raise RuntimeError(f'Existing mod has different contents; keep it or rename manually: {destination}')
            continue
        plan.append({'name':relative.as_posix(),'source':str(path),'sha256':sha,'size':path.stat().st_size})
    return plan

def preview_import(profile, source_profile=None, include_disabled=False):
    source=Path(source_profile) if source_profile is not None else discover_source_profile()
    plan=plan_import(source,profile,include_disabled) if source is not None else []
    return {'sourceProfile':str(source) if source is not None else None,'profile':str(safe_path(profile)),
            'planned':plan,'count':len(plan),'totalBytes':sum(item['size'] for item in plan),
            'includeDisabled':include_disabled,'applied':False}

def beamng_running():
    if os.name!='nt':raise RuntimeError('Automatic process check is supported on Windows only')
    output=subprocess.run(['tasklist','/FO','CSV','/NH'],check=True,capture_output=True,text=True).stdout
    return any(row and row[0].lower().startswith('beamng.drive') for row in csv.reader(output.splitlines()))

def apply_import(source_profile, profile, plan, game_running=beamng_running, include_disabled=False):
    if game_running():raise RuntimeError('Close BeamNG before importing vehicle mods')
    # Rebuild immediately before copying; callers cannot inject files or silently
    # accept a changed source/destination after reviewing the preview.
    if plan_import(source_profile,profile,include_disabled)!=plan:raise RuntimeError('Mod import preview changed; preview again')
    target=safe_path(profile);copied=[]
    if plan and shutil.disk_usage(target).free < sum(item['size'] for item in plan)+64*1024*1024:
        raise RuntimeError('Insufficient free space for the selected vehicle mods')
    for item in plan:
        destination=safe_path(target/'mods'/Path(item['name']))
        destination.parent.mkdir(parents=True,exist_ok=True)
        stage=destination.with_name(destination.name+'.planet-'+uuid.uuid4().hex+'.tmp')
        try:
            with Path(item['source']).open('rb') as source, stage.open('xb') as output:shutil.copyfileobj(source,output)
            if digest(stage)!=item['sha256']:raise RuntimeError('Mod changed during copy')
            if destination.exists():raise RuntimeError(f'Destination appeared during import: {destination}')
            # Windows rename is exclusive; no existing file can be replaced.
            if os.name=='nt':stage.rename(destination)
            else:
                with destination.open('xb') as output,stage.open('rb') as source:shutil.copyfileobj(source,output)
                stage.unlink()
            copied.append(item)
        finally:
            if stage.exists():stage.unlink()
    return {'copied':copied,'profile':str(target),'restartRequired':bool(copied)}

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-profile',type=Path);parser.add_argument('--profile',type=Path,required=True)
    parser.add_argument('--include-disabled',action='store_true',help='Explicitly import disabled source car mods; newly copied ZIPs activate only in the Planet profile')
    parser.add_argument('--apply',action='store_true');args=parser.parse_args()
    try:
        source=args.source_profile or discover_source_profile()
        if source is None:raise RuntimeError('No installed mod profile found; supply --source-profile')
        report=preview_import(args.profile,source,args.include_disabled)
        if args.apply:report=apply_import(source,args.profile,report['planned'],include_disabled=args.include_disabled)
        print(json.dumps(report,ensure_ascii=False,indent=2))
    except (OSError,ValueError,RuntimeError,zipfile.BadZipFile,subprocess.SubprocessError) as error:
        parser.exit(1,f'Mod import refused: {error}\n')
