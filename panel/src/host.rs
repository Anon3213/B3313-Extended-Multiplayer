//! Runs a session server on a thread of its own, as `b3313-server` does in its process.
//!
//! The panel sends it console commands and reads back what it reports; neither side waits on the
//! other.

use b3313_protocol::secure::{PublicKey, ServerKey};
use b3313_server::key_file::load_or_create;
use b3313_server::{Command, Config, Event, StopOnDrop, serve_with_console};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long a server asked to stop for a restart is given to say so to its players.
const RESTART_WAIT: Duration = Duration::from_secs(1);

/// What the server's thread reports.
#[derive(Debug)]
pub enum Notice {
    Event(Event),
    /// The server stopped by itself, because its socket failed.
    Failed(String),
    /// The server stopped because the session was ended, by the host in the chat or by its
    /// owner's command: its players have been told.
    Ended,
}

/// Why `address` cannot be listened on, for the user: the two usual reasons in the panel's
/// own words, which fit a line and say what to do, and any other in the system's.
fn listen_error(address: &str, error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::AddrInUse => format!(
            "Cannot listen on {address}: another program is using that port. Close it, or \
             choose another port."
        ),
        io::ErrorKind::AddrNotAvailable => format!(
            "Cannot listen on {address}: this PC has no such address. Use 0.0.0.0 for every \
             network, or 127.0.0.1 for this PC only."
        ),
        _ => format!("Cannot listen on {address}: {error}"),
    }
}

/// The name of the file the panel keeps its server's key in, beside its preferences, when
/// the server's settings name no other.
const KEY_FILE: &str = "server.key";

/// The key the panel's server is known by: kept in the file its settings name, or else in
/// the panel's own folder, `beside` its preferences, and made there the first time. None
/// where there is no such folder: the server then has a key for one run, and its invites
/// stop working when it stops.
///
/// # Errors
/// Fails, with a message for the user, if the file cannot be read or made, or is not a key.
pub fn server_key(config: &Config, beside: Option<&Path>) -> Result<Option<ServerKey>, String> {
    let ours = beside.map(|preferences| preferences.with_file_name(KEY_FILE));
    let Some(path) = config.key_file.clone().or(ours) else {
        return Ok(None);
    };
    // The panel's folder is made when its preferences are first saved, which may be later.
    if let Some(folder) = path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("could not make the folder {}: {error}", folder.display()))?;
    }
    load_or_create(&path).map(Some)
}

#[derive(Debug)]
pub struct Host {
    address: SocketAddr,
    /// The server's key, which players' bridges know it by and invites carry.
    key: PublicKey,
    /// The password that makes a player the host of the session.
    password: String,
    stop: Arc<AtomicBool>,
    commands: Sender<Command>,
    notices: Receiver<Notice>,
    thread: Option<JoinHandle<()>>,
}

