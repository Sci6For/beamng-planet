"""Bounded public-release updater, run from a copy outside the managed installation."""
import argparse
from contextlib import contextmanager
import csv
import ctypes
import errno
import hashlib
import http.client
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from urllib.parse import urljoin, urlsplit
import uuid
import zipfile

import planet_install as installer

REPOSITORY = 'Sci6For/beamng-planet'
ASSET = 'Planet-Package.zip'
API_URL = f'https://api.github.com/repos/{REPOSITORY}/releases/latest'
MAX_ARCHIVE = 256 * 1024 * 1024
MAX_UNPACKED = 1024 * 1024 * 1024
MAX_ENTRIES = 6000
CHECK_INTERVAL = 6 * 60 * 60
NO_WINDOW = 0x08000000 if os.name == 'nt' else 0
VERSION = re.compile(r'(?:v)?(1\.5\.4-planet([1-9][0-9]{0,5}))\Z')


class UpdateError(RuntimeError):
    pass


class UpdateBusy(UpdateError):
    pass


def validate_channel(channel):
    if (not isinstance(channel, dict) or channel.get('repository') != REPOSITORY
            or channel.get('asset') != ASSET or not VERSION.fullmatch(str(channel.get('version', '')))):
        raise UpdateError('unsupported update channel')
    return channel


def select_release(channel, release):
    validate_channel(channel)
    if not isinstance(release, dict) or not isinstance(release.get('tag_name'), str):
        raise UpdateError('invalid release response')
    if not isinstance(release.get('draft'), bool) or not isinstance(release.get('prerelease'), bool):
        raise UpdateError('invalid release flags')
    match = VERSION.fullmatch(release['tag_name'])
    current = VERSION.fullmatch(channel['version'])
    if release['draft'] or release['prerelease'] or not match or int(match[2]) <= int(current[2]):
        return None
    assets = release.get('assets')
    if not isinstance(assets, list): raise UpdateError('missing release assets')
    matches = [item for item in assets if isinstance(item, dict) and item.get('name') == ASSET]
    if len(matches) != 1: raise UpdateError('missing or duplicate package asset')
    asset = matches[0]
    expected_url = f'https://github.com/{REPOSITORY}/releases/download/{release["tag_name"]}/{ASSET}'
    if asset.get('browser_download_url') != expected_url:
        raise UpdateError('package URL is outside the pinned repository')
    if (type(asset.get('size')) is not int or not 0 < asset['size'] <= MAX_ARCHIVE
            or asset.get('state') != 'uploaded'):
        raise UpdateError('invalid package size or state')
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', str(asset.get('digest', ''))):
        raise UpdateError('missing package SHA-256')
    return {'version': match[1], 'asset': asset}


def allowed_download_url(url, original):
    try:
        parts = urlsplit(url)
        if (parts.scheme != 'https' or parts.username or parts.password or parts.fragment
                or parts.port not in (None, 443)):
            return False
        return url == original or parts.hostname in {
            'release-assets.githubusercontent.com', 'objects.githubusercontent.com', 'github-releases.githubusercontent.com'}
    except ValueError:
        return False


def fetch(url, output, limit, total_timeout, *, api=False):
    """Verified TLS, restricted redirects, bounded body and a shared wall-clock deadline."""
    original, deadline = url, time.monotonic() + total_timeout
    for redirect in range(6):
        if (api and url != API_URL) or (not api and not allowed_download_url(url, original)):
            raise UpdateError('untrusted redirect')
        remaining = deadline - time.monotonic()
        if remaining <= 0: raise UpdateError('request timed out')
        parts = urlsplit(url)
        connection = http.client.HTTPSConnection(parts.hostname, timeout=min(8, remaining))
        try:
            target = parts.path + ('?' + parts.query if parts.query else '')
            connection.request('GET', target, headers={'User-Agent': 'BeamNG-Planet-Updater',
                'Accept': 'application/vnd.github+json' if api else 'application/octet-stream',
                'X-GitHub-Api-Version': '2022-11-28'})
            remaining = deadline - time.monotonic()
            if remaining <= 0: raise UpdateError('request timed out')
            connection.sock.settimeout(min(8, remaining))
            response = connection.getresponse()
            if response.status in (301, 302, 303, 307, 308):
                url = urljoin(url, response.getheader('Location', ''))
                continue
            if response.status != 200: raise UpdateError(f'HTTP {response.status}')
            length = response.getheader('Content-Length')
            if length is not None and (not length.isdigit() or int(length) > limit):
                raise UpdateError('response exceeds size limit')
            count = 0
            while True:
                if response.isclosed(): return count
                remaining = deadline - time.monotonic()
                if remaining <= 0: raise UpdateError('request timed out')
                # A Connection: close response detaches connection.sock; the
                # response still owns that socket until its body is consumed.
                sock = connection.sock or response.fp.raw._sock
                sock.settimeout(min(8, remaining))
                chunk = response.read1(min(64 * 1024, limit - count + 1))
                if time.monotonic() > deadline: raise UpdateError('request timed out')
                if not chunk: return count
                count += len(chunk)
                if count > limit: raise UpdateError('response exceeds size limit')
                output.write(chunk)
        finally:
            connection.close()
    raise UpdateError('too many redirects')


