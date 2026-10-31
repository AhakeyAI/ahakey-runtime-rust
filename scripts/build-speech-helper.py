"""Build the Runtime-owned macOS speech app with a stable permission identity."""
import argparse
from pathlib import Path
import platform
import shutil
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--architecture', default=platform.machine(), choices=['arm64', 'x86_64'])
parser.add_argument('--identity', default='-')
parser.add_argument('--no-timestamp', action='store_true', help='Local debug builds only; distribution requires a secure timestamp.')
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
bundle = args.output.resolve()
binary = bundle / 'Contents/MacOS/ahakey-speech'
binary.parent.mkdir(parents=True, exist_ok=True)
cache = root / 'target/swift-module-cache'
subprocess.run(['xcrun', 'swiftc', '-O', '-swift-version', '5', '-module-cache-path', str(cache),
                '-target', f'{args.architecture}-apple-macosx13.0',
                str(root / 'native/macos/SpeechHelper.swift'), '-o', str(binary)], check=True)
shutil.copy2(root / 'native/macos/SpeechInfo.plist', bundle / 'Contents/Info.plist')
subprocess.run(['codesign', '--force', '--sign', args.identity, '--options', 'runtime',
                *(['--timestamp=none'] if args.no_timestamp else []),
                '--entitlements', str(root / 'native/macos/Speech.entitlements'), str(bundle)], check=True)
print(bundle)
