"""Verify the shared installer payload and reject corrupt release artifacts."""
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), SCRIPTS / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PACKAGER = load('package-runtime')
PUBLISHER = load('publish-release')
VERIFIER = load('verify-installer')


class ReleasePackages(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)
        self.binaries = self.repo / 'binaries'
        self.binaries.mkdir()
        self.stage = self.repo / 'payload'
        (self.repo / 'LICENSE').write_text('Fixture license\n')
        self.info = {'version': '0.1.0', 'sourceCommit': '1' * 40, 'sourceDirty': False,
                     'rustc': 'fixture compiler', 'targetDirectory': 'not-distributed'}

    def macho(self, name, architecture=0x0100000C):
        data = bytearray(64)
        data[:4] = b'\xcf\xfa\xed\xfe'
        struct.pack_into('<I', data, 4, architecture)
        (self.binaries / name).write_bytes(data)

    def test_macos_payload_keeps_helper_manifest_and_executable_modes(self):
        for name in ['ahakey-runtime-rust', 'ahakey-input-monitor']:
            self.macho(name)
        info = PACKAGER.stage_runtime(self.repo, self.binaries, self.stage, 'aarch64-apple-darwin', 'v0.1.0', self.info)
        self.assertNotIn('targetDirectory', info)
        self.assertEqual({file['name'] for file in info['files']},
                         {'ahakey-runtime-rust', 'ahakey-input-monitor', 'LICENSE', 'README.txt'})
        VERIFIER.verify_payload(self.stage, self.stage, info)
        (self.stage / 'ahakey-input-monitor').write_bytes(b'corrupt helper')
        with self.assertRaisesRegex(ValueError, 'Installed content mismatch'):
            VERIFIER.verify_payload(self.stage, self.stage, info)

    def test_windows_payload_checks_pe_architecture(self):
        data = bytearray(256)
        data[:2] = b'MZ'
        struct.pack_into('<I', data, 60, 128)
        data[128:132] = b'PE\0\0'
        struct.pack_into('<H', data, 132, 0x8664)
        binary = self.binaries / 'ahakey-runtime-rust.exe'
        binary.write_bytes(data)
        info = PACKAGER.stage_runtime(self.repo, self.binaries, self.stage, 'x86_64-pc-windows-msvc', 'v0.1.0', self.info)
        VERIFIER.verify_payload(self.stage, self.stage, info)
        struct.pack_into('<H', data, 132, 0x14C)
        binary.write_bytes(data)
        with self.assertRaisesRegex(ValueError, 'not a x86_64-pc-windows-msvc'):
            PACKAGER.check_architecture(binary, 'x86_64-pc-windows-msvc')

    def test_wrong_helper_and_unsafe_label_block_packaging(self):
        self.macho('ahakey-runtime-rust')
        self.macho('ahakey-input-monitor', 0x01000007)
        with self.assertRaisesRegex(ValueError, 'not a aarch64-apple-darwin'):
            PACKAGER.stage_runtime(self.repo, self.binaries, self.stage, 'aarch64-apple-darwin', 'v0.1.0', self.info)
        self.assertFalse(self.stage.exists())
        with self.assertRaises(ValueError):
            PACKAGER.package_runtime(self.repo, self.binaries, self.repo / 'dist',
                                     'aarch64-apple-darwin', '../outside', self.info)

    def test_release_gate_rejects_changed_installer_and_wrong_source(self):
        dist = self.repo / 'dist'
        dist.mkdir()
        for target in PUBLISHER.TARGETS:
            name = f'ahakey-runtime-v0.1.0-{target}{PUBLISHER.installer_suffix(target)}'
            installer = dist / name
            installer.write_bytes(b'Fixture native container')
            digest = hashlib.sha256(installer.read_bytes()).hexdigest()
            Path(str(installer) + '.sha256').write_text(f'{digest}  {name}\n')
            info = dict(self.info, target=target, label='v0.1.0', installer={
                'name': name, 'sha256': digest, 'bytes': installer.stat().st_size})
            Path(str(installer) + '.build-info.json').write_text(json.dumps(info))
        PUBLISHER.verify_packages(dist, 'v0.1.0', '0.1.0', '1' * 40)
        with self.assertRaisesRegex(ValueError, 'checksum/source mismatch'):
            PUBLISHER.verify_packages(dist, 'v0.1.0', '0.1.0', '2' * 40)
        installer.write_bytes(b'Corrupt download')
        with self.assertRaisesRegex(ValueError, 'checksum/source mismatch'):
            PUBLISHER.verify_packages(dist, 'v0.1.0', '0.1.0', '1' * 40)


if __name__ == '__main__':
    unittest.main()
