import struct
import zlib

import pytest
from setup_base_rom import (
    OS_PI_START_DMA_OFFSET,
    OS_PI_START_DMA_SIZE,
    SHIFT_MARKER,
    SHIFT_MARKER_OFFSET,
    apply_bps,
    to_big_endian,
    unshift,
)


def varint(value: int) -> bytes:
    out = bytearray()
    while True:
        low = value & 0x7F
        value >>= 7
        if value == 0:
            out.append(low | 0x80)
            return bytes(out)
        out.append(low)
        value -= 1


def build_patch(source: bytes, target: bytes, commands: bytes, metadata: bytes = b"") -> bytes:
    body = b"BPS1" + varint(len(source)) + varint(len(target)) + varint(len(metadata)) + metadata
    body += commands + struct.pack("<II", zlib.crc32(source), zlib.crc32(target))
    return body + struct.pack("<I", zlib.crc32(body))


def command(action: int, length: int) -> bytes:
    return varint(((length - 1) << 2) | action)


def test_to_big_endian_handles_every_dump_format() -> None:
    expected = bytes.fromhex("80371240") + b"ABCD"
    assert to_big_endian(expected) == expected
    assert to_big_endian(bytes.fromhex("37804012") + b"BADC") == expected
    assert to_big_endian(bytes.fromhex("40123780") + b"DCBA") == expected


def test_to_big_endian_rejects_unknown_header() -> None:
    with pytest.raises(SystemExit):
        to_big_endian(b"\0\0\0\0")


def test_apply_bps_runs_each_action() -> None:
    source = b"ABCDEFGH"
    target = b"ABxyFGFGFG"
    commands = (
        command(0, 2)  # source read: AB
        + command(1, 2)
        + b"xy"  # target read
        + command(2, 2)
        + varint(5 << 1)  # source copy from offset 5: FG
        + command(3, 4)
        + varint(4 << 1)  # target copy from offset 4, overlapping its own output: FGFG
    )
    assert apply_bps(build_patch(source, target, commands), source) == target


def test_apply_bps_skips_metadata() -> None:
    # Regression: reading the metadata size must not rewind past the size field itself.
    source = b"AB"
    patch = build_patch(source, b"AB", command(0, 2), metadata=b"meta")
    assert apply_bps(patch, source) == b"AB"


def test_apply_bps_rejects_corrupt_patch() -> None:
    patch = bytearray(build_patch(b"AB", b"AB", command(0, 2)))
    patch[5] ^= 0xFF
    with pytest.raises(SystemExit, match="corrupt"):
        apply_bps(bytes(patch), b"AB")


def test_apply_bps_rejects_wrong_source() -> None:
    patch = build_patch(b"AB", b"AB", command(0, 2))
    with pytest.raises(SystemExit, match="expects"):
        apply_bps(patch, b"XY")


def test_unshift_removes_marker_and_restores_dma_code() -> None:
    size = SHIFT_MARKER_OFFSET + 0x100
    vanilla = bytes(i & 0xFF for i in range(size))
    start = OS_PI_START_DMA_OFFSET
    end = start + OS_PI_START_DMA_SIZE
    # Build the shifted form the way `make release` does.
    patched_dma = bytearray(vanilla)
    patched_dma[start:end] = b"\xee" * OS_PI_START_DMA_SIZE
    shifted = (
        bytes(patched_dma[:SHIFT_MARKER_OFFSET])
        + SHIFT_MARKER
        + bytes(patched_dma[SHIFT_MARKER_OFFSET : -len(SHIFT_MARKER)])
    )

    result = unshift(shifted, vanilla)

    assert len(result) == len(shifted)
    expected_body = vanilla[: size - len(SHIFT_MARKER)]
    assert result[: len(expected_body)] == expected_body
    assert result[len(expected_body) :] == bytes(len(SHIFT_MARKER))


def test_unshift_rejects_rom_without_marker() -> None:
    with pytest.raises(SystemExit, match="marker"):
        unshift(bytes(SHIFT_MARKER_OFFSET + 0x100), bytes(SHIFT_MARKER_OFFSET + 0x100))
