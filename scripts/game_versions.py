"""The versions of B3313 that multiplayer is built for.

A version is one released ROM: two patches that make different ROMs are two versions, whatever
their names say, since players can only share a session if their games are the same. To add
one, give it a row here, read its addresses out of its ROM with find_engine_symbols.py, build
it (build_game.py), make its patch (make_patch.py), and give the panel its row
(panel/src/game_setup.rs) and the bridge and server its name (protocol/src/game.rs); tests keep
those two in step with this file.
"""

from dataclasses import dataclass
from pathlib import Path

BEE_DIR = Path(__file__).resolve().parent.parent / "Bee"


@dataclass(frozen=True)
class GameVersion:
    # Names its files: Bee/<key>.z64, Multiplayer/versions/<key>.asm, panel/assets/<key>.bps.
    key: str
    # What the game tells its bridge it is, and the server keeps sessions apart by. Never give
    # a number that was once another version's to a new one.
    number: int
    name: str
    # The ROM its patch makes of Super Mario 64 (USA).
    released_sha256: str
    # Whether the release moved the ROM's data by 16 bytes, which is undone before building.
    shifted: bool
    # The ROM the multiplayer code is added to: the released one, with any shift undone.
    base_sha256: str

    @property
    def base_path(self) -> Path:
        return BEE_DIR / f"{self.key}.z64"

    @property
    def multiplayer_path(self) -> Path:
        return BEE_DIR / f"{self.key}-multiplayer.z64"


VERSIONS = (
    GameVersion(
        key="a2ae",
        number=1,
        name="Unabandoned A2 Anniversary Edition",
        released_sha256="def0f5dd465b3a3bac6961cdfbccdb2f994b9de423c247802f9ad27695dd2176",
        shifted=True,
        base_sha256="f8c482f5086349c5dc9054958b97f8ad8bb4d34323f8d2725858d33452898412",
    ),
    GameVersion(
        key="v102h3",
        number=2,
        name="1.0.2 hotfix 3",
        released_sha256="2980ddda4248d0276d2a59325e0eceaa2461ad9532f7c92772901cb84ab88f28",
        shifted=False,
        base_sha256="2980ddda4248d0276d2a59325e0eceaa2461ad9532f7c92772901cb84ab88f28",
    ),
    # The same release with the Unabandoned team's engine changes built over it, which is what
    # some downloads of "1.0.2 hotfix 3" hold.
    GameVersion(
        key="v102h3ua",
        number=3,
        name="1.0.2 hotfix 3 with Unabandoned tweaks",
        released_sha256="ced3fb37cdff3e5dc9606412211db3ac9805631bf8abc5511823fddc215a721a",
        shifted=False,
        base_sha256="ced3fb37cdff3e5dc9606412211db3ac9805631bf8abc5511823fddc215a721a",
    ),
)


def by_key(key: str) -> GameVersion:
    for version in VERSIONS:
        if version.key == key:
            return version
    known = ", ".join(version.key for version in VERSIONS)
    raise SystemExit(f"No version of the game is called {key}; there are {known}")
