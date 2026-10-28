#!/usr/bin/env python3
"""Read-only eligibility and draft-integrity checks for the shipping workflow."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys


class GuardError(ValueError):
    pass


def validate_tag(tag):
    if not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*)?(?:\+[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*)?', tag):
        raise GuardError('Invalid release tag')


def publish_requested(event, ref, manual_publish):
    requested = event == 'push' or (event == 'workflow_dispatch' and manual_publish)
    if requested and ref != 'refs/heads/main':
        raise GuardError('Publishing is permitted only from refs/heads/main')
    return requested


def tag_status(returncode):
    if returncode == 0:
        return True
    if returncode == 2:
        return False
    raise GuardError(f'Cannot establish tag absence: git ls-remote exited {returncode}')


def remote_tag(tag):
    result = subprocess.run(['git', 'ls-remote', '--exit-code', '--tags', 'origin', f'refs/tags/{tag}'],
                            stdout=subprocess.PIPE, text=True, check=False)
    if not tag_status(result.returncode):
        return None
    entries = [line.split() for line in result.stdout.splitlines()]
    if (len(entries) != 1 or len(entries[0]) != 2 or entries[0][1] != f'refs/tags/{tag}'
            or not re.fullmatch(r'[0-9a-f]{40}', entries[0][0])):
        raise GuardError('Git returned an invalid release tag reference')
    return entries[0][0]


def validate_remote_tag(tag_sha, built_sha, prepared=False):
    # Some draft-creation paths may materialize the tag before publication.
    # Accept it only after this run proved absence and only at its exact SHA.
    if tag_sha is not None and not (prepared and tag_sha == built_sha):
        raise GuardError('Release tag already exists or differs from the prepared source SHA')


def gh_json(path, paginate=False):
    command = ['gh', 'api']
    if paginate:
        command.extend(['--paginate', '--slurp'])
    result = subprocess.run([*command, path], stdout=subprocess.PIPE, text=True, check=False)
    if result.returncode:
        raise GuardError('GitHub API request failed; refusing to publish')
    try:
        return json.loads(result.stdout)
    except (ValueError, TypeError) as error:
        raise GuardError('GitHub API returned invalid JSON') from error


def paged_list(path):
    pages = gh_json(path, paginate=True)
    if not isinstance(pages, list) or any(not isinstance(page, list) for page in pages):
        raise GuardError('GitHub API returned an invalid paginated list')
    return [entry for page in pages for entry in page]


def validate_main(ref, built_sha, current_sha):
    if ref != 'refs/heads/main':
        raise GuardError('Publishing is permitted only from refs/heads/main')
    if not all(isinstance(sha, str) and re.fullmatch(r'[0-9a-f]{40}', sha) for sha in (built_sha, current_sha)):
        raise GuardError('Invalid built or current main SHA')
    if current_sha != built_sha:
        raise GuardError('Main has advanced beyond the built source SHA; rebuild the current main commit')


def validate_draft(release, tag, built_sha, required=False):
    if release is None:
        if required:
            raise GuardError('The matching draft release is missing')
        return
    if not isinstance(release, dict) or release.get('draft') is not True or release.get('published_at') is not None:
        raise GuardError('An existing published release must never be overwritten')
    if release.get('tag_name') != tag or release.get('target_commitish') != built_sha:
        raise GuardError('Existing draft does not target the built SHA; inspect it without automatic deletion or retargeting')
    if type(release.get('id')) is not int or release['id'] <= 0:
        raise GuardError('Invalid draft release ID')


def local_assets(directory):
    expected = {}
    for path in sorted(directory.iterdir()):
        if path.is_symlink() or not path.is_file():
            raise GuardError('Release distribution must contain regular files only')
        checksum = hashlib.sha256()
        with path.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                checksum.update(chunk)
        expected[path.name] = {'size': path.stat().st_size, 'sha256': checksum.hexdigest()}
    if not expected:
        raise GuardError('Release distribution is empty')
    return expected


def validate_assets(expected, remote):
    if not isinstance(remote, list):
        raise GuardError('Invalid remote release asset list')
    names = set()
    for asset in remote:
        if not isinstance(asset, dict) or not isinstance(asset.get('name'), str):
            raise GuardError('Invalid remote release asset')
        name = asset['name']
        if name in names or name not in expected:
            raise GuardError(f'Duplicate or unexpected remote release asset: {name}')
        names.add(name)
        if asset.get('state') != 'uploaded' or type(asset.get('size')) is not int or asset['size'] != expected[name]['size']:
            raise GuardError(f'Remote release asset is incomplete or has a different size: {name}')
        digest = asset.get('digest')
        if not isinstance(digest, str) or digest.lower() != f'sha256:{expected[name]["sha256"]}':
            raise GuardError(f'Remote release asset SHA256 digest is missing or differs: {name}')
    if names != set(expected):
        raise GuardError(f'Remote release assets are missing: {sorted(set(expected) - names)}')


def publication_payload(tag, notes):
    prerelease = '-' in tag
    return {'draft': False, 'name': f'gipny-i2p {tag}', 'body': notes,
            'prerelease': prerelease, 'make_latest': 'false' if prerelease else 'true'}


def workflow_context(tag):
    context = {name: os.environ.get(environment, '') for name, environment in (
        ('repository', 'GITHUB_REPOSITORY'), ('sha', 'GITHUB_SHA'),
        ('ref', 'GITHUB_REF'), ('run', 'GITHUB_RUN_ID'), ('attempt', 'GITHUB_RUN_ATTEMPT'))}
    if not context['run'].isdigit() or not context['attempt'].isdigit():
        raise GuardError('Missing GitHub workflow run identity')
    return {'schema': 1, 'tag': tag, **context}


def validate_receipt(receipt, context):
    if not isinstance(receipt, dict) or receipt.get('tag_was_absent') is not True:
        raise GuardError('Missing successful pre-create tag-absence check')
    if any(receipt.get(key) != value for key, value in context.items()):
        raise GuardError('Draft preparation receipt belongs to another workflow run or source SHA')


def checked_draft(tag, required, prepared=False):
    repository = os.environ.get('GITHUB_REPOSITORY', '')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
        raise GuardError('Invalid GitHub repository')
    built_sha = os.environ.get('GITHUB_SHA', '')
    branch = gh_json(f'repos/{repository}/git/ref/heads/main')
    reference = branch.get('object') if isinstance(branch, dict) else None
    current_sha = reference.get('sha') if isinstance(reference, dict) else None
    validate_main(os.environ.get('GITHUB_REF', ''), built_sha, current_sha)
    validate_remote_tag(remote_tag(tag), built_sha, prepared)
    # A successful paginated list distinguishes absence from API/auth failures
    # and includes drafts visible to the publishing token.
    releases = paged_list(f'repos/{repository}/releases?per_page=100')
    if any(not isinstance(release, dict) for release in releases):
        raise GuardError('Invalid GitHub release list')
    matching = [release for release in releases if release.get('tag_name') == tag]
    if len(matching) > 1:
        raise GuardError('More than one release uses the selected tag')
    release = matching[0] if matching else None
    validate_draft(release, tag, built_sha, required=required)
    return repository, release


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    eligibility = commands.add_parser('eligibility')
    eligibility.add_argument('--tag', required=True)
    eligibility.add_argument('--manual-publish', choices=['true', 'false'], default='false')
    prepare = commands.add_parser('prepare')
    prepare.add_argument('--tag', required=True)
    prepare.add_argument('--receipt', type=Path, required=True)
    finalize = commands.add_parser('finalize')
    finalize.add_argument('--tag', required=True)
    finalize.add_argument('--dist', type=Path, required=True)
    finalize.add_argument('--receipt', type=Path, required=True)
    finalize.add_argument('--notes', type=Path, required=True)
    finalize.add_argument('--payload', type=Path, required=True)
    args = parser.parse_args()
    try:
        validate_tag(args.tag)
        if args.command == 'eligibility':
            requested = publish_requested(os.environ.get('GITHUB_EVENT_NAME', ''),
                                          os.environ.get('GITHUB_REF', ''), args.manual_publish == 'true')
            exists = remote_tag(args.tag) is not None if requested else False
            if exists and args.manual_publish == 'true':
                raise GuardError('Release tag already exists; explicit publishing cannot overwrite it')
            print(f'publish={str(requested and not exists).lower()}')
        else:
            context = workflow_context(args.tag)
            receipt = None
            if args.command == 'finalize':
                try:
                    receipt = json.loads(args.receipt.read_text(encoding='utf-8'))
                except ValueError as error:
                    raise GuardError('Invalid draft preparation receipt') from error
                validate_receipt(receipt, context)
            repository, release = checked_draft(args.tag, required=args.command == 'finalize',
                                                prepared=receipt is not None)
            if args.command == 'prepare':
                args.receipt.write_text(json.dumps({**context, 'tag_was_absent': True,
                    'draft_id': release['id'] if release else None}), encoding='utf-8')
                print('existing-draft' if release else 'absent')
            else:
                if receipt.get('draft_id') is not None and receipt['draft_id'] != release['id']:
                    raise GuardError('Prepared draft was replaced before publication')
                remote = paged_list(f'repos/{repository}/releases/{release["id"]}/assets?per_page=100')
                validate_assets(local_assets(args.dist), remote)
                args.payload.write_text(json.dumps(publication_payload(args.tag, args.notes.read_text(encoding='utf-8'))),
                                        encoding='utf-8')
                print('Draft source SHA and complete uploaded asset set verified', file=sys.stderr)
                print(release['id'])
    except (GuardError, OSError) as error:
        print(f'release-guard: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
