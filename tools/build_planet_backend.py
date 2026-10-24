"""Build the maintained Earth backend into a fresh workspace artifact directory."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'vendor/earth154'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    if not output.is_relative_to(ROOT / 'artifacts') or output.exists():
        parser.error('output must be a fresh directory inside project artifacts')
    output.mkdir(parents=True)
    spec = importlib.util.spec_from_file_location('earth_build', SOURCE / 'tools/build_app.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    version = module.version()
    _, ico = module.make_icons(str(output))
    dist = Path(module.pyinstaller(str(output), ico, onefile=True, ver=version))
    exe = dist / 'BeamNG.Earth.exe'
    record = {
        'version': version,
        'executable': str(exe),
        'sha256': hashlib.sha256(exe.read_bytes()).hexdigest(),
        'sourceFiles': {
            p.relative_to(SOURCE).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted((SOURCE / 'server').glob('*.py'))
        },
        'nativeSha256': hashlib.sha256((SOURCE / 'server/earthfast.pyd').read_bytes()).hexdigest(),
    }
    (output / 'build.json').write_text(json.dumps(record, indent=2), encoding='utf-8')
    print(json.dumps({k: v for k, v in record.items() if k != 'sourceFiles'}, indent=2))


if __name__ == '__main__':
    main()
