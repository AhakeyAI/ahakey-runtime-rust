"""Verify distributed archive contents, executable modes and architecture gates."""
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
import zipfile

SPEC = importlib.util.spec_from_file_location('package_runtime', Path(__file__).parents[1] / 'package-runtime.py')
PACKAGER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGER)


class ReleasePackages(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)
        self.binary_dir = self.repo / 'binaries'
        self.binary_dir.mkdir()
        self.output = self.repo / 'dist'
        (self.repo / 'LICENSE').write_text('Fixture license\n')
        self.info = {'version': '0.1.0', 'sourceCommit': '1' * 40, 'sourceDirty': False,
                     'rustc': 'fixture compiler', 'targetDirectory': 'not-distributed'}

    def macho(self, name, architecture=0x0100000C):
        data = bytearray(64)
        data[:4] = b'\xcf\xfa\xed\xfe'
        struct.pack_into('<I', data, 4, architecture)
        (self.binary_dir / name).write_bytes(data)

    def package(self, target, label='v0.1.0'):
        return PACKAGER.package_runtime(self.repo, self.binary_dir, self.output, target, label, self.info)

    def verify(self, archive, read, prefix):
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(Path(str(archive) + '.sha256').read_text(), f'{digest}  {archive.name}\n')
        info = json.loads(read(prefix + '/BUILD-INFO.json'))
        self.assertEqual(info['sourceCommit'], self.info['sourceCommit'])
        self.assertEqual(info['version'], '0.1.0')
        self.assertNotIn('targetDirectory', info)
        for file in info['files']:
            content = read(prefix + '/' + file['name'])
            self.assertEqual(file['sha256'], hashlib.sha256(content).hexdigest())
            self.assertEqual(file['bytes'], len(content))
        return info

    def test_macos_archive_keeps_runtime_helper_and_executable_modes(self):
        for binary in ['ahakey-runtime-rust', 'ahakey-input-monitor']:
            self.macho(binary)
        archive = self.package('aarch64-apple-darwin')
        prefix = archive.name.removesuffix('.tar.gz')
        with tarfile.open(archive) as package:
            def read(name):
                with package.extractfile(name) as file:
                    return file.read()
            info = self.verify(archive, read, prefix)
            self.assertEqual(info['target'], 'aarch64-apple-darwin')
            for binary in ['ahakey-runtime-rust', 'ahakey-input-monitor']:
                self.assertEqual(package.getmember(prefix + '/' + binary).mode, 0o755)
            self.assertEqual({file['name'] for file in info['files']},
                             {'ahakey-runtime-rust', 'ahakey-input-monitor', 'LICENSE', 'README.txt'})

    def test_windows_zip_contains_the_exe_and_verifiable_manifest(self):
        data = bytearray(256)
        data[:2] = b'MZ'
        struct.pack_into('<I', data, 60, 128)
        data[128:132] = b'PE\0\0'
        struct.pack_into('<H', data, 132, 0x8664)
        (self.binary_dir / 'ahakey-runtime-rust.exe').write_bytes(data)
        archive = self.package('x86_64-pc-windows-msvc')
        with zipfile.ZipFile(archive) as package:
            info = self.verify(archive, package.read, archive.stem)
            self.assertEqual(info['target'], 'x86_64-pc-windows-msvc')
            self.assertEqual(package.read(archive.stem + '/ahakey-runtime-rust.exe'), data)
            self.assertNotIn('ahakey-input-monitor', {file['name'] for file in info['files']})

    def test_missing_or_wrong_architecture_helper_blocks_packaging(self):
        self.macho('ahakey-runtime-rust')
        with self.assertRaises(FileNotFoundError):
            self.package('aarch64-apple-darwin')
        self.macho('ahakey-input-monitor', 0x01000007)
        with self.assertRaisesRegex(ValueError, 'not a aarch64-apple-darwin'):
            self.package('aarch64-apple-darwin')
        self.assertFalse(self.output.exists())

    def test_existing_archives_and_unsafe_labels_are_rejected(self):
        self.macho('ahakey-runtime-rust')
        self.macho('ahakey-input-monitor')
        archive = self.package('aarch64-apple-darwin')
        original = archive.read_bytes()
        with self.assertRaises(FileExistsError):
            self.package('aarch64-apple-darwin')
        self.assertEqual(archive.read_bytes(), original)
        with self.assertRaises(ValueError):
            self.package('aarch64-apple-darwin', '../outside')


if __name__ == '__main__':
    unittest.main()
