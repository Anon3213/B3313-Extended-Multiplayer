# Changelog

What each version changed, newest first. Versions of the same protocol play together: a
0.1.0 game joins a 0.1.1 server, and the other way round.

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
