# B3313 Extended Multiplayer

Online multiplayer for B3313, the Super Mario 64 ROM hack: up to 16
players in one world, with shared enemies, coins, stars and chat. It plays the real game, the
Unabandoned A2 Anniversary Edition, in an emulator on each player's PC.

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
- **Most of the game.** Two linked games have been taken through levels 4 to 6 only. One
  place there, level 6 area 6, is known to stop the game of whoever hosts.
- **Keyboards that are not US ones**, and AltGr, for typing the chat in the game.
- **More than two players at once**, other than with simulated ones.

What to know as you play:

- A session adds stars, keys and doors to your save for good. The panel copies your save
  aside each time you join, and the multiplayer game has a save of its own; see "Chat".
- While your game is paused, the others see your Mario stand where he was. Nobody can hit
  him, and the enemies your game was running are run by another player's until you go on.
- Every game in a session must be the same build. The panel builds the game for you and
  says so if the game running is another one.

## What you need

- Windows 10 or 11. Playing needs Windows; the Linux download holds only the server, for
  hosting a session on a machine that players can reach.
- [Parallel Launcher](https://parallel-launcher.ca/), which installs RetroArch, the emulator
  the game runs in. Start any game in Parallel Launcher once, so it sets RetroArch up; after
  that the panel starts the game in it for you.
- Your own Super Mario 64 (USA) ROM.
- The B3313 Unabandoned A2 Anniversary Edition patch (`.bps`), from romhacking.com.

## Playing

1. Start `B3313 Extended Multiplayer.exe` and open **Play**.
2. Under **Game**, choose your Super Mario 64 ROM and the B3313 patch. The panel checks both,
   builds the multiplayer ROM and keeps it; this is needed once.
3. To join someone, copy the invite they sent you, select **Paste invite**, then **Join**. The
   panel starts the game.
4. To host, open **Overview**, start the server, and send players the invite it shows. Then join
   your own server on **Play**.

In the game, hold the D-pad down to see who is in the session.

Pick your Mario's colors from the swatches on **Play**, or type `/color` in the chat for the
list and `/color green` to wear one. Everyone sees you in them, and so do you.

### Who is who

| | Who | Mark | Can |
|---|---|---|---|
| **Player** | Everyone who joins | None | Play, chat, `/help`, `/list`, `/color` |
| **Moderator** | A player the host appoints with `/mod` | Blue shield | Also `/kick` and `/ban` ordinary players. Lasts until they leave |
| **Host** | The one player in charge of the session | Gold crown | Also `/mod`, `/unmod`, remove anyone, and `/end` the session |
| **Server owner** | Whoever runs the server, on their PC or elsewhere | Server icon; lines show as `[Server]` | Everything, from outside the game: the panel's **Server chat**, Players and Bans pages, or the server's console |

The host and the server owner are usually the same person, and are still two things. The
server owner is whoever started the server, and need not be playing. The host is a player.
If you host from the panel and join your own server on **Play**, the panel makes your player
the host, so you are both: you speak as `[Server]` in the Server chat, and as yourself, with
the crown, on **Play** and in the game. A server run by itself has a host only if a player
is given its host password.

### Chat

Press **T** in the game to open the chat, type a line, and press **Enter** to send it. **/**
opens the chat with a command begun. An empty line closes the chat again, and so does Backspace
once nothing is left to remove. You can also chat in the panel, on **Play**; both show the same
lines and take the same commands.

| Command | Who | Does |
|---|---|---|
| `/help`, `/list` | Anyone | Lists the commands, or who is in the session |
| `/color [name]` | Anyone | Wears the colors named, or lists them |
| `/kick <player>`, `/ban <player>` | Host, moderators | Removes a player, by name or by their number in `/list` |
| `/mod <player>`, `/unmod <player>` | Host | Makes a player a moderator, or an ordinary player again |
| `/end` | Host | Ends the session for everyone, and stops the server |

While the chat is open, the game ignores the controller and the keys bound to it, so typing
does not move Mario. The keyboard is read only while the game's window is in front. Choose
another key, or turn typing in the game off, under **Preferences**.

Lines of chat stay on the game's screen for 8 seconds after the newest arrives; **Preferences**
has other times, from 5 seconds to a minute. Opening the chat shows the last six again.

The emulator has hotkeys of its own on some keys you type with: with Parallel Launcher's
settings, P pauses it and ~ resets the game. The panel turns those off in a game it starts,
and leaves the rest, such as Escape and the function keys, as they are. If you start the
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
| Run-ahead and rewind are off, and no saved state loads at the start | Each would put your game at a moment the other players' games are not at |
| The hotkeys for loading a state, rewinding, stepping a frame, fast forward and slow motion do nothing | The same: a game that jumps in time or changes speed falls out of step |
| Your hotkeys on keys you type with do nothing | You type the chat in the game's window. Only while typing in the game is on, under **Preferences** |
| RetroArch takes no commands over the network | It would take them from any PC on your network |

If you start the emulator yourself, none of this is done for you.

## Hosting over the internet

The panel asks your router to open the server's port (UPnP) and puts your public address in the
invite. If the router refuses, forward the server's UDP port to your PC by hand, or run
`b3313-server` on a machine that players can reach. Players who join see the server's IP address.

## Building

See [BUILDING.md](BUILDING.md).

## License

BSD Zero Clause (`LICENSE.txt`): do anything with it, without warranty of any kind. `NOTICE.md`
lists what it is built from.
