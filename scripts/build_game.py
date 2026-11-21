#!/usr/bin/env python3
"""Build the multiplayer ROM of each version of B3313.

Usage: build_game.py [version]...    (default: every version in game_versions.py)

Adds the multiplayer code to Bee/<version>.z64, which setup_base_rom.py makes, and writes
Bee/<version>-multiplayer.z64 and its .sym. The build is TrollEngine/Multiplayer/Makefile, which
needs Linux: on Windows this runs it through WSL, in the distribution B3313_WSL_DISTRO names
or the default one.
"""

import os
import subprocess
import sys
from pathlib import Path

from game_versions import VERSIONS, GameVersion, by_key
from setup_base_rom import require_sha256

ENGINE = Path(__file__).resolve().parent.parent / "TrollEngine"


def make_command(version: GameVersion) -> list[str]:
    make = [
        "make",
        "-f",
        "Multiplayer/Makefile",
        f"VERSION={version.key}",
        f"BASE=../Bee/{version.base_path.name}",
        f"OUT=../Bee/{version.multiplayer_path.name}",
    ]
    if sys.platform != "win32":
        return make
    distro = os.environ.get("B3313_WSL_DISTRO")
    chosen = ["-d", distro] if distro else []
    # --exec, so that no shell between here and make reads the arguments again.
    return ["wsl.exe", *chosen, "--cd", str(ENGINE), "--exec", *make]


def build(version: GameVersion) -> None:
    if not version.base_path.exists():
        raise SystemExit(f"{version.base_path} is missing; make it with setup_base_rom.py")
    require_sha256(
        version.base_path.read_bytes(), version.base_sha256, f"The ROM of {version.name}"
    )
    print(f"== {version.name}", flush=True)
    if subprocess.run(make_command(version), cwd=ENGINE, check=False).returncode:
        raise SystemExit(f"The build of {version.name} failed")


def main() -> None:
    versions = [by_key(key) for key in sys.argv[1:]] or list(VERSIONS)
    for version in versions:
        build(version)


if __name__ == "__main__":
    main()
