"""Reversible filesystem-only Planet installation. Never edits Steam or starts processes."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import tempfile
import uuid
import zipfile

OWNER = 'beamng-planet'
IPC = 2


class InstallError(RuntimeError):
    pass


class RecoveryError(InstallError):
    """Both operation and recovery failed; the stage must survive this process."""


@contextmanager
def recovery_stage(prefix, parent):
    if os.name == 'nt':
        # Python 3.13 mkdtemp uses mode 0700 on Windows, replacing inherited
        # access with owner/admin/system-only permissions. Renamed payloads
        # keep that ACL and cannot be opened by the ordinary Steam user.
        # Exclusive mkdir retains the destination parent's normal ACL without
        # granting new rights or modifying existing directories.
        for _ in range(100):
            stage = Path(parent) / (prefix + uuid.uuid4().hex)
            try:
                stage.mkdir()
                break
            except FileExistsError:
                continue
        else:
            raise FileExistsError('could not allocate an unused recovery stage')
    else:
        stage = Path(tempfile.mkdtemp(prefix=prefix, dir=parent))
    try:
        yield stage
    except RecoveryError:
        # The stage has no finalizer: originals remain for explicit recovery even
        # after this process exits. Never use TemporaryDirectory here.
        raise
    except BaseException:
        shutil.rmtree(stage)
        raise
    else:
        shutil.rmtree(stage)


def rollback(moves, stage, operation_error):
    errors = []
    for source, target in reversed(moves):
        try:
            source.rename(target)
        except BaseException as exc:
            # One blocked tree must not prevent independent trees restoring.
            errors.append({'source': str(source), 'target': str(target), 'error': str(exc)})
    if errors:
        record = {'operationError': str(operation_error), 'recoveryPath': str(stage), 'rollbackErrors': errors}
        try:
            write_json(stage / 'recovery.json', record)
        except OSError as exc:
            errors.append({'error': f'cannot write recovery.json: {exc}'})
        details = '; '.join(f'{item.get("source", "report")} -> {item.get("target", "report")}: {item["error"]}' for item in errors)
        raise RecoveryError(f'Operation failed: {operation_error}. Rollback incomplete; recovery files retained at {stage}. Rollback errors: {details}') from operation_error


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def safe_path(value):
    path = Path(value)
    if not path.is_absolute():
        raise InstallError('paths must be absolute')
    for part in [path, *path.parents]:
        if part.is_symlink() or part.is_junction():
            raise InstallError(f'links/reparse paths are refused: {part}')
    return path.resolve()


def relative(name):
    path = PurePosixPath(name)
    if not name or path.is_absolute() or '..' in path.parts or '\\' in name or ':' in name:
        raise InstallError(f'unsafe manifest path: {name}')
    return Path(*path.parts)


def inventory(root):
    safe_path(root)
    files = {}
    if not root.exists():
        return files
    for path in sorted(root.rglob('*')):
        safe_path(path)
        if path.is_file():
            files[path.relative_to(root).as_posix()] = {'sha256': digest(path), 'size': path.stat().st_size}
    return files


def read_json(path):
    return json.loads(path.read_text(encoding='utf-8-sig'))


def directories(root):
    # File hashes alone would miss user-created empty directories.
    return sorted(p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_dir())


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')


def verify_package(package):
    manifest = read_json(package / 'manifest.json')
    if manifest.get('owner') != OWNER or manifest.get('formatVersion') not in (2, 3):
        raise InstallError('unknown package owner/version')
    expected_ipc = 3 if manifest['formatVersion'] == 3 else 2
    if any(manifest.get(key) != expected_ipc for key in ['ipcVersion', 'helperIpcVersion', 'addonIpcVersion']):
        raise InstallError('mixed IPC versions; helper and addon must agree with package format')
    if expected_ipc == 3:
        validate_lan_runtime(manifest)
    for name in manifest['files']:
        relative(name)
    actual = inventory(package)
    actual.pop('manifest.json', None)
    if actual != manifest['files']:
        raise InstallError('package file/hash mismatch')
    client = {x[7:].lower() for x in actual if x.startswith('client/')}
    addon = {x[6:].lower() for x in actual if x.startswith('addon/')}
    if client & addon:
        raise InstallError('duplicate mounted client/addon paths')
    if expected_ipc == 3:
        verify_lan_payload(package, manifest)
    return manifest


def validate_lan_runtime(manifest):
    if manifest.get('networkVersion') != 1 or manifest.get('worldFormatVersion') != 1:
        raise InstallError('unsupported network/world format')
    runtime = manifest.get('runtime')
    expected = {'bridgePath': 'network/kissmp-bridge.exe', 'serverPath': 'network/kissmp-server.exe',
                'publicListing': False, 'upnp': False, 'allowRemoteScripts': False, 'allowModDownloads': False}
    if runtime != expected:
        raise InstallError('unsafe or unknown LAN runtime configuration')
    for name in [runtime['bridgePath'], runtime['serverPath'], manifest['backendPath'],
                 'beamng-earth-companion.exe', 'licenses/KissMP-LICENSE', 'licenses/KissMP-MODIFICATIONS.md']:
        relative(name)
        if name not in manifest['files']:
            raise InstallError(f'missing LAN payload: {name}')


def verify_lan_payload(package, manifest):
    build = read_json(package / 'build-record.json')
    if any(build.get(k) != v for k,v in {'formatVersion':1,'ipcVersion':3,'networkVersion':1,'worldFormatVersion':1}.items()):
        raise InstallError('LAN build record contract mismatch')
    images = {'backend':manifest['backendPath'], 'helper':'beamng-earth-companion.exe',
              'bridge':manifest['runtime']['bridgePath'], 'server':manifest['runtime']['serverPath']}
    if 'planet-update.exe' in manifest['files']:
        images['updater']='planet-update.exe'
    for role,name in images.items():
        facts=manifest['files'][name]
        if any(build['images'][role].get(k)!=facts[k] for k in ['sha256','size']):
            raise InstallError(f'LAN build image mismatch: {role}')
    prefix='vendor/planet-kissmp/KISSMultiplayer/'
    for source,sha in build['sources'].items():
        if source.startswith((prefix+'lua/',prefix+'art/')):
            name='addon/'+source[len(prefix):]
            if name not in manifest['files'] or manifest['files'][name]['sha256']!=sha:
                raise InstallError(f'LAN module missing/mismatched: {name}')
    for name in manifest['files']:
        if name.startswith(('addon/scripts/kiss_mp/','addon/settings/kissmp')):
            raise InstallError('legacy KissMP automatic entrypoint/config refused')


def compatibility(game, compat):
    for name, sha in compat['gameFiles'].items():
        path = game / relative(name)
        safe_path(path)
        if not path.is_file() or digest(path) != sha:
            raise InstallError(f'game compatibility mismatch: {name}. Supported BeamNG {compat["gameVersion"]}; ordinary menu retained. No files changed.')


def owned(destination):
    manifest = read_json(destination / 'ownership.json')
    if manifest.get('owner') != OWNER or manifest.get('formatVersion') != 2 or safe_path(manifest['destination']) != destination:
        raise InstallError('unknown destination ownership')
    profile = safe_path(manifest['profile'])
    roots = {'helper': destination, 'client': profile / 'mods/unpacked/beamng_earth',
             'addon': profile / 'mods/unpacked/beamng_planet'}
    for kind, root in roots.items():
        actual = inventory(root)
        if kind == 'helper': actual.pop('ownership.json', None)
        if actual != manifest['trees'][kind] or directories(root) != manifest['directories'][kind]:
            raise InstallError(f'owned {kind} files changed; preserve/reconcile user files before update/removal')
    return manifest, roots


def check_collisions(profile, earth, addon, replacing, virtual_files):
    for name in virtual_files:
        loose = profile / relative(name)
        safe_path(loose)
        if loose.exists():
            raise InstallError(f'loose profile Earth/Planet collision: {loose}')
    mods = profile / 'mods'
    if not mods.exists(): return
    # VFS collisions can reside in any named mod, including ZIPs. Only the
    # verified canonical original client or our own current mods are exceptions.
    prefixes = ('lua/ge/extensions/beamngearth/', 'lua/ge/extensions/beamngplanet.lua',
                'lua/ge/extensions/beamngearthautostart.lua', 'ui/ui-vue/mods/beamngplanet/',
                'ui/ui-vue/mods/beamngearth/', 'art/beamngearth/',
                'levels/beamng_earth/', 'scripts/beamng_earth', 'scripts/beamng_earth_autostart/',
                'lua/ge/extensions/kissmp/', 'lua/vehicle/extensions/kissmp/', 'scripts/kiss_mp/',
                'settings/kissmp', 'art/kissmp/', 'lua/ge/extensions/core/input/actions/kissmp.json')
    for path in mods.rglob('*'):
        safe_path(path)
        if replacing and (path.is_relative_to(earth) or path.is_relative_to(addon)):
            continue
        if path.is_file() and path.suffix.lower() == '.zip':
            try:
                with zipfile.ZipFile(path) as archive:
                    if any(name.lower().lstrip('/').startswith(prefixes) or name.lower().lstrip('/') in virtual_files for name in archive.namelist()):
                        raise InstallError(f'Earth/Planet collision in archive: {path}')
            except zipfile.BadZipFile as exc:
                raise InstallError(f'cannot inspect mod archive: {path}') from exc
        if path.is_file():
            name = path.relative_to(mods).as_posix().lower()
            # Each unpacked mod contributes its contents at the VFS root.
            virtual = '/'.join(name.split('/')[2:]) if name.startswith('unpacked/') else name
            if virtual.startswith(prefixes) or virtual in virtual_files:
                raise InstallError(f'unknown Earth/Planet mod collision: {path}')


def install(package, destination, profile, game_root, cache_directory, original_launch_options, dry_run=False):
    package, destination, profile, game, cache = map(safe_path, [package, destination, profile, game_root, cache_directory])
    roots = [package, destination, profile, game, cache]
    for index, left in enumerate(roots):
        for right in roots[index + 1:]:
            if left == right or left.is_relative_to(right) or right.is_relative_to(left):
                raise InstallError('package/destination/profile/game/cache roots must be separate and nonoverlapping')
    if not all(p.is_dir() for p in [package, profile, game, cache, destination.parent]):
        raise InstallError('explicit package/profile/game/cache and destination parent directories must exist')
    if any(char in original_launch_options for char in ['\n', '\r', '\x00']) or '%command%' in original_launch_options.lower():
        raise InstallError('foreign Steam wrapper refused; restore its original arguments explicitly first')
    record = verify_package(package)
    compatibility(game, record['compatibility'])
    earth = profile / 'mods/unpacked/beamng_earth'
    addon = profile / 'mods/unpacked/beamng_planet'
    old = None
    if destination.exists():
        try:
            old, old_roots = owned(destination)
        except (OSError, KeyError, ValueError) as exc:
            raise InstallError('unknown existing destination; never overwritten') from exc
        if (old['profile'] != str(profile) or old['originalLaunchOptions'] != original_launch_options
                or old['gameRoot'] != str(game) or old['cacheDirectory'] != str(cache)):
            raise InstallError('owned update profile/game/cache/original Steam arguments differ')
    elif earth.exists():
        current = {name: facts['sha256'] for name, facts in inventory(earth).items()}
        if current != record['originalClientFiles']:
            raise InstallError('existing Earth client is not the exact supported original; no overlay allowed')
    if addon.exists() and not old:
        raise InstallError('unknown existing Planet addon')
    virtual_files = {name.split('/', 1)[1].lower() for name in record['files'] if name.startswith(('client/', 'addon/'))}
    check_collisions(profile, earth, addon, replacing=bool(old or earth.exists()), virtual_files=virtual_files)
    wrapper = f'"{destination / "beamng-earth-companion.exe"}" -- %command%'
    if original_launch_options: wrapper += ' ' + original_launch_options
    result = {'manifest': str(destination / 'ownership.json'), 'proposedLaunchOptions': wrapper,
              'restoreLaunchOptions': original_launch_options, 'dryRun': dry_run}
    if dry_run: return result
    # Fully populate and verify new files BEFORE touching current installation.
    with recovery_stage('.planet-stage-', destination.parent) as stage:
        helper = stage / 'helper'
        helper.mkdir()
        for name in record['files']:
            rel = relative(name)
            target = stage / rel if rel.parts[0] in ('client', 'addon') else helper / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(package / rel, target)
            if digest(target) != record['files'][name]['sha256']:
                raise InstallError(f'copied package hash mismatch: {name}')
        write_json(helper / 'package-manifest.json', record)
        write_json(helper / 'config.json', {'gameRoot': str(game), 'profile': str(profile), 'cacheDir': str(cache),
                   'backendExe': str(destination / relative(record['backendPath'])),
                   'backendSha256': record['files'][record['backendPath']]['sha256']})
        if record['formatVersion'] == 3:
            runtime = record['runtime']
            image_path = helper / relative(record['backendPath']).parent / 'lan-images.json'
            write_json(image_path, {
                'bridgeExe': str(destination / relative(runtime['bridgePath'])),
                'bridgeSha256': record['files'][runtime['bridgePath']]['sha256'],
                'serverExe': str(destination / relative(runtime['serverPath'])),
                'serverSha256': record['files'][runtime['serverPath']]['sha256']})
        had_original = old['hadOriginalClient'] if old else earth.exists()
        if had_original:
            source = destination / 'original-client' if old else earth
            shutil.copytree(source, helper / 'original-client', copy_function=shutil.copyfile)
        manifest = {'formatVersion': 2, 'owner': OWNER, 'destination': str(destination), 'profile': str(profile),
                    'gameRoot': str(game), 'cacheDirectory': str(cache), 'hadOriginalClient': had_original,
                    'originalLaunchOptions': original_launch_options, 'proposedLaunchOptions': wrapper,
                    'packageManifestSha256': digest(package / 'manifest.json'),
                    'directories': {'helper': directories(helper), 'client': directories(stage / 'client'), 'addon': directories(stage / 'addon')},
                    'trees': {'helper': inventory(helper), 'client': inventory(stage / 'client'), 'addon': inventory(stage / 'addon')}}
        write_json(helper / 'ownership.json', manifest)
        moves = []
        try:
            for name, target in [('helper', destination), ('client', earth), ('addon', addon)]:
                target.parent.mkdir(parents=True, exist_ok=True)
                if target.exists():
                    backup = stage / ('previous-' + name)
                    target.rename(backup)
                    moves.append((backup, target))
                (stage / name).rename(target)
                moves.append((target, stage / name))
        except BaseException as exc:
            rollback(moves, stage, exc)
            raise
    return result


def uninstall(manifest_path, current_launch_options, dry_run=False):
    path = safe_path(manifest_path)
    if path.name != 'ownership.json': raise InstallError('expected ownership.json')
    manifest, roots = owned(path.parent)
    steam_removed = current_launch_options == manifest['originalLaunchOptions']
    if not steam_removed and not dry_run:
        raise InstallError('Steam reference removal must be verified: restore the exact original launch options before helper removal')
    result = {'restoreLaunchOptions': manifest['originalLaunchOptions'], 'dryRun': dry_run, 'steamReferenceRemoved': steam_removed,
              'steps': ['Restore original Steam launch options first', 'Remove owned addon and patched client',
                        'Restore exact original client if backed up', 'Remove owned helper directory']}
    if dry_run: return result
    destination = path.parent
    with recovery_stage('.planet-remove-', destination.parent) as stage:
        moves = []
        try:
            for name in ['client', 'addon', 'helper']:
                target = roots[name]
                target.rename(stage / name)
                moves.append((stage / name, target))
            if manifest['hadOriginalClient']:
                source = stage / 'helper/original-client'
                source.rename(roots['client'])
                moves.append((roots['client'], source))
        except BaseException as exc:
            rollback(moves, stage, exc)
            raise
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    add = sub.add_parser('install')
    for field in ['package', 'destination', 'profile', 'game-root', 'cache-directory']:
        add.add_argument('--' + field, required=True, type=Path)
    add.add_argument('--original-launch-options', required=True)
    remove = sub.add_parser('uninstall')
    remove.add_argument('--manifest', required=True, type=Path)
    remove.add_argument('--current-launch-options', required=True)
    for command in [add, remove]: command.add_argument('--dry-run', action='store_true')
    args = vars(parser.parse_args())
    action = args.pop('action')
    if action == 'uninstall': args['manifest_path'] = args.pop('manifest')
    try:
        result = (install if action == 'install' else uninstall)(**args)
    except (InstallError, OSError, ValueError, KeyError) as exc:
        parser.exit(1, f'Planet installation refused: {exc}\n')
    print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == '__main__': main()
