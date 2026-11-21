//! The bridge's connection code against a real server on a loopback socket.

use b3313_bridge::client::Connection;
use b3313_protocol::metrics::REPORT_INTERVAL;
use b3313_protocol::{
    ChatLine, HitKind, Message, Name, Outfit, PLAYER_STATE_SIZE, PlayerId, PlayerUpdate,
    RemoveReason,
};
use b3313_server::{Command, Config, StopOnDrop, serve, serve_with_console};
use std::io;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Runs `test` against a server, then stops the server, even if `test` panics.
fn with_server(test: impl FnOnce(std::net::SocketAddr) -> io::Result<()>) -> io::Result<()> {
    with_configured_server(Config::default(), test)
}

/// As `with_server`, with the server's settings given.
fn with_configured_server(
    config: Config,
    test: impl FnOnce(std::net::SocketAddr) -> io::Result<()>,
) -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = socket.local_addr()?;
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let server = scope.spawn(|| serve(&socket, config, &stop, |_| {}));
        let stopping = StopOnDrop(&stop);
        let outcome = test(address);
        drop(stopping);
        server
            .join()
            .map_err(|_| io::Error::other("server thread panicked"))??;
        outcome
    })
}

/// Polls until a snapshot with another player's state arrives, or `limit` passes.
fn first_snapshot(
    connection: &mut Connection,
    limit: Duration,
) -> io::Result<Option<Vec<PlayerUpdate>>> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        let snapshot = connection.poll(Instant::now())?.snapshot;
        if let Some(snapshot) = snapshot.filter(|snapshot| !snapshot.players.is_empty()) {
            return Ok(Some(snapshot.players));
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(None)
}

/// Windows fails a receive of a datagram longer than its buffer, where other systems cut the
/// datagram short; neither may end the server, which anyone can send to.
#[test]
fn a_datagram_too_long_to_be_a_packet_is_dropped() -> io::Result<()> {
    with_server(|server| {
        let stranger = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        stranger.send_to(&[0xA5; 2000], server)?;

        Connection::join(server, "", 0).map(|_| ())
    })
}

#[test]
fn a_bridge_whose_game_is_another_version_is_told_which_the_session_plays() -> io::Result<()> {
    with_server(|server| {
        let _first = Connection::join(server, "", 2)?;
        let Err(error) = Connection::join(server, "", 1) else {
            return Err(io::Error::other("joined a session playing another game"));
        };
        // Both games by name, the session's first.
        let told = error.to_string();
        let session = told.find("B3313 1.0.2 hotfix 3,");
        let own = told.find("B3313 Unabandoned A2 Anniversary Edition");
        assert!(session.is_some() && session < own, "{told}");

        // The same game, and one that is not known, are let in.
        Connection::join(server, "", 2)?;
        Connection::join(server, "", 0).map(|_| ())
    })
}

#[test]
fn two_bridges_see_each_other() -> io::Result<()> {
    with_server(|server| {
        let mut first = Connection::join(server, "", 0)?;
        let mut second = Connection::join(server, "", 0)?;
        // In the same level and area, so each gets the other's every state.
        second.send_state(standing_at(0.0))?;
        first.send_state(standing_at(7.0))?;

        let seen = first_snapshot(&mut second, Duration::from_secs(2))?;

        // The first state sent is the first sequence number.
        let expected = PlayerUpdate {
            player: first.player(),
            sequence: 1,
            state: standing_at(7.0),
        };
        assert_eq!(seen, Some(vec![expected]));
        assert!(second.is_server_alive(Instant::now()));
        Ok(())
    })
}

#[test]
fn a_ping_times_the_round_trip_and_the_report_counts_the_link() -> io::Result<()> {
    with_server(|server| {
        let mut connection = Connection::join(server, "", 0)?;
        let start = Instant::now();
        connection.ping_if_due(start)?;
        // Not due yet, so nothing is sent.
        connection.ping_if_due(start)?;
        let waited = Duration::from_millis(300);
        connection.poll(start + waited)?;

        let report = connection
            .report_if_due(start + REPORT_INTERVAL)
            .ok_or_else(|| io::Error::other("no report after a full interval"))?;
        assert_eq!(report.sent.packets, 1);
        assert_eq!(report.round_trips.count, 1);
        assert!(report.round_trips.max < waited);
        // The Pong and a snapshot every 33 ms, none of them lost on loopback.
        assert!(report.received.packets >= 4, "{report}");
        assert_eq!(report.losses.missing, 0);
        Ok(())
    })
}

