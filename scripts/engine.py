#!/usr/bin/env python3
"""Fetch Troll Engine, which this repository builds with but does not contain.

Usage: engine.py setup    fetch the pinned upstream commit into TrollEngine/

Only TrollEngine/Multiplayer/ is this project's own. Everything else under TrollEngine/ comes
from upstream at the commit below, unchanged: the multiplayer build uses its compiler, its
assembler and its headers, and builds none of the engine itself (Multiplayer/Makefile).
"""

import shutil
import subprocess
import sys
from pathlib import Path

UPSTREAM = "https://github.com/Chlorobite/B3313tools-TrollEngine.git"
PINNED_COMMIT = "a01d6b0695e8566673b417d52b6f8316d7f3a9ce"

ROOT = Path(__file__).resolve().parent.parent
ENGINE = "TrollEngine"
# What tells a tree that is already set up from one holding only this project's files.
SETUP_MARKER = Path(ENGINE) / "Makefile"


def git(*arguments: str, cwd: Path) -> bytes:
    """Run git and return its output as bytes: some engine files have CRLF lines, which a
    patch must keep, and reading the output as text would turn them into LF."""
    result = subprocess.run(("git", *arguments), cwd=cwd, check=False, capture_output=True)
    if result.returncode:
        message = result.stderr.decode("utf-8", errors="replace").strip()
        raise SystemExit(f"git {' '.join(arguments)} failed:\n{message}")
    return result.stdout


def fetch_upstream(clone: Path) -> None:
    """Make `clone` a checkout of the pinned commit, byte for byte as upstream stores it."""
    if not (clone / ".git").is_dir():
        clone.mkdir(parents=True, exist_ok=True)
        git("clone", "--quiet", "--no-checkout", UPSTREAM, ".", cwd=clone)
        # Upstream's bundled tools ask for converted line endings, which IDO and armips reject
        # on Windows. This file outranks theirs.
        (clone / ".git" / "info" / "attributes").write_bytes(b"* -text\n")
    git("checkout", "--quiet", "--force", "--detach", PINNED_COMMIT, cwd=clone)


def setup(root: Path) -> None:
    if (root / SETUP_MARKER).exists():
        raise SystemExit(
            f"{root / ENGINE} already holds the engine. To fetch it again, delete everything "
            "in it except Multiplayer/ first."
        )
    clone = root / ".engine"
    fetch_upstream(clone)
    shutil.copytree(clone / ENGINE, root / ENGINE, dirs_exist_ok=True)
    print(f"Troll Engine {PINNED_COMMIT[:7]} is in {root / ENGINE}.")


def main() -> None:
    if len(sys.argv) not in (2, 3) or sys.argv[1] != "setup":
        raise SystemExit(__doc__)
    # A second argument names another folder to work in, to check a fresh setup against this one.
    setup(Path(sys.argv[2]).resolve() if len(sys.argv) == 3 else ROOT)


if __name__ == "__main__":
    main()
