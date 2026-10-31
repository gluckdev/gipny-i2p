#!/usr/bin/env python3
"""Source archive and per-job hashes; no compiler, binary download or env dump."""
import argparse
import copy
import glob
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[2]


def git(*args, cwd=ROOT):
    return subprocess.check_output(['git', *args], cwd=cwd)


def digest(path):
    with Path(path).open('rb') as stream:
        checksum = hashlib.sha256()
        while chunk := stream.read(1024 * 1024):
            checksum.update(chunk)
        return checksum.hexdigest()


def reviewed_lockfiles():
    """Require exact source bytes before and after a build, without normalization."""
    locks = {}
    for name in ('Cargo.lock', 'core/relay/Cargo.lock', 'ui/package-lock.json'):
        path = ROOT / name
        if not path.is_file():
            raise ValueError(f'reviewed dependency lockfile is missing: {name}')
        expected = git('show', f'HEAD:{name}')
        actual = path.read_bytes()
        if actual != expected:
            reason = ('CRLF checkout differs from committed LF bytes'
                      if actual.replace(b'\r\n', b'\n') == expected
                      else 'dependency content differs from the source commit')
            raise ValueError(f'build input differs from reviewed dependency lockfile: {name}: {reason}')
        locks[name] = hashlib.sha256(actual).hexdigest()
    return locks


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + '\n', encoding='utf-8')


def repositories(repo=ROOT, prefix='', require_initialized=False):
    """Inspect gitlinks recursively, retaining expected and actual source SHA."""
    commit = git('rev-parse', 'HEAD', cwd=repo).decode().strip()
    if not re.fullmatch('[0-9a-f]{40}', commit):
        raise ValueError('invalid source commit')
    yield repo, prefix, commit, commit
    for entry in git('ls-tree', '-r', '-z', 'HEAD', cwd=repo).split(b'\0'):
        if not entry:
            continue
        metadata, raw_path = entry.split(b'\t', 1)
        mode, kind, expected = metadata.decode().split()
        if mode != '160000':
            continue
        path = raw_path.decode()
        child = repo / path
        if not (child / '.git').exists():
            if require_initialized:
                raise ValueError(f'uninitialized source submodule: {prefix}{path}')
            continue
        for nested, relative, actual, pinned in repositories(child, prefix + path + '/', require_initialized):
            yield nested, relative, actual, expected if nested == child else pinned


def source_identity():
    actual = git('rev-parse', 'HEAD').decode().strip()
    expected = os.environ.get('GITHUB_SHA', actual)
    if actual != expected:
        raise ValueError(f'checkout {actual} differs from workflow source {expected}')
    repository = os.environ.get('GITHUB_REPOSITORY', 'gluckdev/gipny-i2p')
    return {'repository': f'https://github.com/{repository}', 'commit': actual,
            'submodules': [{'path': prefix.rstrip('/'), 'commit': commit, 'gitlink': pinned}
                           for _, prefix, commit, pinned in repositories() if prefix]}


def guard():
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('build provenance must be generated inside GitHub Actions')
    for name in ('I2P_EMBED_STUB', 'I2P_EMBED_SKIP_NATIVE'):
        if os.environ.get(name, '0') not in ('', '0'):
            raise ValueError(f'{name} is forbidden in shipping builds')
    if os.environ.get('CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS', 'false').lower() not in ('false', '0', ''):
        raise ValueError('release debug assertions are forbidden')


