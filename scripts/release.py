#!/usr/bin/env python3
"""Build the download players get: one zip with the panel, the server and the licenses.

Usage: release.py

Builds both workspaces in release mode, in C:/b3313-release (see RELEASE_DIR), and writes,
under dist/:

    B3313-Extended-Multiplayer-<version>-windows-x64.zip
    B3313-Extended-Multiplayer-<version>-windows-x64.zip.sha256

The version is the panel's, in panel/Cargo.toml. The zip holds no ROM: the panel carries only
this project's own patches (run make_patch.py first if a ROM changed) and builds the game on
the player's PC.
"""

import hashlib
import json
import os
import shutil
import subprocess
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / "dist"
_PLATFORM = "x86_64-pc-windows-msvc"
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
    folder's own. rustup fetches the toolchain there, and cargo the crates, the first time."""
    temporary = RELEASE_DIR / "tmp"
    temporary.mkdir(parents=True, exist_ok=True)
    return {
        **os.environ,
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


def _names_home(file: Path) -> bool:
    """Whether `file` holds the path of the builder's home folder, in either way of writing
    a path and in either of the encodings a program keeps text in."""
    home = str(Path.home()).lower()
    contents = file.read_bytes().lower()
    spellings = (home, home.replace("\\", "/"))
    return any(
        spelling.encode(encoding) in contents
        for spelling in spellings
        for encoding in ("utf-8", "utf-16-le")
    )


def _cargo(*arguments: str, workspace: Path) -> str:
    result = subprocess.run(
        ("cargo", *arguments), cwd=workspace, check=True, stdout=subprocess.PIPE, encoding="utf-8"
    )
    return result.stdout


def third_party_licenses(workspaces: tuple[Path, ...]) -> str:
    """Every crate the programs are built from, with the license texts its authors ship.

    A text shared by many crates is printed once, under all of their names.
    """
    crates: dict[str, tuple[str, Path]] = {}
    for workspace in workspaces:
        metadata = json.loads(
            _cargo(
                "metadata",
                "--format-version",
                "1",
                "--filter-platform",
                _PLATFORM,
                workspace=workspace,
            )
        )
        built = {node["id"] for node in metadata["resolve"]["nodes"]}
        for package in metadata["packages"]:
            # Crates from a registry; this project's own are under LICENSE.txt.
            if package["id"] in built and package["source"]:
                name = f"{package['name']} {package['version']}"
                crates[name] = (
                    package.get("license") or "see its files",
                    Path(package["manifest_path"]).parent,
                )

    users: dict[str, list[str]] = {}
    for name, (_, folder) in sorted(crates.items()):
        for pattern in _LICENSE_FILES:
            for file in sorted(folder.glob(pattern)):
                if file.is_file():
                    text = file.read_text(encoding="utf-8", errors="replace").strip()
                    users.setdefault(text, []).append(name)

    lines = [
        "Third-party software in B3313 Extended Multiplayer",
        "",
        "The programs are built from the Rust crates below, each under the license named.",
        "The license texts their authors ship follow the list.",
        "",
    ]
    lines += [f"{name}: {license_}" for name, (license_, _) in sorted(crates.items())]
    for text, names in users.items():
        lines += ["", "=" * 78, "Applies to: " + ", ".join(names), "=" * 78, "", text]
    return "\n".join(lines) + "\n"


def main() -> None:
    panel = ROOT / "panel"
    with (panel / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["package"]["version"]
    name = f"B3313-Extended-Multiplayer-{version}-windows-x64"

    environment = _build_environment()
    _copy_sources()
    # The toolchain rust-toolchain.toml names, into the release folder if it is not there.
    # Nothing else: left to itself, rustup would update itself too, for the whole PC.
    install = ("rustup", "toolchain", "install", "--no-self-update")
    subprocess.run(install, cwd=SOURCES, check=True, env=environment)
    built = (SOURCES, SOURCES / "panel")
    for workspace in built:
        build = ("cargo", "build", "--release", "--target-dir", str(_target_dir(workspace)))
        subprocess.run(build, cwd=workspace, check=True, env=environment)

    folder = DIST / name
    if folder.exists():
        shutil.rmtree(folder)
    folder.mkdir(parents=True)
    tools, app = (_target_dir(workspace) / "release" for workspace in built)
    files = {
        # The crash reporter reads the .pdb by the name the program was built with.
        app / "b3313-panel.exe": "B3313 Extended Multiplayer.exe",
        app / "b3313_panel.pdb": "b3313_panel.pdb",
        tools / "b3313-server.exe": "b3313-server.exe",
        tools / "b3313-bridge.exe": "b3313-bridge.exe",
        ROOT / "server" / "server.example.conf": "server.example.conf",
        ROOT / "README.md": "README.md",
        ROOT / "CHANGELOG.md": "CHANGELOG.md",
        ROOT / "LICENSE.txt": "LICENSE.txt",
        ROOT / "NOTICE.md": "NOTICE.md",
    }
    for source, target in files.items():
        shutil.copy(source, folder / target)
    (folder / "THIRD-PARTY-LICENSES.txt").write_text(
        third_party_licenses((ROOT, panel)), encoding="utf-8", newline="\n"
    )
    named = [file.name for file in sorted(folder.iterdir()) if _names_home(file)]
    if named:
        raise SystemExit(f"Not released: the builder's home folder is named in {named}")

    archive = DIST / f"{name}.zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zipped:
        for file in sorted(folder.iterdir()):
            zipped.write(file, f"{name}/{file.name}")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(
        f"{digest}  {archive.name}\n", encoding="utf-8", newline="\n"
    )
    print(f"{archive} ({archive.stat().st_size / 1e6:.1f} MB)\nSHA-256 {digest}")


if __name__ == "__main__":
    main()
