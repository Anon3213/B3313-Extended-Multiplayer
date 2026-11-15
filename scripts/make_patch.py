#!/usr/bin/env python3
"""Write the patch the panel carries: what turns the unshifted A2AE base into the multiplayer ROM.

Usage: make_patch.py [multiplayer ROM]    (default: Bee/b3313 multiplayer.z64)

Writes panel/assets/multiplayer.bps and multiplayer.bps.sha256, the SHA-256 of the ROM the patch
makes, which the panel checks after patching. Run it after every multiplayer ROM build that is
to be released, and commit both files: the panel, its bridge and this ROM must stay one version.
"""

import hashlib
import struct
import sys
import zlib
from pathlib import Path

from setup_base_rom import BEE_DIR, UNSHIFTED_SHA256, apply_bps, require_sha256

ASSETS = Path(__file__).resolve().parent.parent / "panel" / "assets"
_SOURCE_READ, _TARGET_READ = 0, 1


def _varint(value: int) -> bytes:
    """A number in BPS's encoding: seven bits a byte, the last byte marked."""
    out = bytearray()
    while True:
        low = value & 0x7F
        value >>= 7
        if value == 0:
            out.append(0x80 | low)
            return bytes(out)
        out.append(low)
        value -= 1


def make_bps(source: bytes, target: bytes) -> bytes:
    """A BPS patch from `source` to `target`, for files that differ in place.

    It only ever copies from the same offset or writes new bytes, which suits a ROM whose
    changes are overwritten code and an appended segment; moved data would be stored again.
    """
    patch = bytearray(b"BPS1" + _varint(len(source)) + _varint(len(target)) + _varint(0))
    shared = min(len(source), len(target))
    position = 0
    while position < len(target):
        same = position < shared and source[position] == target[position]
        end = position + 1
        while end < len(target) and (end < shared and source[end] == target[end]) == same:
            end += 1
        action = _SOURCE_READ if same else _TARGET_READ
        patch += _varint(((end - position - 1) << 2) | action)
        if not same:
            patch += target[position:end]
        position = end
    patch += struct.pack("<II", zlib.crc32(source), zlib.crc32(target))
    patch += struct.pack("<I", zlib.crc32(patch))
    return bytes(patch)


def main() -> None:
    if len(sys.argv) > 2:
        raise SystemExit(__doc__)
    rom = Path(sys.argv[1]) if len(sys.argv) == 2 else BEE_DIR / "b3313 multiplayer.z64"
    base = (BEE_DIR / "b3313 a2ae-unshifted-clean.z64").read_bytes()
    require_sha256(base, UNSHIFTED_SHA256, "Unshifted base ROM")
    target = rom.read_bytes()

    patch = make_bps(base, target)
    if apply_bps(patch, base) != target:
        raise SystemExit("The patch does not reproduce the multiplayer ROM")
    ASSETS.mkdir(exist_ok=True)
    (ASSETS / "multiplayer.bps").write_bytes(patch)
    digest = hashlib.sha256(target).hexdigest()
    (ASSETS / "multiplayer.bps.sha256").write_bytes(f"{digest}\n".encode())
    print(f"Wrote {len(patch)} bytes; the patched ROM's SHA-256 is {digest}")


if __name__ == "__main__":
    main()
