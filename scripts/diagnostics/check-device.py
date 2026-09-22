#!/usr/bin/env python3
"""Probe one running Runtime device; --reconnect explicitly cycles its BLE session."""
import argparse
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('device_id')
parser.add_argument('--studio', type=Path, default=root.parent / 'AhaKey Studio')
parser.add_argument('--reconnect', action='store_true', help='Disconnect/reconnect the selected device before checking readiness; invalidates configuration baseline')
parser.add_argument('--latency', action='store_true', help='Measure handshake and cached configuration RPC latency without device writes')
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='ahakey-device-probe-') as temp:
    directory = Path(temp)
    sources = []
    for name in ['RuntimeDTOs.swift', 'RuntimeClient.swift', 'IPCRuntimeClient.swift']:
        path = directory / name
        path.write_bytes(subprocess.check_output(['git', 'show', f'dev:StudioFrontend/Runtime/{name}'], cwd=args.studio))
        sources.append(str(path))
    probe = directory / 'runtime-probe'
    subprocess.run(['swiftc', '-swift-version', '5', '-o', str(probe), *sources, str(root / 'scripts/diagnostics/RuntimeProbe.swift')], check=True)
    command = [str(probe), args.device_id]
    if args.reconnect:
        command.append('--reconnect')
    if args.latency:
        command.append('--latency')
    raise SystemExit(subprocess.run(command, timeout=45).returncode)
