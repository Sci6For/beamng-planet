"""Build a version/hash-pinned Earth client in a fresh owned directory.

The audit source is read only. Existing outputs are always refused; callers
choose a new destination and own any later removal or installation separately.
"""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'compat/earth153/manifest.json'


class BuildError(RuntimeError):
    pass


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def relative(name):
    path = PurePosixPath(name)
    if not name or path.is_absolute() or '..' in path.parts or '\\' in name or ':' in name:
        raise BuildError(f'unsafe manifest path: {name}')
    return Path(*path.parts)


def build(source_root, output_dir, manifest_path=MANIFEST):
    source = Path(source_root).resolve(strict=True)
    output = Path(output_dir).absolute()
    manifest_path = Path(manifest_path).resolve(strict=True)
    if output.exists() or output.is_symlink():
        raise BuildError(f'output already exists: {output}')
    output = output.resolve()
    if output.is_relative_to(source) or source.is_relative_to(output):
        raise BuildError('output overlaps immutable source')
    if not output.parent.is_dir():
        raise BuildError('output parent must already exist')
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    if (source / 'VERSION').read_text(encoding='utf-8').strip() != manifest['version']:
        raise BuildError('source version mismatch')
    mod = source / relative(manifest['sourceDirectory'])
    expected = manifest['originalFiles']
    actual = set()
    for path in mod.rglob('*'):
        if path.is_symlink() or path.is_junction():
            raise BuildError(f'source links are not owned files: {path}')
        if path.is_file():
            actual.add(path.relative_to(mod).as_posix())
    if actual != set(expected):
        raise BuildError('source file set mismatch (unknown or missing files)')
    for name, sha in expected.items():
        if digest(mod / relative(name)) != sha:
            raise BuildError(f'original hash mismatch: {name}')
    patches = []
    for item in manifest['patches']:
        patch = manifest_path.parent / relative(item['path'])
        # Git's Windows checkout may use CRLF. Patch hashes pin canonical LF;
        # original source hashes above always pin the exact source bytes.
        patch_bytes = patch.read_bytes().replace(b'\r\n', b'\n')
        if hashlib.sha256(patch_bytes).hexdigest() != item['sha256']:
            raise BuildError(f'patch hash mismatch: {item["path"]}')
        patches.append((patch.name, patch_bytes))
    # TemporaryDirectory removes only its own fresh stage, even on failure.
    with tempfile.TemporaryDirectory(prefix='.client-stage-', dir=output.parent) as temporary:
        stage = Path(temporary) / 'client'
        stage.mkdir()
        for name, sha in expected.items():
            dest = stage / relative(name)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(mod / relative(name), dest)
            if digest(dest) != sha:  # catch source changes between verification and copy
                raise BuildError(f'copied original hash mismatch: {name}')
        for index, (patch_name, patch_bytes) in enumerate(patches):
            patch = Path(temporary) / f'{index}-{patch_name}'
            patch.write_bytes(patch_bytes)
            for mode in (['--check'], []):
                result = subprocess.run(['git', '-C', str(stage), 'apply', *mode, str(patch)],
                                        capture_output=True, text=True, check=False)
                if result.returncode:
                    raise BuildError(f'patch failed: {patch.name}: {result.stderr.strip()}')
        for name, addon in manifest['additions'].items():
            if name in expected:
                raise BuildError(f'addition would overwrite original: {name}')
            dest = stage / relative(name)
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative(addon), dest)
        files = {}
        for path in sorted(stage.rglob('*')):
            if path.is_file():
                name = path.relative_to(stage).as_posix()
                files[name] = {'sourceSha256': expected.get(name), 'sha256': digest(path)}
        record = {'formatVersion': 1, 'owner': 'beamng-planet-earth153',
                  'sourceCommit': manifest['sourceCommit'], 'version': manifest['version'],
                  'compatManifestSha256': digest(manifest_path), 'files': files}
        (stage / 'build-manifest.json').write_text(json.dumps(record, indent=2) + '\n', encoding='utf-8')
        # rename is same-volume publication; never merge or remove an existing output.
        if output.exists():
            raise BuildError(f'output already exists: {output}')
        stage.rename(output)
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--manifest', type=Path, default=MANIFEST)
    args = parser.parse_args()
    try:
        result = build(args.source, args.output, args.manifest)
    except (BuildError, OSError, ValueError, KeyError) as exc:
        parser.exit(1, f'Earth client build failed: {exc}\n')
    print(json.dumps({'output': str(args.output.resolve()), 'files': len(result['files']),
                      'sourceCommit': result['sourceCommit']}))


if __name__ == '__main__':
    main()
