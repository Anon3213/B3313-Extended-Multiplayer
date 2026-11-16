# Changelog

What each version changed, newest first. Versions of the same protocol play together: every
version so far joins every other's server.

## Not yet released

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
