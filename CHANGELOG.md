# Changelog

What each version changed, newest first. Versions of the same protocol play together:
those from 0.1.0 to 0.1.4 join each other's servers, as do those from 0.2.0 on, and a version
that does not play with the ones before it says so.

## 0.2.1

Plays with 0.2.0, and builds the same games.

- **A game that could not be brought up to date is no longer played as it was.** With a game
  built by an older version of the panel, and a patch for another version of B3313 chosen on
  the Play page, joining built the patch's game and then played the old one. The panel now
  says which patch to choose.
- The panel says "a B3313 patch" where it said "the B3313 patch", as there are several.

## 0.2.0

This version does not play with earlier ones: the panel, the bridge and the server must all
be this version. The panel builds the game again by itself when you join, if your Super
Mario 64 ROM and your patch are still where they were.

- **The mod plays three versions of B3313:** Unabandoned A2 Anniversary Edition, 1.0.2
  hotfix 3, and 1.0.2 hotfix 3 with Unabandoned tweaks. The last is what some downloads
  called "1.0.2 hotfix 3" are: the same release with the Unabandoned team's changes to the
  engine.
- **Choose any of their patches on the Play page.** The panel tells which version a patch is
  by the game it makes, not by the file's name, says which one it built, and shows it under
  Game from then on.
- **Everyone in a session plays the same version**, as the versions' levels differ. The
  first player in decides which, and a session everyone has left takes whichever comes next.
  A host should tell players which version to bring.
- A player whose game is another version is not let in, and is told which version the
  session is playing and which theirs is. The server's log says so too.
- **Each version has a multiplayer ROM and a save of its own**, so you can set up more than
  one and change between them by choosing another patch. The Anniversary Edition keeps the
  save it had.
- The two 1.0.2 versions have been through less than the Anniversary Edition: the automatic
  tests, and two linked games in levels 4, 5 and part of 9. In level 4 area 4, a player
  standing where the area is entered is pushed into a warp by the next one to arrive.

## 0.1.4

- **Joining no longer costs the emulator 1.5 GB of memory.** To find the game, the bridge
  read all of the emulator's memory, including what the emulator had only set aside, which
  made Windows give it real memory for all of it: about 2 GB held where 0.5 GB was in use.
  It now reads only the memory in use.
- **The game is found in under a second**, where it took five, and the bridge no longer
  keeps a processor core busy while the game starts.
- A server with nobody playing rests: it used 1.4% of a processor core to look for packets
  a thousand times a second, and now uses about a fifth of that.

## 0.1.3

- **`/name <new name>` renames you** in a session. A name another player has is refused,
  and everyone sees who is now called what.
- **Your own name is yours again when you come straight back.** A game closed and started
  again within a few seconds found its old self still holding the name, and was called
  "Mario 2". The server now lets the old one go.
- A player whose name was taken, and so got a number after it, is told so in the chat, and
  the panel's name field says what a name may be.
- The panel shows its version: at the foot of the sidebar, and in a new "About" panel on the
  Preferences page, which also opens the project's page.

The names are the server's doing: a 0.1.3 panel or `b3313-server` is needed to host with
them, and players on earlier versions can join it and use `/name` as they are.

## 0.1.2

- The panel, the bridge and the server have an icon: two pairs of eyes in the dark, one
  pair to a player. The panel's window and taskbar button show it too.
- The panel's server settings show the host password under a new "Advanced" heading, which
  says that hosting from the panel needs nothing typed there.
- In the Players table, the lines between the headings follow the mouse that drags them, as
  the Log's do: right of the address they moved the other way. There is now a line between
  Address and Connected, and none after Flags, the last column.

Nothing changed in how games and servers talk, or in the game itself.

## 0.1.1

- **A session has one host.** A second player who joined with the host password used to
  become a host as well, and neither could remove the other. Now the first keeps the role,
  the second is an ordinary player, and the server owner is told. A host whose connection
  drops has the role again when they are back.
- **`/host <player>` passes the host's role on.** The host types it in the chat and is an
  ordinary player from then on. The server owner can name the host too, with `host <player>`
  at the server's console or `/host <player>` in the panel's Server chat, which is also how
  a session gets a host after its host has left.
- The README says that playing needs Windows, that the Linux download is the server alone,
  and that Parallel Launcher installs RetroArch.

Only the server changed in a way that matters: a 0.1.1 panel or `b3313-server` is needed to
host with these rules, and players on 0.1.0 can join it as they are.

## 0.1.0

The first public release.
