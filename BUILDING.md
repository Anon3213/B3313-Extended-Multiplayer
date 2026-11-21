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

## The ROMs

Only needed to change the code that runs inside the game, `TrollEngine/Multiplayer/`. There
is one multiplayer ROM for each version of B3313 the mod plays, listed in
`scripts/game_versions.py`, and all are built the same way: the multiplayer code is added to
the game's ROM, and the game's own code is left as it is.

1. Fetch Troll Engine, B3313's engine, whose compiler, assembler and headers the build uses.
   This repository does not contain it:

   ```bash
   python scripts/engine.py setup
   ```

   Then build its tools once, on Linux or in WSL (`build-essential` is all it needs), from
   `TrollEngine/`:

   ```bash
   make tools
   ```

2. Put your own Super Mario 64 (USA) ROM and the B3313 patches somewhere outside git's reach
   (`ROM/` and every `.bps` are ignored), and make each version's ROM. The script tells which
   version a patch is by the ROM it makes:

   ```bash
   python scripts/setup_base_rom.py <a B3313 .bps> <your Super Mario 64 ROM>
   ```

3. Build every version's multiplayer ROM, or only those you name. On Windows this runs the
   build in WSL:

   ```bash
   uv run --no-sync python scripts/build_game.py
   ```

4. Make the patches the panel carries from the new ROMs, and commit them:

   ```bash
   uv run --no-sync python scripts/make_patch.py
   ```

### Another version of B3313

Most versions of B3313 were built from engine sources nobody published, so where a version
keeps the engine functions and variables the multiplayer code uses is read out of its ROM.
To add a version:

1. Give it a row in `scripts/game_versions.py`, with a number no version has had, and make
   its ROM as in step 2 above.
2. Build Troll Engine's objects at one or more commits near the version's date: check each
   commit out somewhere, and run `make objs` there. A function the version changed after one
   commit may still match another, so several commits find more.
3. Read the addresses out of the ROM, which writes the version's file in
   `TrollEngine/Multiplayer/versions/`:

   ```bash
   uv run --no-sync python scripts/find_engine_symbols.py <version> Bee/<version>.z64 <tree>...
   ```

   It stops rather than guess. If the version loads the engine differently, or a function
   the hooks rely on is not as expected, it says which, and that part needs work by hand.
4. Build it and make its patch, as above, then give the panel its row (`GAMES` in the panel's
   `game_setup` module) and the bridge and server its name (the protocol crate's `game`
   module). Tests fail while those two differ from `scripts/game_versions.py`.

The build checks each instruction it changes against the ROM first, and stops if the ROM
holds something else there.

The font for names and chat is drawn in `TrollEngine/Multiplayer/font.txt`. After changing it,
run `uv run --no-sync python scripts/make_font.py` and commit `font.bin` with it; add
`--preview font.png` to see sample text in the font without building a ROM.

The mailbox the game and the bridge share is laid out in `TrollEngine/Multiplayer/mailbox.h`
and nowhere else. After changing it, run `uv run --no-sync python scripts/make_layout.py` and
commit `bridge/src/layout.rs` with it, which the bridge takes every address from; a test
fails while it is out of date.

The programs' icons are drawn by `scripts/make_icons.py`, from shapes in the script itself.
After changing it, run `uv run --no-sync python scripts/make_icons.py` and commit what it
writes in `icons/`; `--preview icons.png` draws them all into one picture to look at first.

Never commit a ROM, a B3313 patch, or an engine file.

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
