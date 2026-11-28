# B3313 Extended Multiplayer

Online multiplayer for **B3313**, the Super Mario 64 ROM hack. Up to 16 players share one
world: everyone sees each other's Mario, and enemies, coins, stars and doors are shared. There
is a chat in the game, too.

Each player runs the real game in an emulator on their own Windows PC. A small desktop app, the
**panel**, sets everything up: it builds the multiplayer version of the game from files you
already have, starts it, and connects you to a session. It can also host a session for others.

This is an unofficial fan project. It is not affiliated with Nintendo or the B3313 team, and
it does not contain any ROM.

**[Download the latest release](https://github.com/Anon3213/B3313-Extended-Multiplayer/releases/latest)**,
unzip it anywhere, and start **`B3313 Extended Multiplayer.exe`**. You'll also need the files
listed under [What you need](#what-you-need).

> [!WARNING]
> **Still experimental.** It has been tested mostly by programs that run two games side by
> side on one PC, and played with other people only a little, so expect problems. See
> [Known limits](#known-limits) before you start.

## What you need

- **Windows 10 or 11.**
- **[Parallel Launcher](https://parallel-launcher.ca/)**, which installs the RetroArch
  emulator. Start any game in Parallel Launcher once so that it finishes setting RetroArch up.
  After that, the panel starts the game for you.
- **Your own Super Mario 64 (USA) ROM.**
- **A B3313 patch file (`.bps`)** for one of these versions:
  - Unabandoned A2 Anniversary Edition (available on romhacking.com)
  - 1.0.2 hotfix 3
  - 1.0.2 hotfix 3 with Unabandoned tweaks

  Everyone in a session must play the same version. See
  [Versions of B3313](#versions-of-b3313).

## Getting started

### 1. Set up the game (once per version)

1. Start **`B3313 Extended Multiplayer.exe`** and open the **Play** page.
2. Under **Game**, choose your Super Mario 64 ROM and your B3313 patch.

The panel checks both files, builds the multiplayer ROM and keeps it. Then it tells you which
version of B3313 you have. You only need to do this once for each version.

### 2. Join a session

1. Copy the invite the host sent you.
2. On **Play**, select **Paste invite**, then **Join**.

The panel starts the game and connects you. If your game is a different version from the
session's, you won't be let in, and the panel tells you which version the session plays.

### 3. Or host your own

1. If you want a password, set one on the **Server settings** page.
2. Open the **Overview** page and set **Listen address** to `0.0.0.0:41313`. The default,
   `127.0.0.1`, only lets in games on your own PC.
3. Select **Start server**.
4. Under **Invite players**, copy the **Full invite** and send it to your friends. It includes
   the address, the password and instructions. Tell them which version of B3313 you're playing,
   too.
5. Go to **Play** and join your own server.

The panel asks your router to open the server's port, so that players can join over the
internet. If your router refuses, see [HOSTING.md](HOSTING.md) for what to do instead. That
page also covers passwords, your server's key, and running a server without the panel.

## Playing

- **See who's playing:** hold **down on the D-pad**.
- **Find the others:** the minimap in the lower right corner shows the walls around you from
  above, turned with the camera, so up is the way you're looking. Each player in the same
  area is an arrow in the color of their cap; one beyond the map's edge is a dot at the edge,
  the way to them. A player in another area of the level is a dot at the door, pipe or warp
  that leads straight there, if one does. Press **M** to hide or show it, or type `/map`.
- **Change Mario's colors:** pick a swatch on the **Play** page, or type `/color` in the chat
  to list the colors and `/color green` to wear one.
- **Chat:** press **T** in the game, type a line, and press **Enter** to send it. Press **/**
  instead to start typing a command. You can also chat from the **Play** page; both show the
  same lines.

While you're typing, the game ignores your controller, so Mario doesn't move. The keyboard is
only read while the game's window is in front. Chat lines stay on screen for 8 seconds, and
opening the chat shows the last six again. While it is open, **Page Up** and **Page Down**
scroll back through earlier lines, and the **up** and **down arrows** bring back lines you sent.

Under **Preferences → Chat and minimap in the game** you can change:

- the chat key, or turn typing in the game off;
- how long lines stay up (5 to 60 seconds);
- how many lines show at once as they arrive (1 to 6), so the chat covers less of the game;
- the chat's size (**Large** draws it twice as big);
- whether resting the mouse on the chat's corner of the game shows it (off unless you turn it
  on);
- whether the game shows the minimap as you join (on unless you turn it off), and the key that
  shows or hides it (M, or none if you play Mario with that key; `/map` always works).

Your name can be up to 16 plain characters. If someone already has your name, you get a number
after it. Type `/name` and a new name to change it.

### Your save file

Any stars, keys or doors that anyone collects during a session are added to every player's
save, permanently. Because of that:

- The multiplayer game keeps its own save, separate from the one you use to play B3313 alone.
- The panel copies your save aside each time you join, into
  `%APPDATA%\B3313 Extended Multiplayer\save-backups`, so you can undo a session.

### If the connection drops

If the server goes quiet, or its owner restarts it, your game keeps running on its own and
reconnects by itself for up to two minutes. The chat tells you when this happens. If the server
is stopped for good, or the host ends the session, everyone's session ends.

### Pausing

While your game is paused, the other players see your Mario standing still where you left
him. Nobody can hit him, and another player's game takes over the enemies near you until you
carry on.

At the title screen or a menu you have no Mario, and the others see none. The same goes if
your game stops altogether, as when it crashes or you hold the emulator in its own menu:
after ten seconds your Mario goes from their games. You stay in the session either way, and
he is back when you are in a level again. After a crash, close the emulator and join again,
or reset the game.

## Versions of B3313

The versions of B3313 have different levels, so players can only share a session if they have
the same version. The first player to join a session decides its version for as long as anyone
is in it. When everyone has left, the next player to join decides again.

The panel identifies a patch by the game it produces, not by its file name. This matters
because two files both called "1.0.2 hotfix 3" can be different versions. Once the game is
built, **Play** shows its version under **Game**, so players can compare before they join.

You can set up more than one version. Each one gets its own multiplayer ROM and its own save,
and you switch between them by choosing another patch.

## Roles and chat commands

| Role | Who | Shown as | Can also |
|---|---|---|---|
| **Player** | Everyone who joins | — | Play and chat |
| **Moderator** | A player the host appoints | Blue shield | Kick and ban ordinary players |
| **Host** | The one player in charge of the session | Gold crown | Appoint moderators, remove anyone, pass on the host role, end the session |
| **Server owner** | Whoever runs the server | `[Server]` in the chat | Everything, from outside the game |

When you host from the panel and join your own server, you are both the server owner and the
host. They are still separate roles. The server owner doesn't have to be playing at all,
while the host is always one of the players. [HOSTING.md](HOSTING.md) explains how the host
role is decided.

Type these in the chat, in the game or on the **Play** page:

| Command | Who can use it | What it does |
|---|---|---|
| `/help` | Anyone | Lists the commands |
| `/list` | Anyone | Lists who is in the session, with their numbers |
| `/color [name]` | Anyone | Wears the colors you name, or lists them |
| `/name [new name]` | Anyone | Changes your name, or shows the one you have |
| `/kick <player>` | Host, moderators | Removes a player from the session |
| `/ban <player>` | Host, moderators | Removes a player and keeps them out |
| `/mod <player>` | Host | Makes a player a moderator |
| `/unmod <player>` | Host | Makes a moderator an ordinary player again |
| `/host <player>` | Host | Hands the host role to another player |
| `/end` | Host | Ends the session for everyone and stops the server |

A `<player>` can be a name or a number from `/list`.

These five are carried out by your own game, and only change your own game:

| Command | What it does |
|---|---|
| `/warp <level> [area]` | Takes you to that level, where it starts, or to that area of it, as `/warp 9 2` |
| `/goto <player>` | Takes you to the area a player is in, and puts you beside them |
| `/follow <player>` | Has Mario follow a player by himself, through the paintings and doors they take, until you type `/stop`; your controller does nothing meanwhile |
| `/stop` | Stops a `/warp`, `/goto` or `/follow` |
| `/map` | Hides the minimap, or shows it again, as its key does |

Level and area numbers are the ones the player list shows, as "9-2". A number that is no level
to play in, as 1, the title screen, or one that stops the game, is refused. Any player can use these in their own game; the host
cannot turn them off.

## Bots

A program on your PC can play a game with you as a player: follow you about, chat, or map the
game's warps. Everything for bots is on the panel's **Bots** page:

- **Let a bot play your game** lets a bot connect to your own game while you are in a session.
- **Bot games** starts more games beside yours while you are in a session, from 1 to 8, each a
  player of its own for a bot to play. Each opens in a small window (640 by 480), is muted,
  ignores your controller, and costs about
  a tenth of a processor core and 600 MB of memory. With **Follow you**, their Marios get into
  the game and follow yours by themselves until a bot takes over. They close when you stop
  them or leave.

[BOTS.md](BOTS.md) explains how to write a bot.

## What the panel changes in your emulator

When the panel starts the game, it changes a few RetroArch settings to keep everyone's games
in step. These changes apply only to that run. Your own settings file is never touched, and
games you start from Parallel Launcher work as they always have. To see the exact list for
your PC, open **Show what changes** under **Game** on the **Play** page.

| Change | Why |
|---|---|
| Sound is on, and the game keeps time by it | RetroArch paces the game by its sound. With sound off, the game runs at over twice its normal speed. You can still turn the volume down or mute it |
| Run-ahead, preemptive frames and rewind are off, and no saved state loads at startup | Each of these would put your game at a different moment from everyone else's |
| Hotkeys for loading states, rewinding, frame stepping, fast forward, slow motion, run-ahead and preemptive frames do nothing | A game that jumps in time or changes speed falls out of step with the others |
| The key that closes the emulator must be pressed twice | In Parallel Launcher's settings, that key is Escape, which also closes the chat line you're typing |
| Your hotkeys on keys you type with do nothing | Otherwise typing a chat message would trigger them (for example, P pauses and ~ resets). This applies only while typing in the game is on in **Preferences** |
| RetroArch takes no commands over the network | It would accept them from any PC on your network |
| RetroArch doesn't save its settings when it closes | So none of these changes reaches your own settings |

If you start the emulator yourself instead, none of this is done, and you should chat from the
panel or move the emulator's hotkeys off the typing keys first.

## How it works

B3313 was made for one player, so this project adds three pieces around it:

- **The multiplayer ROM.** The panel patches your copy of B3313 with extra code that draws the
  other players' Marios and shares objects such as enemies and coins. The rest of the game is
  left exactly as it was.
- **The bridge,** which runs inside the panel. It reads the game's memory in the emulator, sends
  your Mario's state and what happens around you to the server, and writes back what the other
  players are doing.
- **The server,** which passes everything between the players in a session and enforces the
  roles and rules. The panel can run one, or you can run `b3313-server` on its own.

All traffic between players and the server is encrypted. Every server also has its own key,
which invites carry so that your panel connects to the right server and nobody else's.

## Known limits

This project is offered as it is, for anyone to try. There's no promise of support, fixes or
further development, and nobody is expected to report problems. You're free to use, change and
share it (see [License](#license)).

So far it has been tested mostly by programs that run two games side by side on one PC and
check what each one shows of the other, and played with other people only a little. Things that
haven't been tried include:

- **Playing with more than two people,** except with simulated players.
- **Most of the game.** On the Unabandoned A2 Anniversary Edition, two linked games have only
  been taken through levels 4 to 6. Level 6, area 6 is known to freeze the game of whoever
  hosts.
- **The two 1.0.2 versions,** beyond a little. They were added later, and their engine's
  source code isn't public. They pass the same automatic tests, and two linked games have been
  through levels 4 and 5 and part of level 9. In level 4, area 4, a player standing where the
  area is entered gets pushed into a warp when the next player arrives.
- **Opening the port automatically over the internet.** Only a router that refuses has been
  tested. Forwarding the port by hand is the fallback (see [HOSTING.md](HOSTING.md)).
- **Non-US keyboards,** and the AltGr key, for typing in the game's chat.

Every game in a session must be the same version of B3313, built by the same version of this
program. The panel builds the game for you, and warns you if the game that's running is a
different one.

## For developers

[BUILDING.md](BUILDING.md) explains how to build the programs and the multiplayer ROMs, run
the tests, and add support for another version of B3313. [CHANGELOG.md](CHANGELOG.md) lists
what changed in each release.

## License

This project's own code is under the BSD Zero Clause License ([LICENSE.txt](LICENSE.txt)): do
anything you like with it, with no warranty of any kind. [NOTICE.md](NOTICE.md) lists what it
is built from, including Troll Engine's license, which covers the multiplayer ROM patches.