def fetch_release():
    output = io.BytesIO()
    fetch(API_URL, output, 2 * 1024 * 1024, 8, api=True)
    return json.loads(output.getvalue())


def verify_download(path, asset):
    if path.stat().st_size != asset['size']: raise UpdateError('download size mismatch')
    sha = hashlib.sha256()
    with path.open('rb') as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b''): sha.update(chunk)
    if 'sha256:' + sha.hexdigest() != asset['digest']: raise UpdateError('download hash mismatch')


def download_asset(asset, target):
    with target.open('xb') as output:
        fetch(asset['browser_download_url'], output, min(asset['size'], MAX_ARCHIVE), 120)
    verify_download(target, asset)


def zip_path(name):
    if not name or '\\' in name or ':' in name or name.startswith('/'):
        raise UpdateError('unsafe ZIP path')
    parts = name.rstrip('/').split('/')
    for part in parts:
        if (not part or part in ('.', '..') or part[-1] in ' .'
                or any(ord(char) < 32 or char in '<>"|?*' for char in part)
                or re.fullmatch(r'(CON|PRN|AUX|NUL|COM[0-9]|LPT[0-9])(?:\..*)?', part, re.I)):
            raise UpdateError('unsafe ZIP path')
    return Path(*parts)


def extract_package(archive, destination):
    installer.safe_path(destination)
    with zipfile.ZipFile(archive) as source:
        entries, names, spellings, total = source.infolist(), {}, {}, 0
        if not entries or len(entries) >= MAX_ENTRIES: raise UpdateError('ZIP entry limit exceeded')
        for entry in entries:
            relative = zip_path(entry.orig_filename)
            key = relative.as_posix().casefold()
            kind = stat.S_IFMT(entry.external_attr >> 16)
            if key in names or kind not in (0, stat.S_IFREG, stat.S_IFDIR) or entry.flag_bits & 1:
                raise UpdateError('duplicate, linked or encrypted ZIP entry')
            names[key] = entry.is_dir()
            for part in [relative, *relative.parents]:
                if part == Path('.'): continue
                normalized = part.as_posix().casefold()
                if normalized in spellings and spellings[normalized] != part.as_posix():
                    raise UpdateError('case-colliding ZIP paths')
                spellings[normalized] = part.as_posix()
            total += entry.file_size
            if entry.file_size < 0 or total > MAX_UNPACKED: raise UpdateError('ZIP unpacked size limit exceeded')
        for name in names:
            if any(parent.as_posix() in names and not names[parent.as_posix()]
                   for parent in Path(name).parents if parent != Path('.')):
                raise UpdateError('ZIP file/directory collision')
        destination.mkdir()
        expanded = 0
        for entry in entries:
            target = installer.safe_path(destination / zip_path(entry.orig_filename))
            if not target.is_relative_to(destination): raise UpdateError('ZIP escaped staging directory')
            if entry.is_dir(): target.mkdir(parents=True, exist_ok=True); continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.open(entry) as incoming, target.open('xb') as outgoing:
                copied = 0
                for chunk in iter(lambda: incoming.read(64 * 1024), b''):
                    copied += len(chunk); expanded += len(chunk)
                    if copied > entry.file_size or expanded > MAX_UNPACKED:
                        raise UpdateError('ZIP expanded size limit exceeded')
                    outgoing.write(chunk)
                if copied != entry.file_size: raise UpdateError('ZIP entry size mismatch')
    return destination