/// A state in area 1 of level 16, `x` units along one axis.
fn standing_at(x: f32) -> b3313_protocol::PlayerState {
    let mut state = [0; PLAYER_STATE_SIZE];
    // struct MpPlayerSlot: active, areaIndex, levelNum, then pos[0].
    let place = [1, 1, 0, 16];
    let position = x.to_be_bytes();
    for (slot, byte) in state.iter_mut().zip(place.iter().chain(&position)) {
        *slot = *byte;
    }
    state
}

#[test]
fn a_hit_sent_over_and_over_reaches_its_target_once() -> io::Result<()> {
    with_server(|server| {
        let mut attacker = Connection::join(server, "", 0)?;
        let mut target = Connection::join(server, "", 0)?;
        attacker.send_state(standing_at(0.0))?;
        target.send_state(standing_at(100.0))?;
        // Let both states reach the server before the hit is judged.
        attacker.poll(Instant::now() + Duration::from_millis(100))?;

        attacker.queue_hit(target.player(), HitKind::Kick);
        let mut hurts = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            attacker.send_hits()?;
            let received = target.poll(Instant::now() + Duration::from_millis(33))?;
            hurts.extend(received.hurts);
        }

        let seen: Vec<_> = hurts.iter().map(|h| (h.attacker, h.kind)).collect();
        assert_eq!(seen, [(attacker.player(), HitKind::Kick)]);
        Ok(())
    })
}

/// Runs `test` against a server that takes console commands, as `with_server` does.
fn with_console_server(
    test: impl FnOnce(std::net::SocketAddr, &mpsc::Sender<Command>) -> io::Result<()>,
) -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let server = socket.local_addr()?;
    let stop = AtomicBool::new(false);
    let (commands, received) = mpsc::channel();
    thread::scope(|scope| {
        let (socket, stop) = (&socket, &stop);
        // The command receiver moves to the server's thread; only the sender stays here.
        let config = Config::default();
        let serving =
            scope.spawn(move || serve_with_console(socket, config, stop, &received, |_| {}));
        let stopping = StopOnDrop(stop);
        let outcome = test(server, &commands);
        drop(stopping);
        serving
            .join()
            .map_err(|_| io::Error::other("server thread panicked"))??;
        outcome
    })
}

/// Polls until the server says why `connection` was removed, or two seconds pass.
fn removal(connection: &mut Connection) -> io::Result<Option<RemoveReason>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let received = connection.poll(Instant::now() + Duration::from_millis(33))?;
        if received.removed.is_some() {
            return Ok(received.removed);
        }
    }
    Ok(None)
}

/// A bridge in the chat: its connection, and the lines it has been sent that no check has
/// looked for yet.
struct Chatter {
    connection: Connection,
    lines: Vec<ChatLine>,
}

impl Chatter {
    fn join(server: std::net::SocketAddr, name: &str, host_password: &str) -> io::Result<Self> {
        let mut connection = Connection::join(server, "", 0)?;
        connection.introduce(Name::new(name).as_ref(), None, host_password)?;
        Ok(Self {
            connection,
            lines: Vec::new(),
        })
    }

    fn poll(&mut self) -> io::Result<Option<RemoveReason>> {
        let until = Instant::now() + Duration::from_millis(33);
        let received = self.connection.poll(until)?;
        self.lines.extend(received.said);
        Ok(received.removed)
    }

    /// Has this player type `text`, sent as a bridge sends it: again each send slot until the
    /// server says it has it.
    fn say(&mut self, text: &str) -> io::Result<()> {
        let message = Message::new(text).ok_or_else(|| io::Error::other("an empty message"))?;
        self.connection.queue_say(message);
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.connection.says_waiting() > 0 {
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "\"{text}\" was never acknowledged"
                )));
            }
            self.connection.send_says()?;
            self.poll()?;
        }
        Ok(())
    }

    /// Waits until this bridge has been sent a chat line containing `wanted`, and returns who
    /// it was from; or an error once two seconds pass.
    fn heard(&mut self, wanted: &str) -> io::Result<Option<PlayerId>> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let found = self
                .lines
                .iter()
                .position(|line| line.text.as_str().contains(wanted));
            if let Some(index) = found {
                return Ok(self.lines.remove(index).from);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!("never heard \"{wanted}\"")));
            }
            self.poll()?;
        }
    }
}

