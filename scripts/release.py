#!/usr/bin/env python3
"""Build the download players get: one zip with the panel, the server and the licenses.

Usage: release.py
       release.py server-licenses <file>

Builds both workspaces in release mode, in C:/b3313-release (see RELEASE_DIR), and writes,
under dist/:

    B3313-Extended-Multiplayer-<version>-windows-x64.zip
    B3313-Extended-Multiplayer-<version>-windows-x64.zip.sha256

The version is the panel's, in panel/Cargo.toml. The zip holds no ROM: the panel carries only
this project's own patches (run make_patch.py first if a ROM changed) and builds the game on
the player's PC.

The second form writes the license texts of the crates the server alone is built from, for
the Linux server's download, which GitHub builds (.github/workflows/release.yml).
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tomllib
import zipfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"
_PLATFORM = "x86_64-pc-windows-msvc"
_SERVER_PLATFORM = "x86_64-unknown-linux-gnu"
# The programs released from each workspace: a release answers for the crates in those.
_TOOLS = ("b3313-server", "b3313-bridge")
_PANEL = ("b3313-panel",)
_METADATA = ("cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform")
_LICENSE_FILES = ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "UNLICENSE*")


# Where a release is built, with everything the build touches: a folder outside the builder's
# home, whose path, which begins with its owner's name, would otherwise be in what is
# released. A program records where its sources were, and the linker writes into the panel's
# .pdb the folder it ran in, where its object files and the toolchain's libraries were, where
# the crates with C in them were compiled from, and its temporary file. So the release folder
# has its own copy of the sources (src), build output (target), toolchain (rustup), crates
# (cargo) and temporary files (tmp). B3313_RELEASE_DIR names another folder.
RELEASE_DIR = Path(os.environ.get("B3313_RELEASE_DIR", "C:/b3313-release"))
SOURCES = RELEASE_DIR / "src"


def _target_dir(workspace: Path) -> Path:
    """Where a workspace's release is built: apart from everyday builds, and one for each
    workspace, which are built with different dependencies."""
    return RELEASE_DIR / "target" / workspace.name


def _build_environment() -> dict[str, str]:
    """The environment a release is built in: every folder the build uses is the release
    folder's own. rustup fetches the toolchain there, and cargo the crates, the first time.

    The copy of the sources is no git checkout, so the panel's build is told the commit its
    crash reports name."""
    temporary = RELEASE_DIR / "tmp"
    temporary.mkdir(parents=True, exist_ok=True)
    commit = subprocess.run(
        ("git", "rev-parse", "--short=10", "HEAD"),
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        encoding="utf-8",
    ).stdout.strip()
    return {
        **os.environ,
        "B3313_COMMIT": commit,
        "RUSTUP_HOME": str(RELEASE_DIR / "rustup"),
        "CARGO_HOME": str(RELEASE_DIR / "cargo"),
        "TMP": str(temporary),
        "TEMP": str(temporary),
    }


def _copy_sources() -> None:
    """Puts the files git tracks, as they are committed or staged, into the release folder:
    a release is built from those, and not from whatever else is in the working folder."""
    if SOURCES.exists():
        shutil.rmtree(SOURCES)
    export = ("git", "checkout-index", "--all", f"--prefix={SOURCES.as_posix()}/")
    subprocess.run(export, cwd=ROOT, check=True)


def names_home(file: Path) -> bool:
    """Whether `file` holds the path of the builder's home folder, in either way of writing
    a path and in either of the encodings a program keeps text in."""
    home = str(Path.home())
    # Lowered as bytes on both sides, which changes only plain letters: lowering the name as
    # text would change an accented capital too, which the file's bytes still have.
    contents = file.read_bytes().lower()
    spellings = (home, home.replace("\\", "/"))
    return any(
        spelling.encode(encoding).lower() in contents
        for spelling in spellings
        for encoding in ("utf-8", "utf-16-le")
    )


def linked_crates(metadata: dict[str, Any], programs: tuple[str, ...]) -> set[str]:
    """The ids of the packages `programs` are built from, in cargo's metadata: everything
    they depend on and what builds that, but nothing only tests use."""
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    names = {package["id"]: package["name"] for package in metadata["packages"]}
    pending = [id_ for id_ in nodes if names[id_] in programs]
    missing = set(programs) - {names[id_] for id_ in pending}
    if missing:
        raise SystemExit(f"No such program in the workspace: {sorted(missing)}")
    found: set[str] = set()
    while pending:
        id_ = pending.pop()
        if id_ in found:
            continue
        found.add(id_)
        pending += [
            dependency["pkg"]
            for dependency in nodes[id_]["deps"]
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"])
        ]
    return found


def license_texts(folder: Path) -> list[str]:
    """The license texts a crate's package holds. A crate that keeps its licenses in the
    folder above ships links to them, which arrive as files holding only a path: no text."""
    texts: list[str] = []
    for pattern in _LICENSE_FILES:
        for file in sorted(folder.glob(pattern)):
            if file.is_file():
                text = file.read_text(encoding="utf-8", errors="replace").strip()
                is_link = text.startswith("../") and "\n" not in text
                if text and not is_link:
                    texts.append(text)
    return texts


def third_party_licenses(
    workspaces: tuple[tuple[Path, tuple[str, ...]], ...],
    platform: str,
    environment: dict[str, str] | None = None,
) -> str:
    """Every crate the named programs of each workspace are built from, for `platform`, with
    the license texts its authors ship.

    A text shared by many crates is printed once, under all of their names.
    """
    crates: dict[str, tuple[str, Path]] = {}
    for workspace, programs in workspaces:
        metadata = json.loads(
            subprocess.run(
                (*_METADATA, platform),
                cwd=workspace,
                check=True,
                stdout=subprocess.PIPE,
                encoding="utf-8",
                env=environment,
            ).stdout
        )
        linked = linked_crates(metadata, programs)
        for package in metadata["packages"]:
            # Crates from a registry; this project's own are under LICENSE.txt.
            if package["id"] in linked and package["source"]:
                name = f"{package['name']} {package['version']}"
                crates[name] = (
                    package.get("license") or "see its files",
                    Path(package["manifest_path"]).parent,
                )

    users: dict[str, list[str]] = {}
    without_text: list[str] = []
    for name, (_, folder) in sorted(crates.items()):
        texts = license_texts(folder)
        if not texts:
            without_text.append(name)
        for text in texts:
            users.setdefault(text, []).append(name)

    lines = [
        "Third-party software in B3313 Extended Multiplayer",
        "",
        "The programs are built from the Rust crates below, each under the license named.",
        "The license texts their authors ship follow the list.",
        "",
    ]
    lines += [f"{name}: {license_}" for name, (license_, _) in sorted(crates.items())]
    if without_text:
        lines += [
            "",
            "The packages of these crates hold no license text of their own. Each is under",
            "the license named for it above, whose text is at https://spdx.org/licenses/:",
            "",
            ", ".join(without_text),
        ]
    for text, names in users.items():
        lines += ["", "=" * 78, "Applies to: " + ", ".join(names), "=" * 78, "", text]
    return "\n".join(lines) + "\n"


def _write_text(file: Path, text: str) -> None:
    file.write_text(text, encoding="utf-8", newline="\n")


def build_zip() -> None:
    environment = _build_environment()
    _copy_sources()
    panel = SOURCES / "panel"
    with (panel / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["package"]["version"]
    name = f"B3313-Extended-Multiplayer-{version}-windows-x64"

    # The toolchain rust-toolchain.toml names, into the release folder if it is not there.
    # Nothing else: left to itself, rustup would update itself too, for the whole PC.
    install = ("rustup", "toolchain", "install", "--no-self-update")
    subprocess.run(install, cwd=SOURCES, check=True, env=environment)
    built = (SOURCES, panel)
    for workspace in built:
        # --locked: a release is of the crates the lock file names, which were checked.
        target = str(_target_dir(workspace))
        build = ("cargo", "build", "--release", "--locked", "--target-dir", target)
        subprocess.run(build, cwd=workspace, check=True, env=environment)

    folder = DIST / name
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    tools, app = (_target_dir(workspace) / "release" for workspace in built)
    # Everything is from the release folder's copy of the sources, as the programs are.
    files = {
        # The crash reporter reads the .pdb by the name the program was built with.
        app / "b3313-panel.exe": "B3313 Extended Multiplayer.exe",
        app / "b3313_panel.pdb": "b3313_panel.pdb",
        tools / "b3313-server.exe": "b3313-server.exe",
        tools / "b3313-bridge.exe": "b3313-bridge.exe",
        SOURCES / "server" / "server.example.conf": "server.example.conf",
        SOURCES / "README.md": "README.md",
        SOURCES / "HOSTING.md": "HOSTING.md",
        SOURCES / "CHANGELOG.md": "CHANGELOG.md",
        SOURCES / "LICENSE.txt": "LICENSE.txt",
        SOURCES / "NOTICE.md": "NOTICE.md",
        panel / "assets" / "icons" / "LICENSE": "LUCIDE-LICENSE.txt",
    }
    for source, target in files.items():
        shutil.copy(source, folder / target)
    licenses = third_party_licenses(((SOURCES, _TOOLS), (panel, _PANEL)), _PLATFORM, environment)
    _write_text(folder / "THIRD-PARTY-LICENSES.txt", licenses)
    named = [file.name for file in sorted(folder.iterdir()) if names_home(file)]
    if named:
        raise SystemExit(f"Not released: the builder's home folder is named in {named}")

    archive = DIST / f"{name}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zipped:
        for file in sorted(folder.iterdir()):
            zipped.write(file, f"{name}/{file.name}")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    _write_text(archive.with_name(archive.name + ".sha256"), f"{digest}  {archive.name}\n")
    print(f"{archive} ({archive.stat().st_size / 1e6:.1f} MB)\nSHA-256 {digest}")


def main() -> None:
    if len(sys.argv) == 3 and sys.argv[1] == "server-licenses":
        licenses = third_party_licenses(((ROOT, ("b3313-server",)),), _SERVER_PLATFORM)
        _write_text(Path(sys.argv[2]), licenses)
    elif len(sys.argv) == 1:
        build_zip()
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