def state_directory(ownership):
    profile = installer.safe_path(ownership['profile'])
    return installer.safe_path(profile / 'settings/beamngEarthUpdates')


@contextmanager
def update_lock(state, timeout=180):
    path = installer.safe_path(state / 'update.lock')
    with path.open('a+b') as file:
        if file.tell() == 0: file.write(b'0'); file.flush()
        file.seek(0)
        deadline = time.monotonic() + timeout
        while True:
            try:
                if os.name == 'nt':
                    import msvcrt
                    msvcrt.locking(file.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except OSError as exc:
                if exc.errno not in (errno.EACCES, errno.EAGAIN): raise
                remaining = deadline - time.monotonic()
                if remaining <= 0: raise UpdateBusy('another updater still owns this profile') from exc
                time.sleep(min(0.25, remaining))
        try:
            yield
        finally:
            file.seek(0)
            if os.name == 'nt': msvcrt.locking(file.fileno(), msvcrt.LK_UNLCK, 1)
            else: fcntl.flock(file.fileno(), fcntl.LOCK_UN)


@contextmanager
def staging(parent):
    parent = installer.safe_path(parent)
    stage = Path(tempfile.mkdtemp(prefix='.planet-update-', dir=parent))
    try:
        yield stage
    finally:
        # Delete only the directory created by this invocation; never follow a
        # substituted junction or a path that escaped its verified parent.
        resolved = installer.safe_path(stage)
        if resolved.parent != parent or not resolved.name.startswith('.planet-update-'):
            raise UpdateError('unsafe staging cleanup')
        shutil.rmtree(resolved)


def wait_parent(pid):
    if pid <= 0 or pid == os.getpid(): raise UpdateError('invalid parent PID')
    if os.name != 'nt': raise UpdateError('Windows handoff required')
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.WaitForSingleObject.restype = wintypes.DWORD
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.OpenProcess(0x00100000, False, pid)
    if not handle:
        if ctypes.get_last_error() == 87: return True  # Process already exited.
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        result = kernel.WaitForSingleObject(handle, 30_000)
        if result == 0xFFFFFFFF: raise ctypes.WinError(ctypes.get_last_error())
        return result == 0
    finally:
        kernel.CloseHandle(handle)


def game_active():
    if os.name != 'nt': raise UpdateError('Windows process inspection required')
    result = subprocess.run(['tasklist.exe', '/FO', 'CSV', '/NH'], capture_output=True,
                            creationflags=NO_WINDOW, timeout=8, check=True)
    names = {'beamng.drive.exe', 'beamng.drive.x64.exe', 'beamng-earth-companion.exe',
             'kissmp-server.exe', 'kissmp-bridge.exe'}
    return any(row and row[0].casefold() in names
               for row in csv.reader(io.StringIO(result.stdout.decode('mbcs', errors='replace'))))


def write_status(state, status, error=None):
    if state is None: return
    value = {'at': time.time(), 'status': status}
    if error: value['error'] = str(error)[:800]
    try: installer.write_json(installer.safe_path(state / 'status.json'), value)
    except OSError: pass


def attempt_update(installation, ownership, state):
    if game_active(): return 'game-active'
    channel = validate_channel(installer.read_json(installation / 'update-channel.json'))
    try:
        checked = installer.read_json(state / 'check.json')
        age = time.time() - checked['checkedAt']
        if checked.get('version') == channel['version'] and 0 <= age < CHECK_INTERVAL: return 'recent-check'
    except (OSError, ValueError, KeyError, TypeError): pass
    candidate = select_release(channel, fetch_release())
    if candidate is None:
        installer.write_json(installer.safe_path(state / 'check.json'), {'checkedAt': time.time(), 'version': channel['version']})
        return 'current'
    archive = installer.safe_path(state / ('download-' + uuid.uuid4().hex + '.zip'))
    try:
        download_asset(candidate['asset'], archive)
        verify_download(archive, candidate['asset'])
        with staging(installation.parent) as stage:
            package = extract_package(archive, stage / 'package')
            record = installer.verify_package(package)
            published = validate_channel(installer.read_json(package / 'update-channel.json'))
            if record.get('version') != candidate['version'] or published['version'] != candidate['version']:
                raise UpdateError('package release version mismatch')
            latest, _ = installer.owned(installation)
            if latest != ownership: raise UpdateError('installation changed during update')
            if game_active(): return 'game-active'
            installer.install(package, installation, Path(ownership['profile']), Path(ownership['gameRoot']),
                              Path(ownership['cacheDirectory']), ownership['originalLaunchOptions'])
        installer.write_json(installer.safe_path(state / 'check.json'), {'checkedAt': time.time(), 'version': candidate['version']})
        return 'updated'
    finally:
        if installer.safe_path(archive).parent != state: raise UpdateError('unsafe download cleanup')
        archive.unlink(missing_ok=True)


@contextmanager
def external_process_environment():
    """Do not make the native helper/game retain this one-file worker's DLLs."""
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith('_PYI_') and key != '_MEIPASS2'}
    environment['PYINSTALLER_RESET_ENVIRONMENT'] = '1'
    kernel = None
    if sys.platform == 'win32':
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.GetDllDirectoryW.argtypes = [ctypes.c_uint, ctypes.c_wchar_p]
        kernel.GetDllDirectoryW.restype = ctypes.c_uint
        kernel.SetDllDirectoryW.argtypes = [ctypes.c_wchar_p]
        kernel.SetDllDirectoryW.restype = ctypes.c_int
        previous = ctypes.create_unicode_buffer(32768)
        if kernel.GetDllDirectoryW(len(previous), previous) >= len(previous):
            raise OSError('DLL directory exceeds Windows path limit')
        if not kernel.SetDllDirectoryW(None):
            raise ctypes.WinError(ctypes.get_last_error())
    try:
        yield environment
    finally:
        if kernel is not None and not kernel.SetDllDirectoryW(previous.value or None):
            raise ctypes.WinError(ctypes.get_last_error())