impl Host {
    /// Listens on `address` and starts serving with `config`, whose `bans` are in force from the
    /// start, and whose `key` is the server's: with none, it has one for this run only.
    ///
    /// # Errors
    /// Fails, with a message for the user, if the address cannot be listened on.
    pub fn start(address: &str, mut config: Config) -> Result<Self, String> {
        // With none set, one nobody else can know, so the player who hosts is the host.
        if config.host_password.is_empty() {
            config.host_password = random_password();
        }
        let password = config.host_password.clone();
        let key = match config.key.take() {
            Some(key) => key,
            None => ServerKey::generate()
                .map_err(|error| format!("Cannot make a key for the server: {error}"))?,
        };
        let public = key.public();
        config.key = Some(key);
        let address = address.trim();
        let socket = UdpSocket::bind(address).map_err(|error| listen_error(address, &error))?;
        let address = socket
            .local_addr()
            .map_err(|error| listen_error(address, &error))?;
        let stop = Arc::new(AtomicBool::new(false));
        let (commands, received) = mpsc::channel();
        let (notify, notices) = mpsc::channel();
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("b3313-server".to_owned())
                .spawn(move || serve(&socket, config, &stop, &received, &notify))
                .map_err(|error| format!("Cannot start the server: {error}"))?
        };
        Ok(Self {
            address,
            key: public,
            password,
            stop,
            commands,
            notices,
            thread: Some(thread),
        })
    }

    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The server's key, which invites carry and a bridge joining it checks.
    #[must_use]
    pub const fn key(&self) -> PublicKey {
        self.key
    }

    /// The password that makes a player the host: the settings', or the one made up at start.
    #[must_use]
    pub fn host_password(&self) -> &str {
        &self.password
    }

    /// Passes a command to the server, which reports the outcome as an event.
    pub fn send(&self, command: Command) {
        // Fails only once the server has stopped, which `drain` reports.
        let _ = self.commands.send(command);
    }

    /// Everything reported since the last call.
    #[must_use]
    pub fn drain(&self) -> Vec<Notice> {
        self.notices.try_iter().collect()
    }

    /// Stops the server and waits for its thread, which takes at most a few milliseconds.
    /// It tells every bridge that the session has ended as it goes.
    pub fn stop(mut self) -> Vec<Notice> {
        self.shut_down();
        self.drain()
    }

    /// Stops the server to start it again: it tells every bridge so, and each then joins the
    /// new one by itself, where `stop` would have ended their sessions.
    pub fn stop_to_restart(mut self) -> Vec<Notice> {
        self.send(Command::Restart);
        // It stops at its next look at its commands, a few milliseconds off; if it does not,
        // it is stopped as ever.
        let deadline = Instant::now() + RESTART_WAIT;
        let is_running = |host: &Self| host.thread.as_ref().is_some_and(|t| !t.is_finished());
        while is_running(&self) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        self.shut_down();
        // It stopped by itself, as it does when the session is ended, but was asked to.
        let notices = self.drain().into_iter();
        notices
            .filter(|notice| !matches!(notice, Notice::Ended))
            .collect()
    }

    fn shut_down(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // A panic on that thread has already been reported by the panic hook.
            let _ = thread.join();
        }
    }
}

/// A host password for a server whose settings have none: 32 hexadecimal digits from the
/// standard library's randomly keyed hasher, which is new to every call.
fn random_password() -> String {
    let half = || RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", half(), half())
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// Says the server failed if its thread panics, which nothing else would.
struct FailedOnPanic<'a>(&'a Sender<Notice>);

impl Drop for FailedOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let message = "the server stopped for a fault in the panel; a crash report was written";
            let _ = self.0.send(Notice::Failed(message.to_owned()));
        }
    }
}

