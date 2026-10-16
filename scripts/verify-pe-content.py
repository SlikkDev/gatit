#!/usr/bin/env python3
"""Allow only Authenticode certificate-table and checksum changes in a PE file."""
from pathlib import Path
import struct
import sys


def content(data):
    data = bytearray(data)
    if data[:2] != b'MZ' or len(data) < 64:
        raise ValueError('Not a PE file')
    pe = struct.unpack_from('<I', data, 60)[0]
    if data[pe:pe + 4] != b'PE\0\0':
        raise ValueError('Missing PE header')
    optional = pe + 24
    magic = struct.unpack_from('<H', data, optional)[0]
    directories = {0x10b: 96, 0x20b: 112}.get(magic)
    if directories is None:
        raise ValueError('Unsupported PE header')
    security = optional + directories + 4 * 8
    offset, size = struct.unpack_from('<II', data, security)
    if bool(offset) != bool(size) or (size and (offset % 8 or offset < security + 8 or offset + size != len(data))):
        raise ValueError('Invalid or nonterminal Authenticode certificate table')
    data[optional + 64:optional + 68] = b'\0' * 4
    data[security:security + 8] = b'\0' * 8
    if size:
        del data[offset:]
    # Authenticode aligns its table to an eight-byte boundary with zero padding.
    data.extend(b'\0' * (-len(data) % 8))
    return bytes(data)


if __name__ == '__main__':
    try:
        if content(Path(sys.argv[1]).read_bytes()) != content(Path(sys.argv[2]).read_bytes()):
            raise ValueError('The signed PE content differs from the uploaded build')
    except (ValueError, OSError, struct.error) as error:
        sys.exit(str(error))