def handoff(installation, parent_pid, game_argv, profile):
    installation = installer.safe_path(installation)
    profile = installer.safe_path(profile)
    if not game_argv or any('\x00' in arg for arg in game_argv): raise UpdateError('missing or invalid game command')
    state = None
    try:
        if not wait_parent(parent_pid): raise UpdateError('parent did not exit within 30 seconds')
        state = state_directory({'profile': profile}); state.mkdir(parents=True, exist_ok=True)
        if getattr(sys, 'frozen', False) and Path(sys.executable).resolve().is_relative_to(installation):
            raise UpdateError('worker must run outside managed installation')
        with update_lock(state):
            ownership_verified = False
            try:
                ownership, _ = installer.owned(installation)
                if installer.safe_path(ownership['profile']) != profile:
                    raise UpdateError('handoff profile differs from owned installation')
                ownership_verified = True
                write_status(state, attempt_update(installation, ownership, state))
            except Exception as exc:
                write_status(state, 'skipped', exc)
                if not ownership_verified: return 1
            # Keep the lock through CreateProcess, including offline/error
            # fallback. The next updater then observes the running helper in
            # game_active() before it can replace any managed files.
            try:
                helper = installer.safe_path(installation / 'beamng-earth-companion.exe')
                with external_process_environment() as environment:
                    subprocess.Popen([str(helper), '--skip-update', '--', *game_argv], creationflags=NO_WINDOW,
                                     close_fds=True, shell=False, env=environment)
            except OSError as exc:
                write_status(state, 'launch-failed', exc)
                return 1
            return 0
    except UpdateBusy as exc:
        write_status(state, 'update-in-progress', exc)
        # The updater that holds the lock owns the replacement and relaunch.
        # Never race its managed-tree transaction by starting another helper.
        return 1
    except Exception as exc:
        write_status(state, 'skipped', exc)
        return 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--handoff', action='store_true', required=True)
    parser.add_argument('--installation', type=Path, required=True)
    parser.add_argument('--profile', type=Path, required=True)
    parser.add_argument('--parent-pid', type=int, required=True)
    parser.add_argument('game', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    game = args.game[1:] if args.game[:1] == ['--'] else args.game
    if not game: parser.error('game executable and arguments must follow --')
    return handoff(args.installation, args.parent_pid, game, args.profile)


if __name__ == '__main__':
    raise SystemExit(main())
