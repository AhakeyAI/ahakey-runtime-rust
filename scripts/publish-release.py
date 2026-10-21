#!/usr/bin/env python3
"""Verify native-package manifests and publish a release with curated notes."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

from runtime_installers import installer_suffix

TARGETS = ['aarch64-apple-darwin', 'x86_64-apple-darwin',
           'x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu']


def verify_packages(directory, tag, version, commit):
    sums, assets = [], []
    for target in TARGETS:
        installer = directory / f'ahakey-runtime-{tag}-{target}{installer_suffix(target)}'
        digest = hashlib.sha256(installer.read_bytes()).hexdigest()
        line = f'{digest}  {installer.name}\n'
        checksum = Path(str(installer) + '.sha256')
        metadata = Path(str(installer) + '.build-info.json')
        info = json.loads(metadata.read_text(encoding='utf-8'))
        if (checksum.read_text() != line or info['sourceDirty'] or info['label'] != tag
                or info['target'] != target or info['version'] != version or info['sourceCommit'] != commit
                or info['installer'] != {'name': installer.name, 'sha256': digest, 'bytes': installer.stat().st_size}):
            raise ValueError(f'Installer checksum/source mismatch: {installer}')
        sums.append(line)
        assets.extend([installer, checksum, metadata])
    aggregate = directory / 'SHA256SUMS'
    aggregate.write_text(''.join(sums), encoding='utf-8')
    return assets + [aggregate]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--commit', required=True)
    parser.add_argument('--dist', type=Path, default=Path('dist'))
    parser.add_argument('--notes', type=Path, required=True)
    parser.add_argument('--update-existing', action='store_true')
    args = parser.parse_args()
    assets = verify_packages(args.dist, args.tag, args.version, args.commit)
    notes = args.notes.read_text(encoding='utf-8')
    if len(notes.strip()) < 200:
        raise ValueError('Release notes must describe the version, installation and limitations')
    if args.update_existing:
        # Repackage the same tagged source. The tag is never moved or recreated.
        subprocess.run(['gh', 'release', 'view', args.tag], check=True, stdout=subprocess.DEVNULL)
        subprocess.run(['gh', 'release', 'upload', args.tag, *map(str, assets), '--clobber'], check=True)
        subprocess.run(['gh', 'release', 'edit', args.tag, '--notes-file', str(args.notes)], check=True)
        old = json.loads(subprocess.check_output(['gh', 'release', 'view', args.tag, '--json', 'assets'], text=True))
        for asset in old['assets']:
            if (asset['name'].startswith('ahakey-runtime-')
                    and asset['name'].endswith(('.tar.gz', '.zip', '.tar.gz.sha256', '.zip.sha256'))):
                subprocess.run(['gh', 'release', 'delete-asset', args.tag, asset['name'], '--yes'], check=True)
    else:
        command = ['gh', 'release', 'create', args.tag, *map(str, assets), '--verify-tag',
                   '--title', 'AhaKey Runtime ' + args.tag, '--notes-file', str(args.notes)]
        if '-' in args.version.split('+')[0]:
            command += ['--prerelease', '--latest=false']
        subprocess.run(command, check=True)


if __name__ == '__main__':
    main()
