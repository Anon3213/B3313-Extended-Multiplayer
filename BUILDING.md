# Building B3313 Extended Multiplayer

There are two halves: the programs (Rust, built on Windows) and the multiplayer ROM (C and
assembly, built on Linux or under WSL). Most changes touch only the programs, which carry the
ROM as a ready-made patch.

## The programs

You need Rust (the version in `rust-toolchain.toml`), [uv](https://docs.astral.sh/uv/),
`cargo-nextest` and `cargo-deny`.

```bash
uv sync
```

```bash
uv run --no-sync python scripts/check.py
```

The second command runs every lint and test, for both Rust workspaces and the scripts. The
panel is a workspace of its own:

```bash
cargo run --manifest-path panel/Cargo.toml
```

## The ROM

Only needed to change the code that runs inside the game, `TrollEngine/Multiplayer/`.

1. Fetch Troll Engine, B3313's engine, which this repository builds on but does not contain:

   ```bash
   python scripts/engine.py setup
   ```

2. Put your own Super Mario 64 (USA) ROM and the B3313 Unabandoned A2 Anniversary Edition
   patch somewhere outside git's reach (`ROM/` is ignored), and make the base ROM:

   ```bash
   python scripts/setup_base_rom.py <the A2AE .bps> <your Super Mario 64 ROM>
   ```

3. Build, on Linux or in WSL (`build-essential` is all it needs), from `TrollEngine/`:

   ```bash
   make clean && make -j DEBUG=0 MULTIPLAYER=1
   ```

4. Make the patch the panel carries from the new ROM, and commit it:

   ```bash
   uv run --no-sync python scripts/make_patch.py
   ```

The font for names and chat is drawn in `TrollEngine/Multiplayer/font.txt`. After changing it,
run `uv run --no-sync python scripts/make_font.py` and commit `font.bin` with it; add
`--preview font.png` to see sample text in the font without building a ROM.

The mailbox the game and the bridge share is laid out in `TrollEngine/Multiplayer/mailbox.h`
and nowhere else. After changing it, run `uv run --no-sync python scripts/make_layout.py` and
commit `bridge/src/layout.rs` with it, which the bridge takes every address from; a test
fails while it is out of date.

If you change an engine file outside `Multiplayer/`, run `python scripts/engine.py patch` and
commit `engine-hooks.patch`: git ignores the engine files themselves.

Never commit a ROM, the A2AE patch, or an engine file.

## Releasing

Set the version in `panel/Cargo.toml`, and say what changed under a heading `## <version>` in
`CHANGELOG.md`, then:

```bash
uv run --no-sync python scripts/release.py
```

It writes the players' zip and its SHA-256 under `dist/`. Pushing a tag `v<version>` has
GitHub build the same zip and a Linux server and attach them to a draft release, whose text
is that version's part of the changelog.

A release is built from the files git has, committed or staged, in a folder of its own,
`C:\b3313-release` unless `B3313_RELEASE_DIR` names another, with a Rust toolchain and crates
fetched there the first time. What is released records where it was built, and a path in
your home folder begins with your name; the script makes no zip if a file in it names that
folder.

## As is

The project is released as it is, with no promise that it will be maintained or that changes
sent to it will be looked at. You are free to build on it under the BSD Zero Clause License;
see `LICENSE.txt` and `NOTICE.md`.
