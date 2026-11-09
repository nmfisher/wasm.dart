#!/usr/bin/env python3
"""Append the probe's one-section tina.embedder-abi v1 stamp."""
import sys
from pathlib import Path

NAME = b"tina.embedder-abi"
HEADER = b"\0asm\r\0\x01\0"

def uleb(value: int) -> bytes:
    out = bytearray()
    while value >= 0x80:
        out.append((value & 0x7f) | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)

def read_uleb(data: bytes, pos: int) -> tuple[int, int]:
    value = 0
    for shift in range(0, 35, 7):
        if pos >= len(data):
            raise ValueError("truncated section length")
        byte = data[pos]
        pos += 1
        if shift == 28 and byte > 0x0f:
            raise ValueError("overflowing section length")
        value |= (byte & 0x7f) << shift
        if byte < 0x80:
            return value, pos
    raise ValueError("oversized section length")

def existing_stamp_count(data: bytes) -> int:
    if not data.startswith(HEADER):
        raise ValueError("not a version-1 Wasm component")
    count = 0
    pos = len(HEADER)
    while pos < len(data):
        section_id = data[pos]
        length, body = read_uleb(data, pos + 1)
        end = body + length
        if end > len(data):
            raise ValueError("section exceeds file")
        if section_id == 0:
            name_len, name_pos = read_uleb(data, body)
            name_end = name_pos + name_len
            if name_end > end:
                raise ValueError("custom section name exceeds section")
            count += data[name_pos:name_end] == NAME
        pos = end
    return count

source, destination = map(Path, sys.argv[1:3])
data = source.read_bytes()
if existing_stamp_count(data):
    raise SystemExit("input already has tina.embedder-abi")
payload = uleb(len(NAME)) + NAME + (1).to_bytes(4, "little")
section = b"\0" + uleb(len(payload)) + payload
destination.write_bytes(data + section)
if existing_stamp_count(destination.read_bytes()) != 1:
    raise SystemExit("stamp verification failed")
