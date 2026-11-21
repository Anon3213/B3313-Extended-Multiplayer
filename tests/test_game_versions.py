import re
from pathlib import Path

import pytest
from game_versions import VERSIONS, GameVersion, by_key

_ROOT = Path(__file__).parent.parent
_PROTOCOL = _ROOT / "protocol" / "src" / "game.rs"
_PANEL = _ROOT / "panel" / "src" / "game_setup.rs"
_ASSETS = _ROOT / "panel" / "assets"
_VERSION_FILES = _ROOT / "TrollEngine" / "Multiplayer" / "versions"


def test_no_two_versions_share_a_key_a_number_or_a_rom() -> None:
    for field in ("key", "number", "name", "released_sha256", "base_sha256"):
        values = [getattr(version, field) for version in VERSIONS]
        assert len(set(values)) == len(values), field
    # Zero is what a bridge says of a game it knows nothing of.
    assert all(0 < version.number < 256 for version in VERSIONS)


def test_a_version_is_found_by_its_key_and_an_unknown_one_is_refused() -> None:
    assert by_key("v102h3").name == "1.0.2 hotfix 3"
    with pytest.raises(SystemExit, match="a2ae"):
        by_key("v0.7")


def test_the_bridge_and_server_name_the_same_versions() -> None:
    listed = re.findall(r'^    \((\d+), "([^"]+)"\),$', _PROTOCOL.read_text(), re.MULTILINE)
    assert [(int(number), name) for number, name in listed] == [
        (version.number, version.name) for version in VERSIONS
    ]


def test_the_panel_builds_the_same_versions() -> None:
    games = re.findall(
        r'name: "([^"]+)",.*?released_sha256: "(\w+)",\s+shifted: (true|false),\s+'
        r'base_sha256: "(\w+)",\s+patch: include_bytes!\("\.\./assets/(\w+)\.bps"\),\s+'
        r'multiplayer_sha256: include_str!\("\.\./assets/(\w+)\.bps\.sha256"\),',
        _PANEL.read_text(),
        re.DOTALL,
    )
    assert games == [
        (
            version.name,
            version.released_sha256,
            "true" if version.shifted else "false",
            version.base_sha256,
            version.key,
            version.key,
        )
        for version in VERSIONS
    ]


@pytest.mark.parametrize("game", VERSIONS, ids=[version.key for version in VERSIONS])
def test_each_version_has_its_addresses_and_its_patch(game: GameVersion) -> None:
    labels = (_VERSION_FILES / f"{game.key}.asm").read_text()
    # The game tells its bridge the number the server knows it by.
    assert f".definelabel mp_game_number, {game.number}\n" in labels
    assert (_ASSETS / f"{game.key}.bps").read_bytes().startswith(b"BPS1")
    assert re.fullmatch(r"[0-9a-f]{64}\n", (_ASSETS / f"{game.key}.bps.sha256").read_text())
