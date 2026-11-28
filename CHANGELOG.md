# Changelog

What each version changed, newest first. Versions of the same protocol play together:
those from 0.1.0 to 0.1.4 join each other's servers, as do those from 0.2.0 to 0.2.2, and
0.4.0 and 0.4.1; a version that does not play with the ones before it says so, as 0.3.0 and
0.4.1 do.

## Not yet released

It plays with 0.4.1's servers and players: nothing a player's program tells the server
changed, and neither did the game.

- **A Bots page in the panel**, for everything about bots. **Let a bot play your game** moved
  there from Preferences. Its shortcut is Ctrl+7, so Preferences is now Ctrl+8.
- **Bot games:** while you are in a session, the Bots page starts from 1 to 8 more games beside
  yours, each a player of its own for a bot on your PC to play, and lists the port each bot
  connects to. Each is your game in an emulator of its own that opens in a window 640 by 480
  behind the panel, even when yours plays full screen, draws its picture at 640 by 480, is
  muted, ignores your controller and goes on running behind other windows. They close when you stop them or leave
  the session. With **Follow you**, their Marios follow yours by themselves until a bot takes
  over.
- **Bots and commands work from the title screen.** A `warp`, `goto` or `follow` asked for
  there, by a bot or as `/warp`, `/goto` or `/follow` in the chat, presses through the title
  screen and the file select into a level first, and is then carried out. They were refused
  until Mario was in a level.

## 0.4.1

**About the version numbers:** 0.4.0 was tagged the same day and never published, and this
takes its place. It holds everything 0.4.0 was to have, listed first, and what was added
after it, at the end. So the versions published go from 0.3.0 straight to 0.4.1. The tag
`v0.4.0` is still in the repository, and anything built from it plays with this version.

**This version does not play with 0.3.0 and earlier**: what a player's program tells the
server changed, so everyone in a session updates together. The panel builds the game again
by itself at the next join, as the game changed too.

- **A player with no Mario is no longer shown standing.** One at the title screen or a
  menu, or whose game crashed, stood where they last were in everyone else's game for as
  long as their emulator stayed open. They now go from the others' games, at once for a
  menu and after ten seconds for a game that has stopped, and are back when they are in a
  level again. A paused game's Mario still stands where he was.
- **Coins collected before you arrived stay gone** in a ring or a line you had not yet come
  near: they appeared when you did, and counted for everyone a second time.
- A bridge closed and started again beside a game still in the session no longer takes
  coins away that nobody collected.
- Enemies other players run are no longer lost for a moment after your game hitches.
- **Something a player picks up no longer vanishes from everyone else's game.** When one
  player grabbed Bowser, he went from the other players' games until they left the room, as
  would anything picked up. And the player holding something no longer sees a second one in
  every other player's hand.
- **Doors open for the players who go through them** in everyone's game, where they stayed
  shut for all but that player.
- **After B3313's Toad jumpscare your controller works again.** If the game went on after it,
  Mario ran on by himself for good and the game stopped at the next warp, which only closing
  the emulator ended.
- **The Play page has a Host password field**, under **Advanced**, for a server whose owner
  gave you one: join with it and you are the host. Most players never open it. It is kept
  with the server when the password is, and hidden in streamer mode.
- The Log and the chats scroll with the keyboard: reach one with Tab, then the arrows, Page
  Up, Page Down, Home and End.
- The Log page no longer goes through its whole log every time it is drawn.

Added since 0.4.0, from a second player's post:

- **Bots can play.** A program on your PC can play your game as a player in a session: see
  where Mario and everyone else are, hold the game's controller, chat, and warp, go to a player
  or follow one. Turn it on under **Preferences → Bots**; `BOTS.md`, in the zip, says how to
  write one, and how to run several from the command line.
- **Commands that move your own Mario:** `/warp <level> [area]` takes you to a level or an area
  of it, `/goto <player>` to another player, `/follow <player>` has Mario follow them by
  himself, through the paintings and doors they take, and `/stop` ends it. Your own game
  carries them out, so the host cannot turn them off.
- **Larger chat:** **Preferences → Chat in the game → Chat text size → Large** draws the chat
  in the game twice as big.
- **Chat history in the game:** while the chat is open, Page Up and Page Down scroll back
  through what was said, and the up and down arrows bring back lines you sent.
- **Less chat on screen if you want it:** **Preferences → Chat in the game → Chat lines on
  screen** shows from 1 to 6 lines at once as they arrive. Opening the chat still shows six.
- **The chat can show while the mouse rests on it**, on the chat's corner of the game's
  window. It is off unless you turn it on, under **Preferences → Chat in the game**.
- In a game the panel starts, the emulator's hotkeys on Page Up, Page Down and the up and down
  arrows are turned off too, as the chat now reads those keys.

## 0.4.0

Never published: 0.4.1 took its place the same day, and its changes are listed there.

## 0.3.0

**This version does not play with 0.2.2 and earlier**: the way a player's program and a
server greet each other changed, so everyone in a session updates together.

- **A server now has a key, and your program checks it.** Until now the session password was
  all that stood between you and someone posing as the server, and every player has the
  password. An invite now carries the server's key, after a `#` in its address, and with it
  your panel joins that server and nobody else who answers at the address. A server joined
  by its address alone is remembered with the key it had, and the panel refuses another
  there afterwards, saying so.
- **A server whose address changes is still the same server.** Paste a new invite and the
  panel knows it by its key: it takes the old one's place under **Recent**.
- **A session survives the server's address changing**, where the server was given by a
  name: your program looks the name up again each time it joins again, and joins only the
  server it was in.
- **A name for invites.** If you host and have a name that follows your address (dynamic
  DNS), type it on **Overview**: invites give it in place of the address, and go on working
  when the address changes.
- The Play page keeps ten recent servers, where it kept five, and marks one whose password is
  saved with a padlock.
- For whoever runs a server: its key is kept in `server.key`, in the panel's own folder, or
  beside the settings file of a `b3313-server` (the new `key_file` setting names another
  place). Keep that file to stay the same server, copy it to move the server to another PC,
  and show it to nobody. `b3313-server` prints the key to give players as it starts, and
  `b3313-bridge` takes an invite's address as it is, or the key alone as `B3313_SERVER_KEY`.
- The sidebar's line for your game says what happened in a few words that fit it, as "Did
  not join", and the Play page says why. A server is remembered once you have joined it, so
  a mistyped address no longer stays under **Recent**.
- The README is written anew for players, and what only a server's owner needs is in
  `HOSTING.md`, which comes in the zip and with the Linux server.

## 0.2.2

From a review of the whole project. It plays with 0.2.1's servers and players; the panel
builds the game again by itself at the next join, as the game changed. Everyone in a session
is better off with it: several of these are faults one player's game shows another.

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
- Mario no longer keeps the session's colors after its bridge was closed without leaving.
- A player who takes the number of one who just left is no longer shown frozen where that
  one stood for minutes; for a second at most.
- **A game no longer stands still for good** when another player talks to a character
  while it is speaking to you.

In the server:

- **A ban removes everyone who plays from the banned address**, each told why. The others
  used to be left in the session unheard until they timed out. In the chat, a ban that would
  shut out your own address, or a moderator only the host may remove, is refused.
- One machine can no longer keep newcomers out by sending from a thousand ports, nor cut a
  player off by sending a flood under that player's address.
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
- A panel that cannot open its window says why in a box, where it seemed to do nothing.
- A moderator's ban in the game no longer takes away the panel's message with your Undo.
- Restart keeps the router's port open for the server it starts, where it closed it and
  asked again, and the closing could arrive last and shut the port just opened.
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
