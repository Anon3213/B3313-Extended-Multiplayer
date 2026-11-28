# Bots

A bot is a program on your PC that plays a game of B3313 Extended Multiplayer, as a player in
a session with everyone else. The multiplayer program reads and writes the game's memory
anyway, so it can let a bot do the same, through a small API: the bot sees where Mario and the
other players are, holds the game's controller, chats, and sends Mario to a level, to a player,
after a player, or to a point.

Nothing here needs a bot made in any one way. A script that follows you about, a language
model that reads the chat and decides what to do, or a program that maps the game's warps all
use the same few requests.

## Setting one up

Each bot plays its own game, in its own emulator, joined to the session like any player.

- **Your own game, from the panel:** turn on **Let a bot play your game** on the **Bots** page,
  then join a session from the **Play** page. While you are in the session, the bot connects
  to `127.0.0.1:33130` and plays that game, the panel's own. The Play page says when a bot
  connects and leaves.
- **Bot games beside yours, from the panel:** while you are in a session, the **Bots** page
  starts from 1 to 8 more games, each a player of its own in your session: your game, copied,
  in an emulator of its own that opens in a window 640 by 480 behind the panel, whatever size
  your own game's is, draws its picture at 640 by 480, is muted, reads no controller, and goes
  on running behind other windows. A bot that looks at the screen gets that 640 by 480
  picture. The page lists the port each one's bot connects to, `127.0.0.1:33131` and
  on, and whether one has; **Stop bot games**, or leaving, closes them. Under **When they
  join**, **Follow you** has their Marios get into the game and follow yours until a bot takes
  the controller; **Wait for a bot** leaves them at the title screen.
- **From a command line**, for bots on a PC without the panel: start each bot's
  emulator with the multiplayer ROM, which the panel builds into
  `%APPDATA%\B3313 Extended Multiplayer\game`.
  Give each bot a copy of it under a name of its own, as `bot1.z64`, or the emulators all write
  one save. Then give each bot's bridge a port of its own:

  ```powershell
  $env:B3313_NAME = "Bot1"; $env:B3313_BOT_PORT = "33131"; .\b3313-bridge.exe "<multiplayer ROM>" <emulator PID> <server address>
  ```

  `b3313-bridge.exe` without arguments lists its other settings.

The port is open to programs on this PC alone, and only while the session is. One bot at a time
plays a game; a second one that connects is told so and let go. This has been tried with
RetroArch's ParallelN64 core, as Parallel Launcher installs it.

## Talking to the bridge

The bot opens a TCP connection to the port and sends requests, one JSON object a line, each
ending in a newline. The bridge answers each request on a line of its own, and sends news
between answers as things happen. The first line must be a `hello`; a connection that starts
with anything else is closed.

Give a request an `"id"`, any number or text, and every answer to it carries the same value as
`"re"`. A request that cannot be done is answered `{"type":"error","error":"...","re":...}`.

```text
→ {"type":"hello","id":1}
← {"type":"welcome","api":1,"game":"Unabandoned A2 Anniversary Edition","player":3,"name":"Bot1","buttons":["A","B",...],"pad_frames_most":900,"levels":[4,5,6,...,36],"re":1}
→ {"type":"follow","player":"Luigi","id":2}
← {"type":"ok","doing":"Following Luigi.","re":2}
← {"type":"chat","from":"Luigi","text":"over here"}
← {"type":"moved","name":"Luigi","you":false,"from":{"level":6,"area":1,"pos":[512,0,-1980]},"to":{"level":9,"area":1,"pos":[-6558,0,6464]}}
```

### Requests

| Request | Answer | What it does |
|---|---|---|
| `{"type":"hello"}` | `welcome` | Starts the conversation: which version of B3313 the game is, the bot's player number and name, the names of the buttons, and the levels a warp may name |
| `{"type":"look"}` | `state` | Where everything is now (below) |
| `{"type":"watch","every":0.5}` | `ok` | Sends the `state` every so many seconds, unasked, 0.1 at the fastest; `0` stops it |
| `{"type":"map"}` | `map` | The areas of the level the game is in, and where each one's warps lead (below) |
| `{"type":"pad","buttons":["A"],"stick":[0,1],"frames":30}` | `ok` | Holds those buttons and that stick for so many of the game's frames, at most 900, in place of the controller; `"frames":0` lets go at once. The stick is `[right, up]`, each from -1 to 1 |
| `{"type":"say","text":"hi"}` | `ok` | Says a line in the chat, as typed by the bot's player. A command for the server, as `/list`, works as typed |
| `{"type":"warp","level":9,"area":2}` | `ok`, then `done` | Takes Mario to that area of that level, or to where the level starts if no area is named |
| `{"type":"goto","player":"Luigi"}` | `ok`, then `done` | Takes Mario to the area that player is in, and puts him beside them |
| `{"type":"follow","player":"Luigi"}` | `ok` | Has Mario follow that player until stopped (below); `done` comes only if it fails |
| `{"type":"go","x":100,"y":0,"z":-2500}` | `ok`, then `done` | Walks Mario to that point of the area he is in, jumping when he is stuck or the way goes up |
| `{"type":"stop"}` | `ok` | Ends a `warp`, `goto`, `follow` or `go`, whose sender is told `done` with `"ok":false` |