#[test]
fn a_moderator_the_host_appoints_in_the_chat_removes_a_player() -> io::Result<()> {
    let config = Config {
        host_password: "my own".to_owned(),
        ..Config::default()
    };
    with_configured_server(config, |server| {
        let mut host = Chatter::join(server, "Mario", "my own")?;
        let mut helper = Chatter::join(server, "Luigi", "")?;
        let mut third = Chatter::join(server, "Peach", "")?;
        // Everyone is told as each is listed, and that comes from the server, not a player.
        assert_eq!(host.heard("Peach joined")?, None);

        helper.say("hello all")?;
        assert_eq!(host.heard("hello all")?, Some(helper.connection.player()));

        // Not a moderator yet, so told so; then appointed, and obeyed.
        helper.say("/kick Peach")?;
        assert_eq!(helper.heard("Only the host and moderators")?, None);
        host.say("/mod luigi")?;
        assert_eq!(third.heard("Luigi is now a moderator")?, None);
        helper.say("/kick Peach")?;
        assert_eq!(removal(&mut third.connection)?, Some(RemoveReason::Kicked));
        assert_eq!(host.heard("Peach was removed")?, None);
        Ok(())
    })
}

#[test]
fn each_line_of_help_is_one_row_of_the_chat_in_the_game() -> io::Result<()> {
    let config = Config {
        host_password: "my own".to_owned(),
        ..Config::default()
    };
    with_configured_server(config, |server| {
        let mut host = Chatter::join(server, "Mario", "my own")?;
        host.heard("Mario joined")?;

        host.say("/help")?;
        // The host's help is the longest, and its last line is the host's own command.
        let deadline = Instant::now() + Duration::from_secs(2);
        let is_all =
            |lines: &[ChatLine]| lines.iter().any(|line| line.text.as_str().contains("/end"));
        while !is_all(&host.lines) && Instant::now() < deadline {
            host.poll()?;
        }
        assert!(host.lines.len() >= 5, "{:?}", host.lines);
        // As the game shows a line from the server: after a dash. A line cut in two would
        // push another off the six rows the game shows at once.
        for line in &host.lines {
            let shown = format!("- {}", line.text.as_str());
            assert_eq!(b3313_bridge::text::rows(&shown).len(), 1, "{shown}");
        }
        Ok(())
    })
}

#[test]
fn the_host_ends_the_session_from_the_chat_and_everyone_is_told() -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let server = socket.local_addr()?;
    let config = Config {
        host_password: "my own".to_owned(),
        ..Config::default()
    };
    // Set only when the test is over: the server is to stop at the host's word alone.
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let serving = scope.spawn(|| serve(&socket, config, &stop, |_| {}));
        let _stopping = StopOnDrop(&stop);
        let mut host = Chatter::join(server, "Mario", "my own")?;
        let mut guest = Chatter::join(server, "Luigi", "")?;
        host.heard("Luigi joined")?;

        guest.say("/end")?;
        assert_eq!(guest.heard("Only the host can end")?, None);

        // Not with `say`, which would take the answer for itself: the answer is the removal.
        let end = Message::new("/end").ok_or_else(|| io::Error::other("an empty message"))?;
        host.connection.queue_say(end);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut told = None;
        while told.is_none() && Instant::now() < deadline {
            host.connection.send_says()?;
            told = host.poll()?;
        }
        assert_eq!(told, Some(RemoveReason::Ended));
        assert_eq!(removal(&mut guest.connection)?, Some(RemoveReason::Ended));

        while !serving.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            serving.is_finished(),
            "the server ran on after its session ended"
        );
        Ok(())
    })
}

