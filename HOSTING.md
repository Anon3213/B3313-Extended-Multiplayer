# Hosting a B3313 Extended Multiplayer server

This page is for whoever runs a server. To get started quickly, follow "Or host your own" in
the [README](README.md#3-or-host-your-own). This page covers the rest: letting players in over
the internet, your server's key, running a server without the panel, and moderating a session.

## Hosting from the panel

The panel runs the server inside itself, so there's nothing else to install.

1. On **Server settings**, choose the session's settings. The most useful ones are
   **Password** (empty lets anyone join; traffic is encrypted either way) and
   **Player limit** (up to 16). You can save them to a file and open that file again later.
2. On **Overview**, set **Listen address**:
   - `127.0.0.1:41313` lets in games on this PC only. It's the default, so nobody else can
     join by accident.
   - `0.0.0.0:41313` lets in other machines, on your network and over the internet.
3. Select **Start server**.
4. Under **Invite players**, copy one of the following and send it to your players:
   - **Full invite:** the address, the password and short instructions.
   - **Address + password:** just those two.
   - **Address:** only the address, if you'd rather give out the password some other way.

   What you copy also carries your server's key (see [Your server's key](#your-servers-key)).
5. On **Play**, join your own server. Your player becomes the host automatically.

**Restart** restarts the server with new settings. Players' games carry on alone and
reconnect by themselves. **Stop server**, or `/end` from the host, ends the session for
everyone.

## Letting players in over the internet

When the server listens on `0.0.0.0`, the panel asks your router to open the server's port
(UPnP) and puts your public address in the invite. **Overview** tells you whether that worked.

If your router refuses, you have two options:

- Forward the server's **UDP** port (41313 unless you changed it) to your PC in your router's
  settings.
- Run `b3313-server` on a machine that players can reach, such as a rented server (see
  [Running a server without the panel](#running-a-server-without-the-panel)).

Players who join can see the server's IP address.

### If your address changes

Most home internet connections get a new address from time to time. When yours changes, send
your players a new invite. Their panels recognize your server by its key, so it simply
replaces the old entry in their recent servers.

If you have a domain name that follows your address (dynamic DNS, which many routers can keep
up to date), type it under **Name for invites** on **Overview**. Invites then use that name in
place of the address and keep working when the address changes. Players already in a session
find the server again by themselves. The name must point to your public address, and your
router must still let the server's port in.

## Your server's key

Every server has a key of its own, made the first time it runs. An invite carries it after a
`#` at the end of the server's address. With it, a player's panel connects only to your server
and refuses anything else that answers at that address. The password alone can't do that,
because every player knows it.

- The key is kept in a file called `server.key`. The panel keeps it in
  `%APPDATA%\B3313 Extended Multiplayer`. `b3313-server` keeps it beside its settings file, or
  in the folder it was started from. The **Key file** setting, under **Advanced** in
  **Server settings**, chooses another place.
- **Keep this file** to stay the same server. To move your server to another PC, copy the file
  there.
- **Show it to nobody.** Whoever has it can pretend to be your server.
- If it's lost, the server makes a new key, and your players will need a new invite.

## Running a server without the panel

The release zip includes `b3313-server.exe`. Each release also has a Linux version,
`b3313-server-v<version>-linux-x64.tar.gz`, for hosting on a machine players can reach. Start
it with the address to listen on and, optionally, a settings file:

```
b3313-server 0.0.0.0:41313 --config server.conf
```

`server.example.conf`, which comes with the server (and is in this repository at
[`server/server.example.conf`](server/server.example.conf)), lists every setting with its
default and allowed range. Without `--config`, every setting keeps its default. When the
server starts, it prints a join code (your address, `#`, and the key) to give your players.

A server run this way has no host until you name one with `host <player>`, or a player joins
with its `host_password`: they type it under **Host password** on the panel's **Play** page,
or give it to the command-line bridge, `b3313-bridge` (run it without arguments for help).

## Moderating a session

There are two roles in charge of a session, and they're easy to confuse:

- The **server owner** runs the server and can do everything, from outside the game. They
  don't need to be playing. Their chat messages are marked `[Server]`.
- The **host** is one of the players, marked with a gold crown. They can appoint moderators,
  remove players, hand the host role on, and end the session, all from the chat.

When you host from the panel and join your own server, you are both: you speak as `[Server]`
in the panel's **Server chat**, and as yourself, wearing the crown, on **Play** and in the
game.

### Commands for the server owner

Type these into `b3313-server`'s console, or after a `/` in the panel's **Server chat** (on
the **Players** page). The panel's **Players** and **Bans** pages do the same with buttons.

| Command | What it does |
|---|---|
| `list` | Lists who is in the session, with their numbers |
| `kick <player>` | Removes a player |
| `ban <player or IP>` | Removes a player and keeps their IP address out |
| `unban <IP>` | Lets a banned IP address in again |
| `mod <player>`, `unmod <player>` | Makes a player a moderator, or an ordinary player again |
| `host <player>` | Makes a player the host |
| `say <message>` | Sends a message to everyone as `[Server]` |
| `end` | Ends the session for everyone and stops the server |

In the console, `<player>` is a number from `list`. In the panel's **Server chat**, it can
also be a name. Bans last until the server stops, unless the **Ban file** setting names a
file to keep them in.

The players' own chat commands are listed in the
[README](README.md#roles-and-chat-commands).

### How the host is chosen

A session has one host at a time.

- The first player to join with the server's host password becomes the host and keeps the
  role. Anyone who joins with the same password later is an ordinary player, and the server
  owner is told about it. (When you host from the panel, it makes up a host password for you
  and uses it when you join, so you don't need to set one.)
- The host can hand the role to another player with `/host <player>`, and is then an ordinary
  player.
- If the host leaves without handing the role on, the session has no host until the server
  owner names one, or a player joins with the host password.
- A host who joined with the host password gets the role back after a dropped connection. A
  host who was named by someone else has to be named again.

Moderators keep their role until they leave the session.