def source_archive(output):
    guard()
    identity = source_identity()
    if any(item['commit'] != item['gitlink'] for item in identity['submodules']):
        raise ValueError('source package must use exactly the pinned public gitlinks')
    version = json.loads((ROOT / 'core/tauri.conf.json').read_text())['version']
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f'gipny-source_{version}_{identity["commit"]}.tar.gz'
    epoch = int(git('show', '-s', '--format=%ct', 'HEAD'))
    root_prefix = f'gipny-source-{identity["commit"]}/'
    # git archive takes committed public source, never local target/, SDKs,
    # signing keys, caches, downloaded executables or the .git credential store.
    with archive.open('wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', mtime=epoch) as compressed:
        with tarfile.open(fileobj=compressed, mode='w|') as destination:
            for repo, prefix, commit, _ in repositories(require_initialized=True):
                process = subprocess.Popen(['git', 'archive', '--format=tar', commit], cwd=repo, stdout=subprocess.PIPE)
                try:
                    with tarfile.open(fileobj=process.stdout, mode='r|') as source:
                        for member in source:
                            entry = copy.copy(member)
                            entry.name = root_prefix + prefix + member.name
                            entry.uid = entry.gid = 0
                            entry.uname = entry.gname = ''
                            entry.mtime = epoch
                            destination.addfile(entry, source.extractfile(member) if member.isfile() else None)
                finally:
                    process.stdout.close()
                    if process.wait() != 0:
                        raise ValueError(f'cannot archive public source {prefix}')
    identity['archive'] = {'name': archive.name, 'sha256': digest(archive), 'size': archive.stat().st_size}
    write_json(output / 'SOURCE-MANIFEST.json', identity)


def tools():
    result = {}
    for name, command in [('rust', ['rustc', '-Vv']), ('cargo', ['cargo', '-V']),
                          ('tauri-cli', ['cargo', 'tauri', '--version']),
                          ('node', ['node', '--version']), ('python', ['python3', '--version']),
                          ('clang', ['clang', '--version']), ('cmake', ['cmake', '--version']),
                          ('brew-dependencies', ['brew', 'list', '--versions', 'boost', 'openssl@3']),
                          ('linux-packages', ['dpkg-query', '-W', 'libboost-dev', 'libssl-dev', 'zlib1g-dev'])]:
        try:
            result[name] = subprocess.check_output(command, stderr=subprocess.STDOUT, timeout=15).decode(errors='replace').strip()
        except (OSError, subprocess.SubprocessError):
            pass
    return result


def record(output, target, patterns, input_manifests, inputs):
    guard()
    files = sorted({Path(path) for pattern in patterns for path in glob.glob(pattern, recursive=True) if Path(path).is_file()})
    if not files:
        raise ValueError('no build artifacts to record')
    names = [path.name for path in files]
    if len(names) != len(set(names)):
        raise ValueError('duplicate build artifact basename')
    dependencies = []
    for path in input_manifests:
        manifest = json.loads(Path(path).read_text())
        if manifest['source']['commit'] != source_identity()['commit']:
            raise ValueError('native input was built from another application source commit')
        # The native library in this directory must match its builder's hash.
        for artifact in manifest['artifacts']:
            native = Path(path).parent / artifact['name']
            if not native.is_file() or digest(native) != artifact['sha256']:
                raise ValueError(f'native build input differs from provenance: {native.name}')
        dependencies.append(manifest)
    inputs_list = []
    for directory in inputs:
        for path in sorted(Path(directory).rglob('*')):
            if path.is_file() and not path.is_symlink():
                inputs_list.append({'name': str(path.relative_to(directory)), 'sha256': digest(path), 'size': path.stat().st_size})
    locks = reviewed_lockfiles()
    write_json(output, {
        'schema': 1, 'source': source_identity(), 'target': target,
        'builder': {'service': 'GitHub Actions', 'workflow': os.environ.get('GITHUB_WORKFLOW_REF'),
                    'run': f'https://github.com/{os.environ["GITHUB_REPOSITORY"]}/actions/runs/{os.environ["GITHUB_RUN_ID"]}',
                    'attempt': os.environ.get('GITHUB_RUN_ATTEMPT'), 'job': os.environ.get('GITHUB_JOB'),
                    'os': platform.system(), 'architecture': platform.machine()},
        'tools': tools(), 'lockfiles': locks, 'native_inputs': dependencies,
        'public_source_patches': {str(path.relative_to(ROOT)): digest(path)
                                 for path in sorted((ROOT / 'i2p-embed/patches').glob('*.patch'))}
                                 | ({'scripts/apply-i2pd-memory-patches.sh': digest(ROOT / 'scripts/apply-i2pd-memory-patches.sh')}
                                    if (ROOT / 'scripts/apply-i2pd-memory-patches.sh').is_file() else {}),
        'build_config': {key: os.environ[key] for key in ('NDK_VERSION',
                         'GIPNY_VCPKG_COMMIT', 'I2PD_REV', 'MACOSX_DEPLOYMENT_TARGET',
                         'I2P_EMBED_STATIC_LIBS', 'GIPNY_RESEED_SOURCE_SHA', 'GIPNY_DHT_SEEDS') if key in os.environ},
        'downloaded_inputs': inputs_list,
        'artifacts': [{'name': path.name, 'sha256': digest(path), 'size': path.stat().st_size} for path in files],
    })


