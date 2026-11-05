#!/usr/bin/env python3
"""Package the pinned native voice runtime without installing or copying the CLI."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath, PureWindowsPath
import platform
import shutil
import subprocess
import tempfile
import tarfile
import urllib.request

BUILD = 'a956835d020762cb2b570053af06f643a11c0ecc'
VERSION = '0.160.0'
PACKAGES = {
    'aarch64-apple-darwin': ('aarch64-apple-darwin', None),
    'x86_64-unknown-linux-gnu': ('x86_64-unknown-linux-musl', '4fcc47ab57f52ff75363951a8761146cd10c8288bd86fed45487dbb204a16b71'),
    'aarch64-unknown-linux-gnu': ('aarch64-unknown-linux-musl', '7f0fe42ff22ecfa3a47bc4a34f5b22c4218b431a4ec0aba51c7d98299f07900c'),
    'x86_64-pc-windows-msvc': ('x86_64-pc-windows-msvc', '7f7fbbc8d6fd4ea2f3b13855ef47ea59663ba7e61fb2e9821df37163b8030891'),
    'aarch64-pc-windows-msvc': ('aarch64-pc-windows-msvc', '0bb6ecbad9c2f5d352ad539bbe43d627b32453bad1d61e5315ec9868f78e1b3c'),
}


def host_target():
    arch = {'amd64': 'x86_64', 'arm64': 'aarch64'}.get(platform.machine().lower(), platform.machine().lower())
    suffix = {'Darwin': 'apple-darwin', 'Linux': 'unknown-linux-gnu', 'Windows': 'pc-windows-msvc'}.get(platform.system())
    target = f'{arch}-{suffix}'
    if target not in PACKAGES:
        raise ValueError(f'unsupported voice target: {target}')
    return target


def relative_path(name):
    path = PurePosixPath(name)
    if (not name or '\\' in name or ':' in name or path.is_absolute()
            or PureWindowsPath(name).drive or '..' in path.parts or path.as_posix() != name
            or name == '.'):
        raise ValueError('invalid manifest path')
    return Path(*path.parts)


def checksum(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def download_package(directory, target):
    app_target, digest = PACKAGES[target]
    if digest is None:
        raise ValueError('supply an explicit source package for macOS')
    name = f'codex-package-{app_target}.tar.gz'
    archive = directory / name
    url = f'https://github.com/openai/codex/releases/download/rust-v{VERSION}/{name}'
    # Only the build machine downloads this fixed, checksum-pinned release.
    with urllib.request.urlopen(url, timeout=120) as response, archive.open('wb') as output:
        shutil.copyfileobj(response, output)
    if checksum(archive) != digest:
        raise ValueError('voice archive checksum mismatch')
    package = directory / 'package'
    package.mkdir()
    with tarfile.open(archive) as source:
        source.extractall(package, filter='data')
    return package


def install(package, destination, target=None):
    target = target or host_target()
    app_target, _ = PACKAGES[target]
    # The helper only initializes from a `codex-resources/voice` directory; it
    # exits with code 23 on initializeRuntime anywhere else.
    if (destination.parent.name, destination.name) != ('codex-resources', 'voice'):
        raise ValueError('destination must end in codex-resources/voice')
    metadata = json.loads((package / 'codex-package.json').read_text())
    if (metadata['layoutVersion'], metadata['version'], metadata['target']) != (1, VERSION, app_target):
        raise ValueError('unsupported Codex package layout, version or target')
    source = package / 'codex-resources/voice'
    manifest = json.loads((source / 'manifest.json').read_text())
    if (manifest['buildCommit'], manifest['appVersion'], manifest['appTarget'], manifest['voiceTarget']) != (BUILD, VERSION, app_target, target):
        raise ValueError(f'unsupported voice package; expected pinned {VERSION} {target}')
    for relative, digest in manifest['sha256'].items():
        path = package / relative_path(relative)
        if checksum(path) != digest:
            raise ValueError('voice package checksum mismatch')
    # Reject untracked runtime files and symlinks before signing/copying code.
    for path in source.rglob('*'):
        if path.is_symlink():
            raise ValueError('voice runtime symlinks are unsupported')
        if path.is_file() and path.name != 'manifest.json' and path.relative_to(package).as_posix() not in manifest['sha256']:
            raise ValueError('untracked runtime resource')
    helper = 'bin/codex-voice-host' + ('.exe' if target.endswith('windows-msvc') else '')
    for required in [helper, 'runtime.json', 'NOTICE.md', 'sources.json']:
        if not (source / required).is_file():
            raise ValueError(f'missing runtime resource: {required}')
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
        stage = Path(temporary) / 'voice'
        shutil.copytree(source, stage)
        shutil.copyfile(Path(__file__).resolve().parents[1] / "dist/voice/Codex-LICENSE.txt", stage / "licenses/Codex-LICENSE.txt")
        # Ship the runtime byte-for-byte: upstream already signs the helper and
        # every library with OpenAI's Developer ID, hardened runtime and a
        # secure timestamp, so re-signing would only downgrade them.
        runtime = json.loads((stage / 'runtime.json').read_text())
        if (runtime['target'], runtime['sourceCommit']) != (target, BUILD):
            raise ValueError('incompatible native runtime')
        for library in runtime['libraries']:
            if checksum(stage / relative_path(library['path'])) != library['sha256']:
                raise ValueError(f"runtime library does not match runtime.json: {library['path']}")
        if target.endswith('apple-darwin'):
            for path in sorted(stage.rglob('*')):
                if path.is_file() and (path.suffix == '.dylib' or path.name == 'codex-voice-host'):
                    subprocess.run(['codesign', '--verify', '--strict', str(path)], check=True)
        hashes = {p.relative_to(stage).as_posix(): checksum(p)
                  for p in sorted(stage.rglob('*')) if p.is_file()}
        (stage / 'zeron-runtime.json').write_text(json.dumps({
            'protocol': 1, 'buildCommit': BUILD, 'sourceVersion': VERSION,
            'target': target, 'sha256': hashes}, indent=2) + '\n')
        if destination.exists():
            shutil.rmtree(destination)
        shutil.move(str(stage), destination)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument('--package', type=Path, help='explicit standalone Codex package directory')
    inputs.add_argument('--download', action='store_true', help='download the checksum-pinned source package on the build machine')
    parser.add_argument('--target', choices=PACKAGES, help='media target; defaults to the build machine')
    parser.add_argument('--destination', type=Path, required=True)
    args = parser.parse_args()
    target = args.target or host_target()
    if args.download:
        with tempfile.TemporaryDirectory(prefix='zeron-voice-package-') as temporary:
            package = download_package(Path(temporary), target)
            install(package, args.destination.resolve(), target)
    else:
        install(args.package.resolve(), args.destination.resolve(), target)
