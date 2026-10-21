"""Native container builders. The supplied payload already contains its manifest."""
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile


def installer_suffix(target):
    if target.endswith('apple-darwin'):
        return '.dmg'
    if target.endswith('windows-msvc'):
        return '.exe'
    return '.deb'


def build_installer(payload, output, target, version):
    with tempfile.TemporaryDirectory(prefix='ahakey-installer-') as temporary:
        root = Path(temporary)
        if target.endswith('apple-darwin'):
            image = root / 'image'
            app = image / 'AhaKey Runtime.app' / 'Contents'
            shutil.copytree(payload, app / 'MacOS')
            resources = app / 'Resources'
            resources.mkdir()
            for name in ['LICENSE', 'README.txt', 'BUILD-INFO.json']:
                shutil.move(str(app / 'MacOS' / name), resources / name)
            with (app / 'Info.plist').open('wb') as file:
                plistlib.dump({
                    'CFBundleIdentifier': 'ai.ahakey.runtime',
                    'CFBundleName': 'AhaKey Runtime',
                    'CFBundleDisplayName': 'AhaKey Runtime',
                    'CFBundleExecutable': 'ahakey-runtime-rust',
                    'CFBundlePackageType': 'APPL',
                    'CFBundleShortVersionString': version.split('-')[0].split('+')[0],
                    'CFBundleVersion': version.split('-')[0].split('+')[0],
                    'LSMinimumSystemVersion': '13.0',
                    'LSUIElement': True,
                    'NSBluetoothAlwaysUsageDescription': 'AhaKey Runtime connects to your AhaKey device over Bluetooth.',
                }, file)
            (image / 'Applications').symlink_to('/Applications')
            shutil.copyfile(payload / 'README.txt', image / 'README.txt')
            subprocess.run(['hdiutil', 'create', '-volname', 'AhaKey Runtime', '-srcfolder', str(image),
                            '-format', 'UDZO', str(output)], check=True)
        elif target.endswith('windows-msvc'):
            compiler = shutil.which('ISCC.exe')
            if not compiler:
                for base in [os.environ.get('ProgramFiles(x86)'), os.environ.get('ProgramFiles')]:
                    if base:
                        candidate = Path(base) / 'Inno Setup 6' / 'ISCC.exe'
                        if candidate.is_file():
                            compiler = str(candidate)
                            break
            if not compiler:
                raise FileNotFoundError('Install Inno Setup 6 and put ISCC.exe on PATH')
            script = Path(__file__).parent / 'installers' / 'windows.iss'
            subprocess.run([compiler, '/Qp', f'/DSourceDir={payload}', f'/DOutputDir={output.parent}',
                            f'/DFileName={output.stem}', f'/DAppVersion={version}',
                            str(script)], check=True)
        else:
            package = root / 'deb'
            shutil.copytree(payload, package / 'usr/lib/ahakey-runtime')
            (package / 'usr/bin').mkdir(parents=True)
            (package / 'usr/bin/ahakey-runtime').symlink_to('../lib/ahakey-runtime/ahakey-runtime-rust')
            docs = package / 'usr/share/doc/ahakey-runtime'
            docs.mkdir(parents=True)
            shutil.copyfile(payload / 'LICENSE', docs / 'copyright')
            shutil.copyfile(payload / 'README.txt', docs / 'README')
            # Let Debian's symbol database determine the actual shared-library requirements.
            (root / 'debian').mkdir()
            (root / 'debian/control').write_text('Source: ahakey-runtime\n\nPackage: ahakey-runtime\nArchitecture: amd64\n')
            dependencies = subprocess.check_output(
                ['dpkg-shlibdeps', '-O', str(payload / 'ahakey-runtime-rust')], cwd=root, text=True)
            depends = next(line.split('=', 1)[1] for line in dependencies.splitlines()
                           if line.startswith('shlibs:Depends='))
            control = package / 'DEBIAN'
            control.mkdir()
            deb_version = version.replace('-', '~')
            # Development builds must not replace a stable release at the same version.
            manifest = json.loads((payload / 'BUILD-INFO.json').read_text(encoding='utf-8'))
            if '-dev.' in manifest['label']:
                deb_version += '~dev.' + manifest['sourceCommit'][:12]
            (control / 'control').write_text(
                f'Package: ahakey-runtime\nVersion: {deb_version}\nArchitecture: amd64\n'
                'Maintainer: AhaKey <zhangxinyang@ahakey.cn>\n'
                'Section: utils\nPriority: optional\n'
                f'Depends: {depends}, bluez, dbus\n'
                'Homepage: https://github.com/AhakeyAI/ahakey-runtime-rust\n'
                'Description: AhaKey Bluetooth Runtime and local WebSocket API\n'
                ' Background service for AhaKey X1 devices and application integration.\n', encoding='utf-8')
            for path in package.rglob('*'):
                if not path.is_symlink():
                    path.chmod(0o755 if path.is_dir() or path.name == 'ahakey-runtime-rust' else 0o644)
            subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(package), str(output)], check=True)
