#!/usr/bin/env python3
"""Build the unmodified Studio dev IPC client and exercise a private Rust simulator."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--studio', type=Path, default=root.parent / 'AhaKey Studio')
args = parser.parse_args()
subprocess.run(['cargo', 'build', '--locked'], cwd=root, check=True)
with tempfile.TemporaryDirectory(prefix='ahakey-swift-interop-') as temp:
    directory = Path(temp)
    sources = []
    for name in ['RuntimeDTOs.swift', 'RuntimeClient.swift', 'IPCRuntimeClient.swift']:
        path = directory / name
        path.write_bytes(subprocess.check_output(['git', 'show', f'dev:StudioFrontend/Runtime/{name}'], cwd=args.studio))
        sources.append(str(path))
    client = directory / 'studio-smoke'
    subprocess.run(['swiftc', '-swift-version', '5', '-o', str(client), *sources, str(root / 'scripts/StudioSmoke.swift')], check=True)
    discovery = directory / 'discovery.json'
    with (directory / 'runtime.log').open('w+') as log:
        runtime = subprocess.Popen([str(root / 'target/debug/ahakey-runtime-rust'), '--mock', '--discovery', str(discovery)], stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 10
            while not discovery.exists():
                if runtime.poll() is not None or time.monotonic() > deadline:
                    log.seek(0)
                    raise RuntimeError(log.read())
                time.sleep(0.05)
            # Registration follows publishing; allow the simulator's first tick to finish.
            time.sleep(0.1)
            subprocess.run([str(client)], env={**os.environ, 'AHAKEY_RUNTIME_DISCOVERY': str(discovery)}, check=True, timeout=30)
        finally:
            runtime.terminate()
            try:
                runtime.wait(timeout=35)
            except subprocess.TimeoutExpired:
                runtime.kill()
                runtime.wait()
