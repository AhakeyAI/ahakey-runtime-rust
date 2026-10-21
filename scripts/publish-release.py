#!/usr/bin/env python3
"""Publish only native installers and generate release notes from Git commits."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile

from release_notes import generate_notes, git
from runtime_installers import installer_suffix

TARGETS = ['aarch64-apple-darwin', 'x86_64-apple-darwin',
           'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']


def release_assets(directory, tag):
    assets = [directory / f'ahakey-runtime-{tag}-{target}{installer_suffix(target)}' for target in TARGETS]
    for path in assets:
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f'Missing installer: {path}')
    return assets


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--dist', type=Path, default=Path('dist'))
    parser.add_argument('--repo', type=Path, default=Path('.'))
    parser.add_argument('--update-existing', action='store_true')
    args = parser.parse_args()
    if args.tag != 'v' + args.version or git(args.repo, 'rev-parse', f'refs/tags/{args.tag}^{{commit}}') != args.commit:
        raise ValueError('Release version/source does not match its tag')
    assets = release_assets(args.dist, args.tag)
    repository = json.loads(subprocess.check_output(['gh', 'repo', 'view', '--json', 'nameWithOwner'], text=True))['nameWithOwner']
    notes = generate_notes(args.repo, args.tag, args.commit, repository)
    with tempfile.TemporaryDirectory(prefix='ahakey-release-') as temporary:
        notes_file = Path(temporary) / 'notes.md'
        notes_file.write_text(notes, encoding='utf-8')
        if args.update_existing:
            # Repackage the same tagged source. The tag is never moved or recreated.
            subprocess.run(['gh', 'release', 'view', args.tag], check=True, stdout=subprocess.DEVNULL)
            subprocess.run(['gh', 'release', 'upload', args.tag, *map(str, assets), '--clobber'], check=True)
            subprocess.run(['gh', 'release', 'edit', args.tag, '--notes-file', str(notes_file)], check=True)
            old = json.loads(subprocess.check_output(['gh', 'release', 'view', args.tag, '--json', 'assets'], text=True))
            for asset in old['assets']:
                name = asset['name']
                obsolete = name in {'SHA256SUMS', 'BUILD-INFO.json'} or (
                    name.startswith('ahakey-runtime-') and name.endswith(
                        ('.tar.gz', '.zip', '.sha256', '.build-info.json')))
                if obsolete:
                    subprocess.run(['gh', 'release', 'delete-asset', args.tag, name, '--yes'], check=True)
        else:
            command = ['gh', 'release', 'create', args.tag, *map(str, assets), '--verify-tag',
                       '--title', 'AhaKey Runtime ' + args.tag, '--notes-file', str(notes_file)]
            if '-' in args.version.split('+')[0]:
                command += ['--prerelease', '--latest=false']
            subprocess.run(command, check=True)


if __name__ == '__main__':
    main()
