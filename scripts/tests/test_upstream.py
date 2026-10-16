import importlib.util
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


upstream = load('upstream-integrity')
pe = load('verify-pe-content')


class ProvenanceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.receipt = self.root / 'receipt.json'
        self.environment = patch.dict(os.environ, GITHUB_SHA='a' * 40)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        for name in upstream.NAMES:
            (self.root / (name + '-x86_64-pc-windows-msvc.exe')).write_bytes(name.encode())
        upstream.create(self.root, self.receipt)

    def test_source_build_and_packaged_payload(self):
        upstream.verify(self.root, self.receipt, True)
        for name in upstream.NAMES:
            (self.root / (name + '-x86_64-pc-windows-msvc.exe')).rename(self.root / (name + '.exe'))
        upstream.verify(self.root, self.receipt)

    def test_modified_binary_rejected(self):
        (self.root / 'whisper-cli-x86_64-pc-windows-msvc.exe').write_bytes(b'modified')
        with self.assertRaises(ValueError): upstream.verify(self.root, self.receipt, True)

    def test_receipts_cannot_broaden_exception_or_change_source(self):
        original = self.receipt.read_text()
        for change in [lambda d: d.update(commit='b' * 40),
                       lambda d: d['source'].update(commit='b' * 40),
                       lambda d: d['artifacts'].update({'another.exe': 'a' * 64})]:
            data = json.loads(original)
            change(data)
            self.receipt.write_text(json.dumps(data))
            with self.assertRaises(ValueError): upstream.verify(self.root, self.receipt, True)

    def test_duplicate_missing_and_symlink_rejected(self):
        target = self.root / 'whisper-cli-x86_64-pc-windows-msvc.exe'
        nested = self.root / 'nested'
        nested.mkdir()
        copy = nested / target.name
        copy.write_bytes(target.read_bytes())
        with self.assertRaises(ValueError): upstream.verify(self.root, self.receipt, True)
        target.unlink()
        copy.unlink()
        with self.assertRaises(ValueError): upstream.verify(self.root, self.receipt, True)
        actual = self.root / 'actual'
        actual.write_bytes(b'whisper-cli')
        target.symlink_to(actual)
        with self.assertRaises(ValueError): upstream.verify(self.root, self.receipt, True)


class SigningContentTests(unittest.TestCase):
    def fixture(self, magic=0x20b):
        data = bytearray(1025)
        data[:2] = b'MZ'
        struct.pack_into('<I', data, 60, 128)
        data[128:132] = b'PE\0\0'
        struct.pack_into('<H', data, 152, magic)
        data[800:804] = b'code'
        return data

    def signed(self, original, magic=0x20b):
        data = bytearray(original)
        data.extend(b'\0' * (-len(data) % 8))
        offset = len(data)
        data.extend(b'fixture certificate bytes')
        security = 152 + (112 if magic == 0x20b else 96) + 32
        struct.pack_into('<II', data, security, offset, len(data) - offset)
        struct.pack_into('<I', data, 152 + 64, 12345)
        return data

    def test_only_authenticode_fields_may_change(self):
        for magic in [0x10b, 0x20b]:
            original = self.fixture(magic)
            self.assertEqual(pe.content(original), pe.content(self.signed(original, magic)))

    def test_code_modification_or_nonzero_padding_rejected(self):
        original = self.fixture()
        for location in [800, 1025]:
            signed = self.signed(original)
            signed[location] = 1
            self.assertNotEqual(pe.content(original), pe.content(signed))

    def test_invalid_or_nonterminal_security_table_rejected(self):
        data = self.signed(self.fixture())
        data.extend(b'extra data')
        with self.assertRaises(ValueError): pe.content(data)
        for offset, size in [(0, 1), (1, 0), (1031, 1), (8, 1048)]:
            data = self.fixture()
            struct.pack_into('<II', data, 152 + 112 + 32, offset, size)
            with self.assertRaises(ValueError): pe.content(data)


if __name__ == '__main__':
    unittest.main()
