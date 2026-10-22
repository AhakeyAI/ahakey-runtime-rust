"""Verify installer payloads and the automatically selected release history."""
import importlib.util
from pathlib import Path
import struct
import subprocess
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
VERIFIER = load('verify-installer')
NOTES = load('release_notes')


class ReleasePackages(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)
        self.binaries = self.repo / 'binaries'
        self.binaries.mkdir()
        self.stage = self.repo / 'payload'
        (self.repo / 'LICENSE').write_text('Fixture license\n')
        self.info = {'version': '0.1.0', 'sourceCommit': '1' * 40}

    def macho(self, name, architecture=0x0100000C):
        data = bytearray(64)
        data[:4] = b'\xcf\xfa\xed\xfe'
        struct.pack_into('<I', data, 4, architecture)
        (self.binaries / name).write_bytes(data)

    def test_macos_payload_keeps_helper_without_build_metadata(self):
        for name in ['ahakey-runtime-rust', 'ahakey-input-monitor']:
            self.macho(name)
        PACKAGER.stage_runtime(self.repo, self.binaries, self.stage, 'aarch64-apple-darwin', 'v0.1.0', self.info)
        self.assertEqual({file.name for file in self.stage.iterdir()},
                         {'ahakey-runtime-rust', 'ahakey-input-monitor', 'LICENSE', 'README.txt'})
        VERIFIER.verify_payload(self.stage, self.stage, 'aarch64-apple-darwin', self.binaries)
        (self.stage / 'ahakey-input-monitor').write_bytes(b'corrupt helper')
        with self.assertRaisesRegex(ValueError, 'Installed content mismatch'):
            VERIFIER.verify_payload(self.stage, self.stage, 'aarch64-apple-darwin', self.binaries)

    def test_windows_payload_checks_pe_architecture(self):
        data = bytearray(256)
        data[:2] = b'MZ'
        struct.pack_into('<I', data, 60, 128)
        data[128:132] = b'PE\0\0'
        struct.pack_into('<H', data, 132, 0x8664)
        binary = self.binaries / 'ahakey-runtime-rust.exe'
        binary.write_bytes(data)
        PACKAGER.stage_runtime(self.repo, self.binaries, self.stage, 'x86_64-pc-windows-msvc', 'v0.1.0', self.info)
        VERIFIER.verify_payload(self.stage, self.stage, 'x86_64-pc-windows-msvc', self.binaries)
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

    def test_automatic_notes_use_only_the_tagged_versions_commit_range(self):
        def git(*arguments):
            return subprocess.check_output(['git', '-C', str(self.repo), *arguments], text=True).strip()
        git('init', '-q')
        git('config', 'user.name', 'Release fixture')
        git('config', 'user.email', 'fixture@example.invalid')
        git('add', 'LICENSE')
        git('commit', '-qm', 'feat: initial Runtime')
        first = git('rev-parse', 'HEAD')
        git('tag', 'v0.1.0')
        initial = NOTES.generate_notes(self.repo, 'v0.1.0', first, 'example/runtime')
        self.assertIn('feat: initial Runtime', initial)
        self.assertIn('新增功能', initial)
        git('commit', '--allow-empty', '-qm', 'fix(ble): reconnect device')
        second = git('rev-parse', 'HEAD')
        git('tag', 'v0.2.0')
        git('commit', '--allow-empty', '-qm', 'feat: future feature')
        notes = NOTES.generate_notes(self.repo, 'v0.2.0', second, 'example/runtime')
        self.assertIn('fix(ble): reconnect device', notes)
        self.assertIn('问题修复', notes)
        self.assertIn('compare/v0.1.0...v0.2.0', notes)
        self.assertNotIn('initial Runtime', notes)
        self.assertNotIn('future feature', notes)


if __name__ == '__main__':
    unittest.main()
