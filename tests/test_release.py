from pathlib import Path
from typing import Any

import pytest
from release import license_texts, linked_crates


def _package(name: str) -> dict[str, Any]:
    return {"id": name, "name": name}


def _node(name: str, **dependencies: str | None) -> dict[str, Any]:
    """A crate in cargo's metadata, with what it depends on and how: None is an ordinary
    dependency, and "dev" one of its tests only."""
    deps = [{"pkg": pkg, "dep_kinds": [{"kind": kind}]} for pkg, kind in dependencies.items()]
    return {"id": name, "deps": deps}


METADATA: dict[str, Any] = {
    "packages": [_package(name) for name in ("server", "tools", "net", "codec", "mock", "cc")],
    "resolve": {
        "nodes": [
            _node("server", net=None, mock="dev"),
            _node("tools", mock=None),
            _node("net", codec=None, cc="build"),
            _node("codec"),
            _node("mock"),
            _node("cc"),
        ]
    },
}


def test_a_program_answers_for_what_it_is_built_from_and_not_for_its_tests() -> None:
    # "mock" is in the workspace's lock file for the server's tests and for a program that is
    # not released: the server's download does not hold it.
    assert linked_crates(METADATA, ("server",)) == {"server", "net", "codec", "cc"}


def test_a_program_the_workspace_lacks_stops_the_release() -> None:
    with pytest.raises(SystemExit, match="renamed"):
        linked_crates(METADATA, ("server", "renamed"))


def test_a_link_to_a_license_is_not_its_text(tmp_path: Path) -> None:
    (tmp_path / "LICENSE-MIT").write_text("Permission is hereby granted\nfree of charge\n")
    # What a symlink in a crate's package arrives as on Windows.
    (tmp_path / "LICENSE-APACHE").write_text("../LICENSE-APACHE")
    (tmp_path / "README.md").write_text("not a license")
    assert license_texts(tmp_path) == ["Permission is hereby granted\nfree of charge"]
