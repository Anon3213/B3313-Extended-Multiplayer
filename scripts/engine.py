#!/usr/bin/env python3
"""Fetch Troll Engine, which this repository builds on but does not contain.

Usage: engine.py setup    fetch the pinned upstream commit into TrollEngine/ and hook it
       engine.py patch    rewrite the hooks patch from the engine files as they are now

Only TrollEngine/Multiplayer/ is this project's own. Everything else under TrollEngine/ comes
from upstream at the commit below, plus the few lines that call into the multiplayer code, kept
as `engine-hooks.patch`. After changing an engine file, run `patch` and commit the result.
"""

import shutil
import subprocess
import sys
from pathlib import Path

UPSTREAM = "https://github.com/Chlorobite/B3313tools-TrollEngine.git"
PINNED_COMMIT = "a01d6b0695e8566673b417d52b6f8316d7f3a9ce"

ROOT = Path(__file__).resolve().parent.parent
ENGINE = "TrollEngine"
HOOKS_PATCH = ROOT / ENGINE / "Multiplayer" / "engine-hooks.patch"
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
    # Without this, a Windows git set to convert line endings rewrites each hooked file with CRLF.
    git("-c", "core.autocrlf=false", "apply", "--whitespace=nowarn", str(HOOKS_PATCH), cwd=root)
    print(f"Troll Engine {PINNED_COMMIT[:7]} is in {root / ENGINE}, with the multiplayer hooks.")


def write_patch(root: Path) -> None:
    clone = root / ".engine"
    fetch_upstream(clone)
    # Compares upstream's files with this tree's. Files upstream does not have, such as
    # Multiplayer/ and build output, are not in its index and so never appear.
    # Run from `root`: git reads paths against where it runs, and the clone lies inside the tree.
    diff = git(
        f"--git-dir={clone / '.git'}",
        "--work-tree=.",
        "diff",
        "--no-color",
        "--",
        ENGINE,
        ":(exclude)*.o",
        cwd=root,
    )
    HOOKS_PATCH.write_bytes(diff)
    print(f"Wrote {HOOKS_PATCH.relative_to(ROOT)}: {diff.count(b'diff --git')} files hooked.")


def main() -> None:
    commands = {"setup": setup, "patch": write_patch}
    if len(sys.argv) not in (2, 3) or sys.argv[1] not in commands:
        raise SystemExit(__doc__)
    # A second argument names another folder to work in, to check a fresh setup against this one.
    root = Path(sys.argv[2]).resolve() if len(sys.argv) == 3 else ROOT
    commands[sys.argv[1]](root)


if __name__ == "__main__":
    main()
