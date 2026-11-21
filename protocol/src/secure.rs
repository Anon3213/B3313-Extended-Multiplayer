//! The encrypted channel between a bridge and the server.
//!
//! Both ends turn the session password into a key with Argon2id, using a salt the server picks for
//! each run, then run the Noise `NNpsk0` handshake with that key. Its first message only decrypts
//! with the right key, so the server learns of a wrong password at once. Every packet afterwards is
//! sealed with ChaCha20-Poly1305 under an explicit nonce, because UDP loses and reorders packets,
//! and a window of recent nonces turns away replays.
//!
//! All the cryptography is the `snow` and `argon2` crates'; this module only wires them together.

use argon2::Argon2;
use snow::{Builder, HandshakeState, StatelessTransportState};
use std::fmt;

/// The Noise protocol: a pre-shared key mixed in first, fresh X25519 keys on both sides, and
/// ChaCha20-Poly1305 with BLAKE2s.
const NOISE_PATTERN: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
pub const SALT_SIZE: usize = 16;
pub type Salt = [u8; SALT_SIZE];
/// The key Argon2id derives from the password, and Noise mixes in as its pre-shared key.
pub type Key = [u8; 32];
/// The authentication tag sealing adds to every packet.
pub const TAG_SIZE: usize = 16;
/// Room for either handshake message with its payload.
const HANDSHAKE_BUFFER: usize = 1024;
/// How many nonces below the newest a packet may arrive and still be opened.
const REPLAY_WINDOW: u64 = 64;

#[derive(Debug)]
pub struct SecureError(String);

impl fmt::Display for SecureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SecureError {}

impl From<snow::Error> for SecureError {
    fn from(error: snow::Error) -> Self {
        Self(format!("noise: {error}"))
    }
}

/// Turns the session password into the handshake key. Slow on purpose: tens of milliseconds and
/// 19 MiB per guess, so a recorded handshake cannot be brute-forced cheaply.
///
/// # Errors
/// Fails only if Argon2 rejects its own default parameters.
pub fn derive_key(password: &str, salt: &Salt) -> Result<Key, SecureError> {
    let mut key = [0; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|error| SecureError(format!("argon2: {error}")))?;
    Ok(key)
}

fn builder(key: &Key) -> Result<Builder<'_>, SecureError> {
    let params = NOISE_PATTERN
        .parse()
        .map_err(|error: snow::Error| SecureError(format!("noise pattern: {error}")))?;
    Ok(Builder::new(params).psk(0, key)?)
}

/// The client's half of a handshake in progress.
#[derive(Debug)]
pub struct Initiator(HandshakeState);

impl Initiator {
    /// Starts a handshake, returning the first message to send.
    ///
    /// # Errors
    /// Fails if Noise cannot set up, which only a broken build would cause.
    pub fn start(key: &Key) -> Result<(Self, Vec<u8>), SecureError> {
        let mut state = builder(key)?.build_initiator()?;
        let mut message = vec![0; HANDSHAKE_BUFFER];
        let length = state.write_message(&[], &mut message)?;
        message.truncate(length);
        Ok((Self(state), message))
    }

    /// Completes the handshake with the server's reply, returning the channel and the payload the
    /// server sent with it.
    ///
    /// # Errors
    /// Fails if the reply was not made by a server holding the same key.
    pub fn finish(mut self, reply: &[u8]) -> Result<(Channel, Vec<u8>), SecureError> {
        let mut payload = vec![0; HANDSHAKE_BUFFER];
        let length = self.0.read_message(reply, &mut payload)?;
        payload.truncate(length);
        let channel = Channel::new(self.0.into_stateless_transport_mode()?);
        Ok((channel, payload))
    }
}

/// The server's half of a handshake whose first message checked out.
#[derive(Debug)]
pub struct Responder(HandshakeState);

impl Responder {
    /// Reads a client's first handshake message.
    ///
    /// # Errors
    /// Fails if the message is malformed or was made with another key, as with a wrong password.
    pub fn accept(key: &Key, message: &[u8]) -> Result<Self, SecureError> {
        let mut state = builder(key)?.build_responder()?;
        let mut payload = vec![0; HANDSHAKE_BUFFER];
        state.read_message(message, &mut payload)?;
        Ok(Self(state))
    }

