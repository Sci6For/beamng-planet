"""Self-contained Windows front end for the existing owned Planet installer."""
import argparse
import csv
import io
import json
import os
from pathlib import Path
import subprocess
import sys

from planet_install import compatibility, digest, install, safe_path, verify_package
from planet_mod_import import preview_import, apply_import

NO_WINDOW = 0x08000000


def bundle():
    return Path(getattr(sys, '_MEIPASS', Path(__file__).resolve().parent))


def verified_payload():
    root = bundle()
    package = root / 'payload'
    expected = (root / 'payload.sha256').read_text().strip()
    if digest(package / 'manifest.json') != expected:
        raise ValueError('Повреждён установочный пакет. Скопируйте EXE заново.')
    return package, verify_package(package)


def game_closed():
    result = subprocess.run(['tasklist.exe', '/FO', 'CSV', '/NH'], capture_output=True,
                            creationflags=NO_WINDOW, timeout=15, check=True)
    for row in csv.reader(io.StringIO(result.stdout.decode('mbcs', errors='replace'))):
        if row and row[0].lower() in {
            'beamng.drive.exe', 'beamng.drive.x64.exe', 'beamng-earth-companion.exe',
            'kissmp-server.exe', 'kissmp-bridge.exe'}:
            raise ValueError('Закройте BeamNG и помощник «Планеты», затем повторите установку.')


def install_for_friend(game, target, import_vehicles=False):
    package, record = verified_payload()
    game, target = safe_path(game), safe_path(target)
    # Check all immutable inputs before creating the isolated user directories.
    compatibility(game, record['compatibility'])
    for other in (game, package):
        if target == other or target.is_relative_to(other) or other.is_relative_to(target):
            raise ValueError('Папка «Планеты» должна находиться отдельно от игры и установщика.')
    game_closed()
    profile = target / 'user/current'
    cache = target / 'cache'
    profile.mkdir(parents=True, exist_ok=True)
    cache.mkdir(parents=True, exist_ok=True)
    destination = target / 'companion'
    result = install(package, destination, profile, game, cache, '')
    imported = 0
    import_error = None
    if import_vehicles:
        try:
            preview = preview_import(profile, include_disabled=True)
            if preview['planned']:
                copied = apply_import(preview['sourceProfile'], profile, preview['planned'], include_disabled=True)
                imported = len(copied['copied'])
        except (OSError, ValueError, RuntimeError) as error:
            import_error = str(error)
    args = result['proposedLaunchOptions'] + ' -userpath "' + str(target / 'user') + '"'
    instructions = ('ПЛАНЕТА — установка завершена\n\n'
                    'Steam → BeamNG.drive → Свойства → Общие → Параметры запуска:\n\n'
                    + args + '\n\nЗапускайте игру через Steam. В главном меню откройте «Планета».\n'
                    'Для игры вдвоём используйте одинаковую сборку на обоих ПК.\n'
                    'Друг подключается по приглашению хоста в меню «Планеты».\n'
                    'Города загружаются автоматически; нужен интернет.\n'
                    'Основной профиль и сохранения BeamNG не изменяются.\n')
    (target / 'Начало работы.txt').write_text(instructions, encoding='utf-8-sig')
    return {'ok': True, 'launchOptions': args, 'folder': str(target),
            'version': record.get('version'), 'manifestSha256': digest(package / 'manifest.json'),
            'importedVehicles': imported, 'vehicleImportError': import_error}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--verify-payload', action='store_true')
    parser.add_argument('--install', action='store_true')
    parser.add_argument('--game-root')
    parser.add_argument('--destination')
    parser.add_argument('--import-vehicles', action='store_true')
    parser.add_argument('--report')
    args = parser.parse_args()
    try:
        if args.verify_payload:
            _, record = verified_payload()
            result = {'ok': True, 'files': len(record['files'])}
        elif args.install:
            if not args.game_root or not args.destination or not args.report:
                raise ValueError('Не заданы папки установки.')
            result = install_for_friend(args.game_root, args.destination, args.import_vehicles)
        else:
            command = ['powershell.exe', '-NoProfile', '-STA', '-ExecutionPolicy', 'Bypass',
                       '-File', str(bundle() / 'planet_setup_ui.ps1'), '-Worker', sys.executable]
            return subprocess.run(command, creationflags=NO_WINDOW).returncode
    except Exception as exc:
        result = {'ok': False, 'error': str(exc)}
    if args.report:
        Path(args.report).write_text(json.dumps(result, ensure_ascii=False), encoding='utf-8')
    elif sys.stdout:
        print(json.dumps(result, ensure_ascii=False))
    return 0 if result['ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