fn serve(
    socket: &UdpSocket,
    config: Config,
    stop: &AtomicBool,
    commands: &Receiver<Command>,
    notify: &Sender<Notice>,
) {
    // A panic in the server marks it stopped, so `Host` never waits on a dead thread, and
    // says so, or the panel would show it running for good.
    let _stopping = StopOnDrop(stop);
    let _failed = FailedOnPanic(notify);
    let log = |event| {
        let _ = notify.send(Notice::Event(event));
    };
    match serve_with_console(socket, config, stop, commands, log) {
        Err(error) => {
            let _ = notify.send(Notice::Failed(format!("the server stopped: {error}")));
        }
        // Not asked to by the panel, so by whoever ended the session.
        Ok(()) if !stop.load(Ordering::Relaxed) => {
            let _ = notify.send(Notice::Ended);
        }
        Ok(()) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{Host, Notice, server_key};
    use b3313_bridge::client::Connection;
    use b3313_protocol::{RemoveReason, Role};
    use b3313_server::{Command, Config, Event};
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::{Duration, Instant};

    /// Waits up to two seconds for an event that `wanted` picks out.
    fn wait_for<T>(host: &Host, wanted: impl Fn(&Event) -> Option<T>) -> Option<T> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            for notice in host.drain() {
                if let Notice::Event(event) = notice
                    && let Some(found) = wanted(&event)
                {
                    return Some(found);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn a_hosted_server_always_has_a_host_password_and_keeps_the_settings_one() -> Result<(), String>
    {
        let first = Host::start("127.0.0.1:0", Config::default())?;
        let second = Host::start("127.0.0.1:0", Config::default())?;
        // Made up, long enough not to be guessed, and not the same twice.
        assert_eq!(first.host_password().len(), 32);
        assert_ne!(first.host_password(), second.host_password());

        let config = Config {
            host_password: "my own".to_owned(),
            ..Config::default()
        };
        assert_eq!(
            Host::start("127.0.0.1:0", config)?.host_password(),
            "my own"
        );
        Ok(())
    }

    #[test]
    fn a_server_started_again_is_the_same_server_to_its_players() -> Result<(), String> {
        let folder = std::env::temp_dir().join(format!("b3313-host-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        // The panel's folder is not there yet, as before its preferences were ever saved.
        let preferences = folder.join("panel.conf");
        let key_of_a_run = || {
            let key = server_key(&Config::default(), Some(&preferences))?;
            let config = Config {
                key,
                ..Config::default()
            };
            let host = Host::start("127.0.0.1:0", config)?;
            // What a player's bridge meets is what the panel puts in its invites.
            let bridge = Connection::join(host.address(), "", 0).map_err(|e| e.to_string())?;
            assert_eq!(bridge.server_key(), host.key());
            Ok::<_, String>(host.key())
        };
        let outcome = key_of_a_run().and_then(|first| Ok((first, key_of_a_run()?)));
        let _ = std::fs::remove_dir_all(&folder);
        let (first, again) = outcome?;
        assert_eq!(again, first);

        // With no folder to keep one in, a key for the run, and no file made anywhere.
        assert_eq!(server_key(&Config::default(), None)?, None);
        Ok(())
    }

    #[test]
    fn a_player_with_the_hosts_password_is_the_host() -> Result<(), String> {
        let host = Host::start("127.0.0.1:0", Config::default())?;
        let mut bridge = Connection::join(host.address(), "", 0).map_err(|e| e.to_string())?;
        bridge
            .introduce(None, None, host.host_password())
            .map_err(|e| e.to_string())?;
        let role = wait_for(&host, |event| match event {
            Event::RoleChanged(_, role) => Some(*role),
            _ => None,
        });
        assert_eq!(role, Some(Role::Host));
        Ok(())
    }

    #[test]
    fn a_hosted_server_admits_players_and_obeys_the_panel() -> Result<(), String> {
        let host = Host::start("127.0.0.1:0", Config::default())?;
        let mut bridge = Connection::join(host.address(), "", 0).map_err(|e| e.to_string())?;
        let player = bridge.player();
        let joined = wait_for(&host, |event| match event {
            Event::Joined(id, _) => Some(*id),
            _ => None,
        });
        assert_eq!(joined, Some(player));

        host.send(Command::Kick(player));
        let kicked = wait_for(&host, |event| match event {
            Event::Kicked(id) => Some(*id),
            _ => None,
        });
        assert_eq!(kicked, Some(player));
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut removed = None;
        while removed.is_none() && Instant::now() < deadline {
            let received = bridge.poll(Instant::now() + Duration::from_millis(33));
            removed = received.map_err(|e| e.to_string())?.removed;
        }
        assert_eq!(removed, Some(RemoveReason::Kicked));
        Ok(())
    }

    #[test]
    fn a_session_the_host_ends_stops_the_server_and_its_players_are_told() -> Result<(), String> {
        let host = Host::start("127.0.0.1:0", Config::default())?;
        let mut bridge = Connection::join(host.address(), "", 0).map_err(|e| e.to_string())?;

        host.send(Command::End);

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut ended = false;
        let mut removed = None;
        while !(ended && removed.is_some()) && Instant::now() < deadline {
            let notices = host.drain();
            ended |= notices.iter().any(|notice| matches!(notice, Notice::Ended));
            let received = bridge.poll(Instant::now() + Duration::from_millis(33));
            removed = removed.or(received.map_err(|e| e.to_string())?.removed);
        }
        assert!(ended, "the panel was not told its server had stopped");
        assert_eq!(removed, Some(RemoveReason::Ended));
        Ok(())
    }

    #[test]
    fn a_server_stopped_to_start_again_tells_its_players_to_come_back() -> Result<(), String> {
        let host = Host::start("127.0.0.1:0", Config::default())?;
        let mut bridge = Connection::join(host.address(), "", 0).map_err(|e| e.to_string())?;

        let notices = host.stop_to_restart();

        // Not the host's ending the session, which the panel would say and act on.
        assert!(
            notices
                .iter()
                .all(|notice| matches!(notice, Notice::Event(_)))
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut removed = None;
        while removed.is_none() && Instant::now() < deadline {
            let received = bridge.poll(Instant::now() + Duration::from_millis(33));
            removed = received.map_err(|e| e.to_string())?.removed;
        }
        assert_eq!(removed, Some(RemoveReason::Restarting));
        Ok(())
    }

    /// The panel shows the last moderation event, which for a ban must be the ban.
    #[test]
    fn a_ban_reports_the_removal_before_the_ban() -> Result<(), String> {
        let host = Host::start("127.0.0.1:0", Config::default())?;
        let bridge = Connection::join(host.address(), "", 0).map_err(|e| e.to_string())?;
        host.send(Command::Ban(bridge.player()));
        let mut moderation = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        while moderation.len() < 2 && Instant::now() < deadline {
            for notice in host.drain() {
                match notice {
                    Notice::Event(Event::Kicked(_)) => moderation.push("kicked"),
                    Notice::Event(Event::Banned(..)) => moderation.push("banned"),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(moderation, ["kicked", "banned"]);
        Ok(())
    }

    #[test]
    fn the_starting_bans_are_in_force() -> Result<(), String> {
        let loopback = IpAddr::from(Ipv4Addr::LOCALHOST);
        let config = Config {
            bans: vec![loopback],
            ..Config::default()
        };
        let host = Host::start("127.0.0.1:0", config)?;
        host.send(Command::Unban(loopback));
        let unbanned = wait_for(&host, |event| match event {
            Event::Unbanned(ip) => Some(*ip),
            Event::CommandFailed(_) => Some(IpAddr::from(Ipv4Addr::UNSPECIFIED)),
            _ => None,
        });
        assert_eq!(
            unbanned,
            Some(loopback),
            "unbanning works only on a ban in force"
        );
        Ok(())
    }

    #[test]
    fn stopping_frees_the_address_and_a_taken_one_is_reported() -> Result<(), String> {
        let host = Host::start("127.0.0.1:0", Config::default())?;
        let address = host.address().to_string();
        let taken = Host::start(&address, Config::default());
        // In the panel's words, not the system's, which run to twice the length.
        let said = taken.err().unwrap_or_default();
        assert!(said.contains(&address), "{said}");
        assert!(
            said.contains("another program is using that port"),
            "{said}"
        );
        assert!(!said.contains("os error"), "{said}");

        // Stopped by the panel: neither a failure nor the host's ending it from the chat.
        let notices = host.stop();
        assert!(
            notices
                .iter()
                .all(|notice| matches!(notice, Notice::Event(_)))
        );
        let again = Host::start(&address, Config::default())?;
        assert_eq!(again.address().to_string(), address);
        Ok(())
    }

    #[test]
    fn an_unusable_address_is_explained() {
        let error = Host::start("not an address", Config::default()).err();
        assert!(error.is_some_and(|message| message.contains("not an address")));
    }
}