    /// Completes the handshake, returning the channel and the reply carrying `payload`.
    ///
    /// # Errors
    /// Fails if the payload is too large for one message.
    pub fn finish(mut self, payload: &[u8]) -> Result<(Channel, Vec<u8>), SecureError> {
        let mut reply = vec![0; HANDSHAKE_BUFFER];
        let length = self.0.write_message(payload, &mut reply)?;
        reply.truncate(length);
        Ok((Channel::new(self.0.into_stateless_transport_mode()?), reply))
    }
}

/// Remembers which recent nonces were opened, so a replayed packet is turned away.
#[derive(Clone, Copy, Debug, Default)]
struct ReplayWindow {
    newest: Option<u64>,
    /// Bit `n` is set if nonce `newest - n` was opened.
    seen: u64,
}

impl ReplayWindow {
    fn is_fresh(&self, nonce: u64) -> bool {
        match self.newest {
            None => true,
            Some(newest) if nonce > newest => true,
            Some(newest) => {
                let behind = newest - nonce;
                behind < REPLAY_WINDOW && self.seen & (1 << behind) == 0
            }
        }
    }

    fn mark(&mut self, nonce: u64) {
        match self.newest {
            Some(newest) if nonce <= newest => self.seen |= 1 << (newest - nonce),
            Some(newest) => {
                let ahead = nonce - newest;
                self.seen = if ahead < REPLAY_WINDOW {
                    (self.seen << ahead) | 1
                } else {
                    1
                };
                self.newest = Some(nonce);
            }
            None => {
                self.seen = 1;
                self.newest = Some(nonce);
            }
        }
    }
}

/// An established channel: seals what one end sends and opens what the other sent.
pub struct Channel {
    transport: StatelessTransportState,
    next_nonce: u64,
    window: ReplayWindow,
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("next_nonce", &self.next_nonce)
            .finish_non_exhaustive()
    }
}

impl Channel {
    fn new(transport: StatelessTransportState) -> Self {
        Self {
            transport,
            next_nonce: 0,
            window: ReplayWindow::default(),
        }
    }

