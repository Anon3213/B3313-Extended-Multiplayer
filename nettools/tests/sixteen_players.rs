//! Sixteen bridge connections against a running server, through the encrypted handshake: everyone
//! sees everyone else, a seventeenth is turned away, and a departure reaches everyone.

use b3313_bridge::client::Connection;
use b3313_protocol::{PLAYER_STATE_SIZE, PlayerId, PlayerState, PlayerUpdate};
use b3313_server::{Config, StopOnDrop, serve};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(10);
/// Where in a state each player puts its number: after the level and area, which are the same for
/// everyone, so every player is near every other and gets their states every tick.
const MARKER: usize = 4;

fn marked_state(player: PlayerId) -> PlayerState {
    let mut state = [player.get(); PLAYER_STATE_SIZE];
    state[..MARKER].copy_from_slice(&[1, 1, 0, 16]);
    state
}

/// Polls until a snapshot satisfies `done`, which gets its present mask and updates.
fn await_snapshot(
    connection: &mut Connection,
    done: impl Fn(u16, &[PlayerUpdate]) -> bool,
) -> io::Result<Vec<PlayerUpdate>> {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        let received = connection.poll(Instant::now() + Duration::from_millis(33))?;
        if let Some(snapshot) = received.snapshot
            && done(snapshot.present, &snapshot.players)
        {
            return Ok(snapshot.players);
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "no such snapshot"))
}

fn run(server: SocketAddr) -> io::Result<()> {
    let mut players = (0..16)
        .map(|_| Connection::join(server, ""))
        .collect::<io::Result<Vec<_>>>()?;
    for player in &mut players {
        player.send_state(marked_state(player.player()))?;
    }

    let latecomer = Connection::join(server, "").err();
    let refusal = latecomer.map(|error| error.to_string());
    assert_eq!(refusal.as_deref(), Some("the session is full"));

    for player in &mut players {
        let me = player.player();
        let seen = await_snapshot(player, |_, updates| updates.len() == 15)?;
        let others_only = seen.iter().all(|update| update.player != me);
        assert!(others_only, "{me:?} got its own state");
        assert!(
            seen.iter()
                .all(|update| update.state.get(MARKER) == Some(&update.player.get())),
            "states were mixed up"
        );
    }

    let mut leaver = players.remove(0);
    leaver.leave();
    let gone = leaver.player().bit();
    for player in &mut players {
        await_snapshot(player, |present, updates| {
            present & gone == 0 && updates.len() == 14
        })?;
    }
    Ok(())
}

#[test]
fn sixteen_players_see_each_other_and_a_departure() -> io::Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let server = socket.local_addr()?;
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let serving = scope.spawn(|| serve(&socket, Config::default(), &stop, |_| {}));
        let stopping = StopOnDrop(&stop);
        let outcome = run(server);
        drop(stopping);
        serving
            .join()
            .map_err(|_| io::Error::other("the server thread panicked"))??;
        outcome
    })
}