#[test]
fn a_server_that_is_stopped_tells_its_players_the_session_has_ended() -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let server = socket.local_addr()?;
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| serve(&socket, Config::default(), &stop, |_| {}));
        let stopping = StopOnDrop(&stop);
        let mut player = Connection::join(server, "", 0)?;

        drop(stopping);

        // At once, where it used to wait out the server's silence to learn it.
        assert_eq!(removal(&mut player)?, Some(RemoveReason::Ended));
        Ok(())
    })
}

/// Polls until the server's roster shows `player` wearing `outfit`, or two seconds pass.
fn sees_worn(connection: &mut Connection, player: PlayerId, outfit: Outfit) -> io::Result<bool> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let received = connection.poll(Instant::now() + Duration::from_millis(33))?;
        let worn = received.roster.is_some_and(|roster| {
            roster
                .iter()
                .any(|entry| entry.player == player && entry.outfit == outfit)
        });
        if worn {
            return Ok(true);
        }
    }
    Ok(false)
}

#[test]
fn an_outfit_a_bridge_asks_to_wear_is_shown_to_everyone() -> io::Result<()> {
    with_server(|server| {
        let mut chooser = Chatter::join(server, "Luigi", "")?;
        let mut watcher = Chatter::join(server, "Peach", "")?;
        let luigi = chooser.connection.player();
        let gold = Outfit::named("gold").ok_or_else(|| io::Error::other("no gold outfit"))?;
        // Until he asks, he wears what his number gives, which is not gold.
        assert_ne!(Outfit::of_player(luigi), gold);

        chooser.connection.wear(gold)?;
        assert!(sees_worn(&mut watcher.connection, luigi, gold)?);
        assert!(sees_worn(&mut chooser.connection, luigi, gold)?);
        Ok(())
    })
}

#[test]
fn a_kicked_bridge_is_told_and_the_others_stay() -> io::Result<()> {
    with_console_server(|server, commands| {
        let mut stayer = Connection::join(server, "", 0)?;
        let mut kicked = Connection::join(server, "", 0)?;
        commands
            .send(Command::Kick(kicked.player()))
            .map_err(io::Error::other)?;

        assert_eq!(removal(&mut kicked)?, Some(RemoveReason::Kicked));

        // The other player is still in, and sees the kicked one gone.
        let mut present = u16::MAX;
        let deadline = Instant::now() + Duration::from_secs(2);
        while present != 0 && Instant::now() < deadline {
            let received = stayer.poll(Instant::now() + Duration::from_millis(33))?;
            if let Some(snapshot) = received.snapshot {
                present = snapshot.present;
            }
        }
        assert_eq!(present, 0);
        assert!(stayer.is_server_alive(Instant::now()));
        Ok(())
    })
}

#[test]
fn banning_an_address_removes_everyone_playing_from_it() -> io::Result<()> {
    with_console_server(|server, commands| {
        let mut first = Connection::join(server, "", 0)?;
        let mut second = Connection::join(server, "", 0)?;
        commands
            .send(Command::BanAddress(Ipv4Addr::LOCALHOST.into()))
            .map_err(io::Error::other)?;

        assert_eq!(removal(&mut first)?, Some(RemoveReason::Banned));
        assert_eq!(removal(&mut second)?, Some(RemoveReason::Banned));
        Ok(())
    })
}

#[test]
fn a_bridge_with_nothing_to_send_stays_in_the_session() -> io::Result<()> {
    with_server(|server| {
        let mut idle = Connection::join(server, "", 0)?;
        let timeout = Config::default().player_timeout;
        let deadline = Instant::now() + timeout + Duration::from_secs(1);
        while Instant::now() < deadline {
            idle.keep_alive()?;
            idle.poll(Instant::now())?;
            thread::sleep(Duration::from_millis(100));
        }

        let mut newcomer = Connection::join(server, "", 0)?;
        assert_ne!(
            newcomer.player(),
            idle.player(),
            "the idle player's number was reused"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut present = 0;
        while present == 0 && Instant::now() < deadline {
            let received = newcomer.poll(Instant::now() + Duration::from_millis(33))?;
            present = received.snapshot.map_or(0, |snapshot| snapshot.present);
        }
        // The idle player, and nobody else.
        assert_eq!(present, idle.player().bit());
        assert!(idle.is_server_alive(Instant::now()));
        Ok(())
    })
}
