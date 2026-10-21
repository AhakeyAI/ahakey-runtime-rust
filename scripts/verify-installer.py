#!/usr/bin/env python3
"""Install or mount a native package, verify its payload, and start a mock Runtime."""
import argparse
import filecmp
import json
import os
from pathlib import Path
import plistlib
import shutil
import socket
import subprocess
import tempfile
import time
from urllib.parse import urlparse


def verify_payload(binary_dir, resources, target, reference):
    runtime = 'ahakey-runtime-rust' + ('.exe' if 'windows' in target else '')
    binaries = [runtime] + (['ahakey-input-monitor'] if 'apple-darwin' in target else [])
    for name in binaries:
        path = binary_dir / name
        if not filecmp.cmp(path, reference / name, shallow=False):
            raise ValueError(f'Installed content mismatch: {path}')
        if os.name != 'nt' and not path.stat().st_mode & 0o111:
            raise ValueError(f'Executable bit missing: {path}')
    for name in ['LICENSE', 'README.txt']:
        if not (resources / name).is_file():
            raise ValueError(f'Installed documentation missing: {name}')
    for directory in {binary_dir, resources}:
        if any(path.name.lower().endswith(('.build-info.json', '.sha256'))
               or path.name.lower() in {'build-info.json', 'sha256sums'}
               for path in directory.iterdir()):
            raise ValueError(f'Unexpected build metadata in installer: {directory}')


def smoke_runtime(binary, root):
    result = subprocess.run([str(binary), '--help'], check=True, capture_output=True, text=True, timeout=30)
    if 'AhaKey Runtime' not in result.stdout:
        raise ValueError('Installed binary did not print Runtime help')
    private = root / 'private'
    private.mkdir(mode=0o700)
    discovery = private / 'discovery.json'
    with (root / 'runtime.log').open('w+') as log:
        process = subprocess.Popen([str(binary), '--mock', '--parent-stdin', '--discovery', str(discovery)],
                                   stdin=subprocess.PIPE, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 30
            while not discovery.exists():
                if process.poll() is not None or time.monotonic() > deadline:
                    log.seek(0)
                    raise ValueError('Installed Runtime did not start: ' + log.read())
                time.sleep(0.1)
            info = json.loads(discovery.read_text())
            endpoint = urlparse(info['endpoint'])
            if endpoint.scheme != 'ws' or endpoint.hostname != '127.0.0.1' or not info['token']:
                raise ValueError('Installed Runtime produced invalid discovery')
            with socket.create_connection((endpoint.hostname, endpoint.port), timeout=5):
                pass
            process.stdin.close()
            if process.wait(timeout=15) != 0 or discovery.exists():
                raise ValueError('Installed Runtime failed clean parent-stdin shutdown')
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


def verify_installer(installer, target, reference):
    with tempfile.TemporaryDirectory(prefix='ahakey-install-check-') as temporary:
        root = Path(temporary)
        if target.endswith('apple-darwin'):
            mount = root / 'mounted'
            subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount),
                            str(installer)], check=True)
            try:
                # Exercise the copy-to-Applications layout without modifying /Applications.
                app = root / 'AhaKey Runtime.app'
                shutil.copytree(mount / app.name, app)
                bundle = app / 'Contents'
                info = plistlib.loads((bundle / 'Info.plist').read_bytes())
                if info['CFBundleExecutable'] != 'ahakey-runtime-rust' or not info['LSUIElement']:
                    raise ValueError('Invalid application entry point')
                binaries, resources = bundle / 'MacOS', bundle / 'Resources'
                verify_payload(binaries, resources, target, reference)
                smoke_runtime(binaries / 'ahakey-runtime-rust', root)
            finally:
                subprocess.run(['hdiutil', 'detach', str(mount)], check=True)
        elif target.endswith('windows-msvc'):
            installed = root / 'installed'
            installed.mkdir()
            # A previous installer left this file in the same application directory.
            (installed / 'BUILD-INFO.json').write_text('{}')
            subprocess.run([str(installer), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-',
                            '/NOICONS', f'/DIR={installed}', f'/LOG={root / "install.log"}'], check=True, timeout=120)
            try:
                verify_payload(installed, installed, target, reference)
                smoke_runtime(installed / 'ahakey-runtime-rust.exe', root)
            finally:
                uninstaller = installed / 'unins000.exe'
                if uninstaller.exists():
                    subprocess.run([str(uninstaller), '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'],
                                   check=True, timeout=120)
                    deadline = time.monotonic() + 15
                    while installed.exists() and time.monotonic() < deadline:
                        time.sleep(0.2)
                    if (installed / 'ahakey-runtime-rust.exe').exists():
                        raise ValueError('Uninstall did not remove Runtime')
        else:
            extracted = root / 'installed'
            subprocess.run(['dpkg-deb', '--extract', str(installer), str(extracted)], check=True)
            location = extracted / 'usr/lib/ahakey-runtime'
            verify_payload(location, location, target, reference)
            command = extracted / 'usr/bin/ahakey-runtime'
            if command.resolve() != location / 'ahakey-runtime-rust':
                raise ValueError('Debian CLI link does not point to Runtime')
            smoke_runtime(command, root)
    print(f'Verified installer, payload, mock startup and shutdown: {installer.name}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--installer', type=Path, required=True)
    parser.add_argument('--binary-dir', type=Path, required=True, help='Reference binaries from the build')
    args = parser.parse_args()
    verify_installer(args.installer.resolve(), args.target, args.binary_dir.resolve())
