import pytest
from make_patch import make_bps
from setup_base_rom import apply_bps

SOURCE = bytes(range(200)) * 3


@pytest.mark.parametrize(
    "target",
    [
        SOURCE,
        SOURCE[:100] + b"changed" + SOURCE[107:],
        SOURCE + b"an appended segment",
        SOURCE[:50],
        b"\xff" + SOURCE[1:-1] + b"\xff",
        b"",
    ],
    ids=["same", "changed inside", "longer", "shorter", "changed at both ends", "empty"],
)
def test_a_patch_turns_the_source_into_the_target(target: bytes) -> None:
    assert apply_bps(make_bps(SOURCE, target), SOURCE) == target


def test_a_patch_stores_only_what_changed() -> None:
    target = SOURCE[:300] + b"\x00" * 8 + SOURCE[308:]
    # The header, two copies, eight new bytes and three checksums: far under the file's size.
    assert len(make_bps(SOURCE, target)) < 40