def collect(directory, output):
    """Verify every shipping asset before flattening it and publish one index."""
    output.mkdir(parents=True, exist_ok=True)
    expected_sha = source_identity()['commit']
    manifests = []
    source = None
    indexed = {}
    for path in sorted(directory.rglob('*.json')):
        if path.name == 'SOURCE-MANIFEST.json':
            if source is not None:
                raise ValueError('multiple source manifests')
            source = json.loads(path.read_text())
        elif path.name.startswith('build-manifest-'):
            manifest = json.loads(path.read_text())
            if manifest['source']['commit'] != expected_sha:
                raise ValueError('build source SHA differs from the release checkout')
            manifests.append(manifest)
            for artifact in manifest['artifacts']:
                if artifact['name'] in indexed:
                    raise ValueError(f'duplicate build provenance for {artifact["name"]}')
                indexed[artifact['name']] = artifact
        else:
            raise ValueError(f'unexpected JSON release input: {path.name}')
    if source is None or source['commit'] != expected_sha or not manifests:
        raise ValueError('source/build provenance missing or from another source SHA')
    indexed[source['archive']['name']] = source['archive']
    remaining = dict(indexed)
    for path in sorted(directory.rglob('*')):
        if not path.is_file() or path.name.endswith('.json'):
            continue
        if path.name not in remaining:
            raise ValueError(f'artifact has no unique source provenance: {path.name}')
        artifact = remaining.pop(path.name)
        if path.is_symlink() or path.stat().st_size != artifact['size'] or digest(path) != artifact['sha256']:
            raise ValueError(f'artifact checksum/size differs from build provenance: {path.name}')
        destination = output / path.name
        if destination.exists():
            raise ValueError(f'duplicate release asset: {path.name}')
        shutil.copyfile(path, destination)
    if remaining:
        raise ValueError(f'missing build artifacts: {sorted(remaining)}')
    write_json(output / 'SOURCE-MANIFEST.json', source)
    write_json(output / 'BUILD-MANIFEST.json', {'schema': 1, 'source': source, 'builds': manifests})
    lines = [f'{digest(path)}  {path.name}\n' for path in sorted(output.iterdir()) if path.is_file() and path.name != 'SHA256SUMS.txt']
    (output / 'SHA256SUMS.txt').write_text(''.join(lines), encoding='utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    sub.add_parser('locks', help='check exact committed dependency bytes before a build')
    source = sub.add_parser('source'); source.add_argument('output', type=Path)
    build = sub.add_parser('record'); build.add_argument('output', type=Path); build.add_argument('target')
    build.add_argument('--artifact', action='append', required=True)
    build.add_argument('--input-manifest', action='append', default=[])
    build.add_argument('--inputs', action='append', default=[])
    release = sub.add_parser('collect'); release.add_argument('directory', type=Path); release.add_argument('output', type=Path)
    args = parser.parse_args()
    if args.command == 'locks':
        guard()
        print(json.dumps(reviewed_lockfiles(), sort_keys=True))
    elif args.command == 'source': source_archive(args.output)
    elif args.command == 'record': record(args.output, args.target, args.artifact, args.input_manifest, args.inputs)
    else: collect(args.directory, args.output)


if __name__ == '__main__':
    main()