    /// Encrypts and authenticates `plaintext`, returning the nonce to send with it.
    ///
    /// # Errors
    /// Fails if the plaintext is too large, or after 2^64 packets.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<(u64, Vec<u8>), SecureError> {
        let nonce = self.next_nonce;
        self.next_nonce = nonce
            .checked_add(1)
            .ok_or_else(|| SecureError("nonces used up".to_owned()))?;
        let mut sealed = vec![0; plaintext.len() + TAG_SIZE];
        let transport = &self.transport;
        let length = transport.write_message(nonce, plaintext, &mut sealed)?;
        sealed.truncate(length);
        Ok((nonce, sealed))
    }

    /// Decrypts a sealed packet, or returns `None` if it fails authentication, as a forged or
    /// tampered packet does, or repeats one already opened.
    pub fn open(&mut self, nonce: u64, sealed: &[u8]) -> Option<Vec<u8>> {
        if !self.window.is_fresh(nonce) {
            return None;
        }
        let mut plaintext = vec![0; sealed.len()];
        let length = self
            .transport
            .read_message(nonce, sealed, &mut plaintext)
            .ok()?;
        // Only an authentic packet may move the window.
        self.window.mark(nonce);
        plaintext.truncate(length);
        Some(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::{Channel, Initiator, Key, Responder, SecureError, derive_key};

    const SALT: [u8; 16] = [7; 16];

    /// A handshake between two keys, as a client and server with these passwords would run it.
    fn connect(client: &Key, server: &Key) -> Result<(Channel, Channel, Vec<u8>), SecureError> {
        let (initiator, hello) = Initiator::start(client)?;
        let responder = Responder::accept(server, &hello)?;
        let (server_channel, reply) = responder.finish(b"welcome")?;
        let (client_channel, payload) = initiator.finish(&reply)?;
        Ok((client_channel, server_channel, payload))
    }

    #[test]
    fn the_same_password_connects_and_carries_the_payload() -> Result<(), SecureError> {
        let key = derive_key("hunter2", &SALT)?;
        let (mut client, mut server, payload) = connect(&key, &key)?;
        assert_eq!(payload, b"welcome");

        let (nonce, sealed) = client.seal(b"state")?;
        assert_ne!(sealed.get(..5), Some(&b"state"[..]), "sent in the clear");
        assert_eq!(server.open(nonce, &sealed), Some(b"state".to_vec()));
        let (nonce, sealed) = server.seal(b"snapshot")?;
        assert_eq!(client.open(nonce, &sealed), Some(b"snapshot".to_vec()));
        Ok(())
    }

    #[test]
    fn a_wrong_password_is_caught_by_the_first_message() -> Result<(), SecureError> {
        let right = derive_key("hunter2", &SALT)?;
        let wrong = derive_key("hunter3", &SALT)?;
        let (_, hello) = Initiator::start(&wrong)?;
        assert!(Responder::accept(&right, &hello).is_err());
        // The salt matters as much as the password.
        assert_ne!(derive_key("hunter2", &[8; 16])?, right);
        Ok(())
    }

    #[test]
    fn tampered_replayed_and_forged_packets_are_turned_away() -> Result<(), SecureError> {
        let key = derive_key("", &SALT)?;
        let (mut client, mut server, _) = connect(&key, &key)?;
        let (nonce, sealed) = client.seal(b"state")?;

        let mut tampered = sealed.clone();
        if let Some(byte) = tampered.first_mut() {
            *byte ^= 1;
        }
        assert_eq!(server.open(nonce, &tampered), None);
        // The same packet presented under another nonce.
        assert_eq!(server.open(nonce + 1, &sealed), None);
        assert!(server.open(nonce, &sealed).is_some());
        assert_eq!(server.open(nonce, &sealed), None, "replayed");

        // A packet sealed on another session with the same password does not open here.
        let (mut stranger, _, _) = connect(&key, &key)?;
        let (nonce, forged) = stranger.seal(b"state")?;
        assert_eq!(server.open(nonce, &forged), None);
        Ok(())
    }

    #[test]
    fn late_packets_open_once_within_the_window() -> Result<(), SecureError> {
        let key = derive_key("", &SALT)?;
        let (mut client, mut server, _) = connect(&key, &key)?;
        let sealed: Vec<(u64, Vec<u8>)> = (0..100)
            .map(|_| client.seal(b"x"))
            .collect::<Result<_, _>>()?;
        let open = |server: &mut Channel, index: usize| {
            sealed
                .get(index)
                .and_then(|(nonce, packet)| server.open(*nonce, packet))
                .is_some()
        };

        assert!(open(&mut server, 99));
        assert!(open(&mut server, 50), "late but inside the window");
        assert!(!open(&mut server, 50), "a second time");
        assert!(!open(&mut server, 20), "too late");
        assert!(open(&mut server, 98));
        Ok(())
    }

    /// What was opened stays opened as newer packets arrive: the record of it moves along
    /// with the newest, and is not begun again.
    #[test]
    fn a_packet_cannot_be_replayed_after_newer_ones_arrived() -> Result<(), SecureError> {
        let key = derive_key("", &SALT)?;
        let (mut client, mut server, _) = connect(&key, &key)?;
        let sealed: Vec<(u64, Vec<u8>)> = (0..300)
            .map(|_| client.seal(b"x"))
            .collect::<Result<_, _>>()?;
        let open = |server: &mut Channel, index: usize| {
            sealed
                .get(index)
                .and_then(|(nonce, packet)| server.open(*nonce, packet))
                .is_some()
        };

        for index in 0..3 {
            assert!(open(&mut server, index), "in order: {index}");
        }
        assert!(!open(&mut server, 0) && !open(&mut server, 1), "replayed");
        // One that was skipped is still new, once.
        assert!(open(&mut server, 10));
        assert!(open(&mut server, 5) && !open(&mut server, 5));
        assert!(!open(&mut server, 2), "replayed after the window moved");
        // The newest leaps further than the window is wide: everything before it is too old.
        assert!(open(&mut server, 299));
        assert!(!open(&mut server, 10) && !open(&mut server, 11));
        assert!(open(&mut server, 298) && !open(&mut server, 298));
        Ok(())
    }
}