A player's name may be given whole, in any case, or by its start, if only one name starts so.
`done` carries `"ok"`, true if Mario got there, and `"words"`, which say what happened: `"At
9-2."`, `"Level 9 has no area 15; it has 0, 1, 2, 3."`. A new `warp`, `goto`, `follow`, `go`
or `pad` replaces whatever Mario was doing.

### What comes unasked

- `{"type":"chat","from":"Luigi","text":"..."}`: every line of the chat, `from` null for the
  server's own.
- `{"type":"moved","name":"...","you":false,"from":{...},"to":{...}}`: a player, or the bot's
  own Mario (`"you":true`), is in another area than before. `from` is where they were last seen
  in the old one, which is where a painting, a door or a pipe took them; `to` is where they
  came out.
- `state`, while watching.

### The state

```json
{"type":"state","playing":true,"level":9,"area":1,
 "mario":{"pos":[-6558,0,6464],"vel":[0,0,0],"speed":0,"yaw":90,"action":"0x0c400201",
          "in_air":false,"swimming":false,"health":8,"coins":3,"stars":12,"lives":4},
 "camera_yaw":-180,
 "players":[{"name":"Luigi","player":2,"level":9,"area":1,"pos":[-6000,0,6000],"distance":727}],
 "doing":"following Luigi"}
```

- `playing` is false at a menu or between areas, when nothing can be done.
- Places are the game's own units: Mario is about 160 tall. Y is up.
- Angles are degrees: 0 faces +Z, 90 faces +X. `camera_yaw` is the angle from what the camera
  looks at to the camera, or null when the game has no camera.
- `health` is the wedges of the power meter, 0 to 8. `action` is Mario's action as Super Mario
  64 numbers them; the decompilation's `sm64.h` names them.
- `players` lists the other players in a level, as this game draws them, and `distance` is
  seen from above, for those in the same area only.

### The map

```json
{"type":"map","level":9,"area":1,"areas":[
  {"area":1,"exits":[{"node":10,"to":{"level":6,"area":1,"node":11},"pos":[-6558,0,6464]}]},
  {"area":2,"exits":[...]}]}
```

Each exit is one of the level's warp nodes, with the level, area and node it leads to. Only the
area Mario is in has a `pos` for its exits: where Mario comes out when he arrives by that node,
which for a door or a pipe is also where to go to leave by it. Paintings are not among them; the
`moved` news finds those.

## Getting about in B3313

- **From the title screen:** a game joins the session as soon as it has started, at the title
  screen, where `playing` is false. A `warp`, `goto` or `follow` sent then presses Start and A
  in turn, once a second, through the title screen, the file select and any text, until Mario
  runs in a level, and is carried out there; `doing` says "getting into the game" meanwhile. A
  bot that would rather press its own way in sends `pad`, which ends that.
- **Steering by yourself:** the game turns the stick by the camera. To walk toward a point, take
  the angle to it, `atan2(dx, dz)`, less `camera_yaw`, and push the stick to `[sin, -cos]` of
  what is left: straight up walks away from the camera. A push under about a tenth does nothing.
  `go` and `follow` do this for you.
- **Paintings** take Mario in when he jumps into them. A follower that loses its player goes to
  where the player was last seen and jumps there, which takes it through the painting or door
  they used; if that does not work within 15 seconds, it warps after them.
- **Warps** go where the game's own warps go. A level is started as the file select starts it,
  with Mario where its script puts him; an area of the level Mario is in is reached by one of
  that area's own warp nodes, so an area the level lacks is refused, not entered. Warping to a
  player who is in another level starts that level first. The level numbers that are no
  level to play in, as 1, the title screen, and others that stop the game, are refused. `welcome` lists the
  levels a warp may name.
- **Learning the maze:** the `moved` news, kept from session to session, is a graph of where
  each way out of an area leads. The B3313 community's maps, such as
  [b3313-unpacked](https://github.com/b3313-unpacked/b3313-unpacked) and the
  [B3313 wiki's list of areas](https://b3313.fandom.com/wiki/List_of_Areas), name the rooms and
  their exits for people, though not by the game's level and area numbers.

## An example

A bot in Python that follows a player and says where it is when asked:

```python
import json
import socket

PLAYER = "Luigi"

bot = socket.create_connection(("127.0.0.1", 33130))
lines = bot.makefile("r", encoding="utf-8")


def send(request):
    bot.sendall((json.dumps(request) + "\n").encode())


send({"type": "hello"})
print(json.loads(lines.readline()))
send({"type": "follow", "player": PLAYER})
for line in lines:
    message = json.loads(line)
    if message["type"] == "chat" and message["from"] == PLAYER:
        if "where are you" in message["text"].lower():
            send({"type": "look"})
    elif message["type"] == "state":
        send({"type": "say", "text": f"In {message['level']}-{message['area']}."})
    elif message["type"] == "done" and not message["ok"]:
        print(message["words"])
        send({"type": "follow", "player": PLAYER})
```

## Limits

- A bot sees only its own game: other players as that game draws them, and nothing of an area
  Mario is not in but the level's warps. The bridge takes no pictures; a bot that looks at the
  screen takes them of the emulator's window itself.
- While a bot holds the controller, a gamepad on that emulator does nothing, and while the bot
  is sending Mario somewhere, the bridge holds it.
- A `warp`, a `goto`, and a `follow` that cannot keep up on foot, move Mario as no player
  could. The commands `/warp`, `/goto`, `/follow` and `/stop` let anyone do the same in their
  own game; the session's host cannot turn them off.
- The game runs at 30 frames a second, so `"frames":30` is about a second.
