# B3313 Extended Multiplayer

Online multiplayer for B3313, the Super Mario 64 ROM hack: up to 16
players in one world, with shared enemies, coins, stars and chat. It plays the real game in an
emulator on each player's PC, in any of three versions:

- Unabandoned A2 Anniversary Edition
- 1.0.2 hotfix 3
- 1.0.2 hotfix 3 with Unabandoned tweaks, which some downloads of "1.0.2 hotfix 3" are

Everyone in a session plays the same version; see "Versions of the game".

This is an unofficial project. It is not affiliated with Nintendo or the B3313 team, and it
contains no ROM.

## As is

This is released as it is, for anyone to try. It comes with no promise of support, fixes,
maintenance or further development, now or later, and nobody is expected to report anything.
It is free to use, change and pass on; see "License".

Nobody has played it with other people yet. It has been tested by programs that run two games
side by side on one PC and check what each shows of the other, and no further. Expect
problems.

What is known not to have been tried:

- **Joining over the internet through the panel's own port opening.** Only a router that
  refuses to open the port has been seen. Forwarding the port by hand is the fallback; see
  "Hosting over the internet".
- **Most of the game.** In the Unabandoned A2 Anniversary Edition, two linked games have been
  taken through levels 4 to 6 only. One place there, level 6 area 6, is known to stop the
  game of whoever hosts.
- **The two 1.0.2 versions, more than a little.** They were added later, and their engine's
  source is not public, so the multiplayer code was fitted to the games as built. Each passes
  the same automatic tests as the Anniversary Edition, and two linked games have been through
  levels 4 and 5 and part of level 9. In level 4 area 4, a player standing where the area is
  entered is pushed into a warp by the next player to arrive there.
- **Keyboards that are not US ones**, and AltGr, for typing the chat in the game.
- **More than two players at once**, other than with simulated ones.

What to know as you play:

- A session adds stars, keys and doors to your save for good. The panel copies your save
  aside each time you join, and the multiplayer game has a save of its own; see "Chat".
- While your game is paused, the others see your Mario stand where he was. Nobody can hit
  him, and the enemies your game was running are run by another player's until you go on.
- Every game in a session must be the same version of B3313, built by the same version of
  this program. The panel builds the game for you and says so if the game running is another
  one.

## What you need

- Windows 10 or 11. Playing needs Windows; the Linux download holds only the server, for
  hosting a session on a machine that players can reach.
