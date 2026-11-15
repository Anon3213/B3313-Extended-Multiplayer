#!/usr/bin/env python3
"""Build the unshifted A2AE base ROM that Troll Engine patches.

Usage: setup_base_rom.py <A2AE .bps> <vanilla SM64 US ROM>

Writes two files to Bee/: a clean copy, and "b3313 silved.z64", the input that
`make` overwrites. Re-running resets that input to the clean base.
"""

import hashlib
import struct
import sys
import zlib
from pathlib import Path

PATCHED_SHA256 = "def0f5dd465b3a3bac6961cdfbccdb2f994b9de423c247802f9ad27695dd2176"
UNSHIFTED_SHA256 = "f8c482f5086349c5dc9054958b97f8ad8bb4d34323f8d2725858d33452898412"

# `make release` inserts this marker, then patches osPiStartDma to add 0x10 to cart addresses.
SHIFT_MARKER = b"YAHAAAAAAAAAAAAA"
SHIFT_MARKER_OFFSET = 0xF5580
OS_PI_START_DMA_OFFSET = 0x80324460 - 0x80245000
OS_PI_START_DMA_SIZE = 0x80324570 - 0x80324460

BEE_DIR = Path(__file__).resolve().parent.parent / "Bee"


def to_big_endian(rom: bytes) -> bytes:
    """Normalize a vanilla dump to .z64 byte order, whatever its file extension claims."""
    magic = rom[:4]
    if magic == b"\x80\x37\x12\x40":
        return rom
    swapped = bytearray(rom)
    if magic == b"\x37\x80\x40\x12":
        swapped[0::2], swapped[1::2] = rom[1::2], rom[0::2]
    elif magic == b"\x40\x12\x37\x80":
        swapped[0::4], swapped[1::4] = rom[3::4], rom[2::4]
        swapped[2::4], swapped[3::4] = rom[1::4], rom[0::4]
    else:
        raise SystemExit(f"Unrecognized ROM header: {magic.hex()}")
    return bytes(swapped)


def apply_bps(patch: bytes, source: bytes) -> bytes:
    """Return the patched ROM, or exit if any CRC stored in the patch does not match."""
    if patch[:4] != b"BPS1":
        raise SystemExit("Not a BPS1 patch")
    source_crc, target_crc, patch_crc = struct.unpack("<III", patch[-12:])
    if zlib.crc32(patch[:-4]) != patch_crc:
        raise SystemExit("Patch file is corrupt")
    if zlib.crc32(source) != source_crc:
        raise SystemExit("Vanilla ROM is not the one this patch expects")

    position = 4

    def read_varint() -> int:
        nonlocal position
        value, shift = 0, 1
        while True:
            byte = patch[position]
            position += 1
            value += (byte & 0x7F) * shift
            if byte & 0x80:
                return value
            shift <<= 7
            value += shift

    def read_signed() -> int:
        raw = read_varint()
        return -(raw >> 1) if raw & 1 else raw >> 1

    read_varint()  # source size; the CRC above already covers it
    target = bytearray(read_varint())
    metadata_size = read_varint()  # read first: read_varint itself advances position
    position += metadata_size

    written = source_cursor = target_cursor = 0
    while position < len(patch) - 12:
        command = read_varint()
        action, length = command & 3, (command >> 2) + 1
        if action == 0:
            target[written : written + length] = source[written : written + length]
        elif action == 1:
            target[written : written + length] = patch[position : position + length]
            position += length
        elif action == 2:
            source_cursor += read_signed()
            target[written : written + length] = source[source_cursor : source_cursor + length]
            source_cursor += length
        else:
            target_cursor += read_signed()
            # Byte by byte: target copies may overlap their own output on purpose.
            for offset in range(length):
                target[written + offset] = target[target_cursor + offset]
            target_cursor += length
        written += length

    if zlib.crc32(target) != target_crc:
        raise SystemExit("Patched output failed its CRC check")
    return bytes(target)


def unshift(shifted: bytes, vanilla: bytes) -> bytes:
    """Undo `make release`'s shift, keeping the ROM size unchanged.

    The header checksum goes stale here; `make` recalculates it on its output.
    """
    marker_end = SHIFT_MARKER_OFFSET + len(SHIFT_MARKER)
    if shifted[SHIFT_MARKER_OFFSET:marker_end] != SHIFT_MARKER:
        raise SystemExit("Shift marker not found; this is not a shifted release ROM")
    rom = bytearray(shifted[:SHIFT_MARKER_OFFSET] + shifted[marker_end:] + bytes(len(SHIFT_MARKER)))
    dma_end = OS_PI_START_DMA_OFFSET + OS_PI_START_DMA_SIZE
    rom[OS_PI_START_DMA_OFFSET:dma_end] = vanilla[OS_PI_START_DMA_OFFSET:dma_end]
    return bytes(rom)


def require_sha256(data: bytes, expected: str, label: str) -> None:
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise SystemExit(f"{label} SHA-256 mismatch: got {actual}, expected {expected}")


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    patch = Path(sys.argv[1]).read_bytes()
    vanilla = to_big_endian(Path(sys.argv[2]).read_bytes())

    patched = apply_bps(patch, vanilla)
    require_sha256(patched, PATCHED_SHA256, "Patched ROM")
    base = unshift(patched, vanilla)
    require_sha256(base, UNSHIFTED_SHA256, "Unshifted ROM")

    (BEE_DIR / "b3313 a2ae-unshifted-clean.z64").write_bytes(base)
    (BEE_DIR / "b3313 silved.z64").write_bytes(base)
    print(f"Wrote the unshifted A2AE base to {BEE_DIR}")


if __name__ == "__main__":
    main()
