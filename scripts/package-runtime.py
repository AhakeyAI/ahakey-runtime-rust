#!/usr/bin/env python3
"""Build a native installer for Runtime and its helper; no publication."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
from runtime_installers import build_installer, installer_suffix

TARGETS = {
    'aarch64-apple-darwin': ('macos', 0x0100000C),
    'x86_64-apple-darwin': ('macos', 0x01000007),
    'x86_64-pc-windows-msvc': ('windows', 0x8664),
    'x86_64-unknown-linux-gnu': ('linux', 62),
}


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def check_architecture(path, target):
    system, architecture = TARGETS[target]
    with path.open('rb') as source:
        header = source.read(64)
        if system == 'macos' and header[:4] == b'\xcf\xfa\xed\xfe':
            actual = struct.unpack_from('<I', header, 4)[0]
        elif system == 'linux' and header[:6] == b'\x7fELF\x02\x01':
            actual = struct.unpack_from('<H', header, 18)[0]
        elif system == 'windows' and header[:2] == b'MZ' and len(header) == 64:
            source.seek(struct.unpack_from('<I', header, 60)[0])
            pe = source.read(6)
            actual = struct.unpack_from('<H', pe, 4)[0] if pe[:4] == b'PE\0\0' else None
        else:
            actual = None
    if actual != architecture:
        raise ValueError(f'{path.name} is not a {target} executable')


def source_info(repo):
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version=1'], cwd=repo, text=True))
    package = next(item for item in metadata['packages']
                   if Path(item['manifest_path']).resolve() == repo / 'Cargo.toml')
    return {
        'version': package['version'],
        'sourceCommit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
        'sourceDirty': bool(subprocess.check_output(['git', 'status', '--porcelain'], cwd=repo, text=True).strip()),
        'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
        'targetDirectory': metadata['target_directory'],
    }


def stage_runtime(repo, binary_dir, stage, target, label, info):
    """Create the shared payload used by all native containers."""
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.+_-]*', label):
        raise ValueError('Build label must be a filename-safe version')
    system = TARGETS[target][0]
    runtime = 'ahakey-runtime-rust' + ('.exe' if system == 'windows' else '')
    binaries = [runtime] + (['ahakey-input-monitor'] if system == 'macos' else [])
    for name in binaries:
        check_architecture(binary_dir / name, target)
    stage.mkdir(parents=True)
    for binary in binaries:
        shutil.copyfile(binary_dir / binary, stage / binary)
        (stage / binary).chmod(0o755)
    shutil.copyfile(repo / 'LICENSE', stage / 'LICENSE')
    command = runtime if system == 'windows' else './' + runtime
    platform_notes = {
        'macos': 'macOS 13+. --input-monitor enables the bundled helper and requires Input Monitoring permission.\n'
                 'macOS 13 及以上。--input-monitor 启用附带的输入监听 helper，需要输入监控权限。\n'
                 'No Developer ID signature/notarization. Re-sign both executables when bundling in Studio.\n'
                 '未做 Developer ID 签名或公证；集成 Studio 时对两个程序统一签名。',
        'windows': 'Windows 10+ x64. The installer installs for the current user.\n'
                   'Windows 10 及以上 x64。安装到当前用户目录，从开始菜单启动。',
        'linux': 'Linux x64, glibc 2.35+, libdbus-1.so.3, BlueZ and D-Bus services.\n'
                 'Linux x64，需要 glibc 2.35+、libdbus-1.so.3、BlueZ 和 D-Bus 服务。\n'
                 'On Ubuntu/Debian: sudo apt-get install libdbus-1-3 bluez',
    }
    (stage / 'README.txt').write_text(
        f'AhaKey Runtime {label}\nTarget / 平台: {target}\nCommit / 提交: {info["sourceCommit"]}\n\n'
        f'{platform_notes[system]}\n\n'
        f'Help / 帮助: {command} --help\nSimulator / 模拟器: {command} --mock\n'
        f'Hardware / 连接设备: {command}\n\n'
        'macOS: drag AhaKey Runtime.app to Applications; launch to run in the background.\n'
        'macOS：将 AhaKey Runtime.app 拖入应用程序，打开后在后台运行；活动监视器可退出。\n'
        'macOS CLI: /Applications/"AhaKey Runtime.app"/Contents/MacOS/ahakey-runtime-rust --help\n'
        'Windows CLI: %LOCALAPPDATA%\\Programs\\AhaKey Runtime\\ahakey-runtime-rust.exe --help\n'
        'Linux: sudo apt install ./ahakey-runtime-*.deb; ahakey-runtime --help\n'
        'Runtime is a background service with no settings window. No login autostart is installed.\n'
        'Runtime 是后台服务，没有设置窗口；安装后不会自动开机启动。\n'
        'Startup may request Bluetooth access. BUILD-INFO.json records source and file hashes.\n'
        '首次连接可能需要蓝牙授权；BUILD-INFO.json 记录源码和文件校验值。\n', encoding='utf-8')
    manifest = {key: value for key, value in info.items() if key != 'targetDirectory'}
    manifest.update({
        'label': label, 'target': target,
        'builtAt': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'files': [{'name': path.name, 'sha256': sha256(path), 'bytes': path.stat().st_size}
                  for path in sorted(stage.iterdir())],
    })
    (stage / 'BUILD-INFO.json').write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    return manifest


def package_runtime(repo, binary_dir, output, target, label, info):
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.+_-]*', label):
        raise ValueError('Build label must be a filename-safe version')
    name = f'ahakey-runtime-{label}-{target}'
    output.mkdir(parents=True, exist_ok=True)
    archive = output / (name + installer_suffix(target))
    if any(path.exists() for path in [archive, Path(str(archive) + '.sha256'),
                                      Path(str(archive) + '.build-info.json')]):
        raise FileExistsError(f'Package exists: {archive}; use a new output directory')
    with tempfile.TemporaryDirectory(prefix='ahakey-package-') as temporary:
        stage = Path(temporary) / name
        manifest = stage_runtime(repo, binary_dir, stage, target, label, info)
        build_installer(stage, archive, target, info['version'])
        manifest['installer'] = {'name': archive.name, 'sha256': sha256(archive),
                                 'bytes': archive.stat().st_size}
        Path(str(archive) + '.build-info.json').write_text(
            json.dumps(manifest, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
    Path(str(archive) + '.sha256').write_text(f'{sha256(archive)}  {archive.name}\n', encoding='utf-8')
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True, choices=TARGETS)
    parser.add_argument('--label', required=True)
    parser.add_argument('--output', type=Path, default=Path('dist'))
    parser.add_argument('--require-clean', action='store_true')
    parser.add_argument('--repo', type=Path, help='Runtime source checkout; defaults to this repository')
    parser.add_argument('--packaging-commit', help='Commit containing the packaging tools')
    args = parser.parse_args()
    repo = (args.repo or Path(__file__).resolve().parents[1]).resolve()
    info = source_info(repo)
    if args.packaging_commit:
        info['packagingCommit'] = args.packaging_commit
    if args.require_clean and info['sourceDirty']:
        raise SystemExit('Release packaging requires a clean source checkout')
    binary_dir = Path(info['targetDirectory']) / args.target / 'release'
    archive = package_runtime(repo, binary_dir, args.output.resolve(), args.target, args.label, info)
    print(archive)
    print(str(archive) + '.sha256')


if __name__ == '__main__':
    main()