- [Parallel Launcher](https://parallel-launcher.ca/), which installs RetroArch, the emulator
  the game runs in. Start any game in Parallel Launcher once, so it sets RetroArch up; after
  that the panel starts the game in it for you.
- Your own Super Mario 64 (USA) ROM.
- A B3313 patch (`.bps`) of one of the three versions above, the one your session plays. The
  Unabandoned A2 Anniversary Edition's is on romhacking.com.

## Playing

1. Start `B3313 Extended Multiplayer.exe` and open **Play**.
2. Under **Game**, choose your Super Mario 64 ROM and a B3313 patch. The panel checks both,
   builds the multiplayer ROM and keeps it, and says which version of B3313 it is; this is
   needed once for each version you play.
3. To join someone, copy the invite they sent you, select **Paste invite**, then **Join**. The
   panel starts the game. Your game must be the version theirs is; see "Versions of the game".
4. To host, open **Overview**, start the server, and send players the invite it shows, and tell
   them which version of B3313 you play. Then join your own server on **Play**.

In the game, hold the D-pad down to see who is in the session.

Pick your Mario's colors from the swatches on **Play**, or type `/color` in the chat for the
list and `/color green` to wear one. Everyone sees you in them, and so do you.

### Versions of the game

The versions of B3313 have different levels, so players can only share a session if their
games are the same version. The first player into a session decides which it plays, and
anyone who then joins with another is not let in and is told which version the session plays.
Once everyone has left, the session takes whichever version comes next.

The panel tells which version a patch is by the game it makes, not by the file's name: two
files both called "1.0.2 hotfix 3" can be different versions. It says which one it built when
the game is ready, and **Play** shows it under **Game** from then on, so players can compare
before they join. Each version has a multiplayer ROM and a save of its own, so you can set up
more than one and change between them by choosing another patch.

### Who is who

| | Who | Mark | Can |
|---|---|---|---|
| **Player** | Everyone who joins | None | Play, chat, `/help`, `/list`, `/color`, `/name` |
| **Moderator** | A player the host appoints with `/mod` | Blue shield | Also `/kick` and `/ban` ordinary players. Lasts until they leave |
| **Host** | The one player in charge of the session | Gold crown | Also `/mod`, `/unmod`, remove anyone, pass the role on with `/host`, and `/end` the session |
| **Server owner** | Whoever runs the server, on their PC or elsewhere | Server icon; lines show as `[Server]` | Everything, from outside the game: the panel's **Server chat**, Players and Bans pages, or the server's console |

The host and the server owner are usually the same person, and are still two things. The
server owner is whoever started the server, and need not be playing. The host is a player.
If you host from the panel and join your own server on **Play**, the panel makes your player
the host, so you are both: you speak as `[Server]` in the Server chat, and as yourself, with
the crown, on **Play** and in the game. A server run by itself has a host only if its owner
names one with `host <player>`, or a player joins with its host password, which takes
`b3313-bridge` run by hand: **Play** has no field for one.

A session has one host. The first player to join with the host password keeps the role: anyone
who joins with it later is an ordinary player, and the server owner is told. The host hands
the role to another player with `/host <player>` and is an ordinary player from then on. If
the host leaves without doing so, the session has no host until the server owner names one or
a player joins with the host password. A host who joined with the host password has the role
again when they are back from a dropped connection; one who was named must be named again.

### Chat

Press **T** in the game to open the chat, type a line, and press **Enter** to send it. **/**
opens the chat with a command begun. An empty line closes the chat again, and so does Backspace
once nothing is left to remove. You can also chat in the panel, on **Play**; both show the same
lines and take the same commands.

| Command | Who | Does |
|---|---|---|
| `/help`, `/list` | Anyone | Lists the commands, or who is in the session |
| `/color [name]` | Anyone | Wears the colors named, or lists them |
| `/name [new name]` | Anyone | Takes the new name, or says the one you have |
| `/kick <player>`, `/ban <player>` | Host, moderators | Removes a player, by name or by their number in `/list` |
| `/mod <player>`, `/unmod <player>` | Host | Makes a player a moderator, or an ordinary player again |
| `/host <player>` | Host | Makes that player the host in the host's place |
| `/end` | Host | Ends the session for everyone, and stops the server |

A name is up to 16 plain characters, and no two players in a session have the same one,
whatever the case of its letters. If the name you join with is taken, you get a number after
it and the chat says so; `/name` and a name that is free changes it, at most every ten
seconds. A name is free again as soon as its player has left. If you close the game and come
straight back, you have your own name again.

While the chat is open, the game ignores the controller and the keys bound to it, so typing
does not move Mario. The keyboard is read only while the game's window is in front. Choose
another key, or turn typing in the game off, under **Preferences**.

Lines of chat stay on the game's screen for 8 seconds after the newest arrives; **Preferences**
has other times, from 5 seconds to a minute. Opening the chat shows the last six again.

The emulator has hotkeys of its own on some keys you type with: with Parallel Launcher's
settings, P pauses it and ~ resets the game. The panel turns those off in a game it starts,
and leaves the rest, such as the menu's F1, as they are, but for those in the table under
"What the panel changes in your emulator". If you start the
emulator yourself, take its hotkeys off the typing keys first, or chat in the panel.

Stars, keys and doors that anyone gets in a session are added to every player's own save file.
The panel copies your save aside each time you join, into
`%APPDATA%\B3313 Extended Multiplayer\save-backups`, so a session can be undone. The multiplayer
game keeps a save of its own, apart from the one you play B3313 alone with.

If the server goes quiet, or its owner restarts it, your game plays on alone and joins again
by itself, for up to two minutes; the chat says when. The owner stopping the server, or the
host's `/end`, ends the session for everyone.

## What the panel changes in your emulator

When the panel starts the game, it changes a few of RetroArch's settings so that the session
keeps in step. The changes last for that run only. Your own settings file is never written to,
and games you start from Parallel Launcher are as they always were. The **Game** panel on
**Play** lists them for your PC, under **Show what changes**.

| Change | Why |
|---|---|
| Sound is on, and the game keeps time by it | RetroArch paces a game by its sound; with sound off the game runs at over twice its speed. You can still mute it |
| Run-ahead, preemptive frames and rewind are off, and no saved state loads at the start | Each would put your game at a moment the other players' games are not at |
| The hotkeys for loading a state, rewinding, stepping a frame, fast forward, slow motion, run-ahead and preemptive frames do nothing | The same: a game that jumps in time or changes speed falls out of step |
| The key that closes the emulator must be pressed twice | With Parallel Launcher's settings it is Escape, which also closes a line of chat you are typing |
| Your hotkeys on keys you type with do nothing | You type the chat in the game's window. Only while typing in the game is on, under **Preferences** |
| RetroArch takes no commands over the network | It would take them from any PC on your network |

If you start the emulator yourself, none of this is done for you.

## Hosting over the internet

The panel asks your router to open the server's port (UPnP) and puts your public address in the
invite. If the router refuses, forward the server's UDP port to your PC by hand, or run
`b3313-server` on a machine that players can reach. Players who join see the server's IP address.

## Building

See [BUILDING.md](BUILDING.md). [CHANGELOG.md](CHANGELOG.md) lists what each version changed.

## License

BSD Zero Clause (`LICENSE.txt`): do anything with it, without warranty of any kind. `NOTICE.md`
lists what it is built from.
