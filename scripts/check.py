#!/usr/bin/env python3
"""Run every lint and test baseline, stopping at the first failure.

Usage: uv run --no-sync python scripts/check.py
"""

import subprocess
import sys

_PANEL = ("--manifest-path", "panel/Cargo.toml")

_COMMANDS = (
    ("cargo", "fmt", "--check"),
    ("cargo", "clippy", "--all-targets", "--", "-D", "warnings"),
    ("cargo", "nextest", "run"),
    ("cargo", "deny", "check"),
    # The control panel is a workspace of its own, with its own dependency policy.
    ("cargo", "fmt", "--check", *_PANEL),
    ("cargo", "clippy", "--all-targets", *_PANEL, "--", "-D", "warnings"),
    ("cargo", "nextest", "run", *_PANEL),
    ("cargo", "deny", *_PANEL, "check"),
    ("uv", "run", "--no-sync", "ruff", "check"),
    ("uv", "run", "--no-sync", "ruff", "format", "--check"),
    ("uv", "run", "--no-sync", "pyright"),
    ("uv", "run", "--no-sync", "pytest", "-q"),
)


def main() -> None:
    for command in _COMMANDS:
        print(f"== {' '.join(command)}", flush=True)
        if subprocess.run(command, check=False).returncode:
            sys.exit(f"Failed: {' '.join(command)}")
    print("All checks passed.")


if __name__ == "__main__":
    main()
