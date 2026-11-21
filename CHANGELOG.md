# Changelog

What each version changed, newest first. Versions of the same protocol play together:
those from 0.1.0 to 0.1.4 join each other's servers, as do those from 0.2.0 on, and a version
that does not play with the ones before it says so.

## Not yet released

From a review of the whole project. It plays with 0.2.1's servers and players; the panel
builds the game again by itself at the next join, as the game changed.

In the game:

- **Pausing no longer takes coins away.** When a game was paused and went on, coins nobody
  had collected could vanish from it: as many as others had collected of the same ring.
- **Coins others collected stay gone after Mario dies** in the area, where they came back
  for the player who died.
- **Mario is no longer left stuck** when he talks to a character, or is grabbed by an
  enemy, that another player's game was running because that player stood nearer.
- **Enemies others defeated stay gone after Mario dies** in the area too, where the player
  who died had them back and fought what nobody else could see.
- Enemies no longer all turn on your Mario for one frame now and then, which happened when
  the game read who runs what while it was being written.
- An enemy you attacked is handed on to a nearer player's game when the server says so, not
  some seconds later, during which two games ran it.
- A game reset in a session has the session's world again before its first level.
- **With three or more players, two could become one Mario** after a level loaded: one was
  not drawn, and a hit on the other landed on both.
- The other players no longer turn see-through or metal when you put on such a cap.
- A player who takes the number of one who just left is no longer shown frozen where that
  one stood for minutes; for a second at most.
- **A game no longer stands still for good** when another player talks to a character
  while it is speaking to you.

In the server:

- **A ban removes everyone who plays from the banned address**, each told why. The others
  used to be left in the session unheard until they timed out. In the chat, a ban that would
  shut out your own address, or a moderator only the host may remove, is refused.
- One machine can no longer keep newcomers out by sending from a thousand ports.
- A player already in the session can no longer have another removed by claiming their name.
- In a busy area, every enemy now moves for everyone: with more news than a packet holds,
  the same few were sent each time.
- A session everyone has left no longer gives its stars to players of another version of
  B3313.
- A ban of an IPv4 address also holds on a server that hears both IPv4 and IPv6.
- A `/kick` or `/ban` in the chat removes only the player it named, not whoever took their
  number if they left first.
- A settings or ban file saved as "UTF-8 with BOM" is read, where it was refused whole, and
  an unban takes the address out of the ban file however it was written there.
- **`b3313-server` stopped with Ctrl+C, or by a signal on Linux, ends the session in order**:
  players are told at once, where they waited and tried to join again for two minutes.

In the panel and the bridge:

- **Ban on a player's row bans nobody if that player has left** while the question stood.
- A game the panel starts has preemptive frames off, and the key that closes the emulator,
  Escape with Parallel Launcher's settings, must be pressed twice, as it also closes a line
  of chat.
- Reset all no longer turns streamer mode off, and streamer mode hides a few things it
  missed.
- The bridge no longer risks crashing the emulator while it looks for a game that is slow
  to start, says at once when the emulator was closed, and makes about 2,700 fewer system
  calls a second in a busy area.
- The game the panel builds is written whole or not at all.
- The panel says when it found no save of the game to copy aside before a session.
- Restart waits for the router to close the port before it asks for it again, which made
  it less likely, not impossible, that the port just opened is closed.
- A host password taken out of the settings no longer lives on through Restart.
- A setting in your emulator's file that is not valid text no longer stops every join, and
  a file far too large to be a ROM or a patch is refused unread.
- A network that fails again each time the bridge has joined again ends the session with
  those words, and not as "The game closed".
- A line in the Server chat too long for the game's chat is refused, where players lost
  its end; `/list` there shows its answer.

In a release: Lucide's license, and for the Linux server the licenses of what it is built
from; the commit a panel was built from is in its crash reports again.

## 0.2.1

**About the version numbers:** 0.2.0 was made the same day and never published, and this
takes its place. It holds everything 0.2.0 was to have, listed here, and what was put right
after it, at the end. So the versions published go from 0.1.4 straight to 0.2.1. The tag
`v0.2.0` is still in the repository, and anything built from it plays with this version.

This version does not play with 0.1.4 and earlier: the panel, the bridge and the server must
all be this version. The panel builds the game again by itself when you join, if your Super
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

Put right since 0.2.0:

- **A game that could not be brought up to date is no longer played as it was.** With a game
  built by an older version of the panel, and a patch for another version of B3313 chosen on
  the Play page, joining built the patch's game and then played the old one. The panel now
  says which patch to choose.
- The panel says "a B3313 patch" where it said "the B3313 patch", as there are several.

## 0.2.0

Never published: 0.2.1 took its place the same day, and its changes are listed there.

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
