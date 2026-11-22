//! The encrypted channel between a bridge and the server.
//!
//! Both ends turn the session password into a key with Argon2id, using a salt the server picks for
//! each run, then run the Noise `NXpsk0` handshake with that key. Its first message only decrypts
//! with the right key, so the server learns of a wrong password at once. The server's reply
//! carries its own long-term public key and proves it holds the private half: the password says
//! who may join, and that key says which server was reached, since every player knows the
//! password and could otherwise pose as the server. Every packet afterwards is sealed with
//! ChaCha20-Poly1305 under an explicit nonce, because UDP loses and reorders packets, and a
//! window of recent nonces turns away replays.
//!
//! All the cryptography is the `snow` and `argon2` crates'; this module only wires them together.

use argon2::Argon2;
use snow::{Builder, HandshakeState, StatelessTransportState};
use std::fmt;
use std::str::FromStr;

/// The Noise protocol: a pre-shared key mixed in first, fresh X25519 keys on both sides, the
/// server's long-term key sent in its reply, and ChaCha20-Poly1305 with BLAKE2s.
///
/// There is one pattern and no way to ask for another: a server that offered a handshake
/// without its key would have to say so before anything is authenticated, and someone between
/// the two could then choose the weaker one for them.
const NOISE_PATTERN: &str = "Noise_NXpsk0_25519_ChaChaPoly_BLAKE2s";
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

/// The size of either half of a server's key.
pub const KEY_SIZE: usize = 32;
/// The letters a key is written in: RFC 4648's base32, which has no two that look alike in
/// both cases.
const KEY_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// How many letters a key is written in: five bits a letter, the last one's low bits unused.
pub const KEY_TEXT_LENGTH: usize = (KEY_SIZE * 8).div_ceil(5);

/// Writes a key's bytes as letters.
fn key_text(bytes: &[u8; KEY_SIZE]) -> String {
    let mut text = String::with_capacity(KEY_TEXT_LENGTH);
    let mut bits = 0_u32;
    let mut count = 0_u32;
    let mut letter = |index: u32| {
        let letter = usize::try_from(index & 31)
            .ok()
            .and_then(|index| KEY_ALPHABET.get(index));
        text.push(char::from(letter.copied().unwrap_or(b'A')));
    };
    for &byte in bytes {
        bits = (bits << 8) | u32::from(byte);
        count += 8;
        while count >= 5 {
            count -= 5;
            letter(bits >> count);
        }
    }
    if count > 0 {
        letter(bits << (5 - count));
    }
    text
}

/// Reads a key's bytes from its letters, in either case, or returns `None` if they are not
/// exactly one key: a letter outside the alphabet, too few or too many, or unused bits set,
/// which no key is written with.
fn key_bytes(text: &str) -> Option<[u8; KEY_SIZE]> {
    if text.len() != KEY_TEXT_LENGTH {
        return None;
    }
    let mut bytes = [0; KEY_SIZE];
    let mut filled = bytes.iter_mut();
    let mut bits = 0_u32;
    let mut count = 0_u32;
    for letter in text.bytes() {
        let letter = letter.to_ascii_uppercase();
        let value = KEY_ALPHABET.iter().position(|&known| known == letter)?;
        bits = (bits << 5) | u32::try_from(value).ok()?;
        count += 5;
        if count >= 8 {
            count -= 8;
            *filled.next()? = u8::try_from((bits >> count) & 0xFF).ok()?;
        }
    }
    let unused = bits & ((1 << count) - 1);
    (unused == 0).then_some(bytes)
}

/// The public half of a server's key: what a player's bridge knows a server by, wherever the
/// server's address has moved to. Written as `KEY_TEXT_LENGTH` letters.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; KEY_SIZE]);

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&key_text(&self.0))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl FromStr for PublicKey {
    type Err = SecureError;

    fn from_str(text: &str) -> Result<Self, SecureError> {
        key_bytes(text).map(Self).ok_or_else(|| {
            SecureError(format!(
                "a server's key is {KEY_TEXT_LENGTH} letters and digits"
            ))
        })
    }
}

/// What separates a server's address from its key in a join code.
pub const JOIN_CODE_SEPARATOR: char = '#';

/// Writes the join code for the server at `address` with `key`: what its owner gives players.
#[must_use]
pub fn join_code(address: &str, key: &PublicKey) -> String {
    format!("{address}{JOIN_CODE_SEPARATOR}{key}")
}

/// Splits what a player gave as a server into its address and the key it must have: a join
/// code has both, and a plain address no key.
///
/// # Errors
/// Fails if there is a key and it is not one, so that a mistyped key is never taken for none.
pub fn split_join_code(server: &str) -> Result<(&str, Option<PublicKey>), SecureError> {
    match server.split_once(JOIN_CODE_SEPARATOR) {
        Some((address, key)) => Ok((address.trim(), Some(key.trim().parse()?))),
        None => Ok((server.trim(), None)),
    }
}

/// A server's long-term key, both halves. The private half is what makes a server that
/// server: whoever has it can pose as it, so it is kept in a file of its own and never shown.
#[derive(Clone)]
pub struct ServerKey {
    private: [u8; KEY_SIZE],
    public: PublicKey,
}

impl fmt::Debug for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerKey")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

/// Two keys are one server's if their public halves are: the private half is never compared,
/// which would take a time that tells of it.
impl PartialEq for ServerKey {
    fn eq(&self, other: &Self) -> bool {
        self.public == other.public
    }
}

impl Eq for ServerKey {}

impl ServerKey {
    /// Makes a new key, as a server does the first time it runs.
    ///
    /// # Errors
    /// Fails if the system gives no random numbers.
    pub fn generate() -> Result<Self, SecureError> {
        let pair = builder(&[0; 32])?.generate_keypair()?;
        let private = <[u8; KEY_SIZE]>::try_from(pair.private.as_slice())
            .map_err(|_| SecureError("the new key has the wrong size".to_owned()))?;
        Self::from_private(private)
    }

    /// The key whose private half is `private`.
    ///
    /// The public half is found by running the handshake against itself and reading the key
    /// the client's side met, so that it is by construction the one players will see, and the
    /// curve's arithmetic stays `snow`'s.
    fn from_private(private: [u8; KEY_SIZE]) -> Result<Self, SecureError> {
        let psk = [0; 32];
        let (initiator, hello) = Initiator::start(&psk)?;
        let mut state = builder(&psk)?
            .local_private_key(&private)?
            .build_responder()?;
        let mut buffer = vec![0; HANDSHAKE_BUFFER];
        state.read_message(&hello, &mut buffer)?;
        let length = state.write_message(&[], &mut buffer)?;
        let reply = buffer.get(..length).unwrap_or_default();
        let (_, _, public) = initiator.finish(reply)?;
        Ok(Self { private, public })
    }

    /// The public half, which players' bridges check.
    #[must_use]
    pub const fn public(&self) -> PublicKey {
        self.public
    }

    /// The private half as text, for the server's key file and nowhere else.
    #[must_use]
    pub fn private_text(&self) -> String {
        key_text(&self.private)
    }

    /// Reads a key back from `private_text`'s form, as a key file holds it.
    ///
    /// # Errors
    /// Fails if the text is not a key.
    pub fn from_private_text(text: &str) -> Result<Self, SecureError> {
        let private = key_bytes(text.trim())
            .ok_or_else(|| SecureError("this is not a server's key".to_owned()))?;
        Self::from_private(private)
    }
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

    /// Completes the handshake with the server's reply, returning the channel, the payload the
    /// server sent with it, and the server's key, which the reply proved the server holds.
    /// Whether that is the server the player meant is the caller's to check.
    ///
    /// # Errors
    /// Fails if the reply was not made by a server holding the same password's key.
    pub fn finish(mut self, reply: &[u8]) -> Result<(Channel, Vec<u8>, PublicKey), SecureError> {
        let mut payload = vec![0; HANDSHAKE_BUFFER];
        let length = self.0.read_message(reply, &mut payload)?;
        payload.truncate(length);
        let server = self
            .0
            .get_remote_static()
            .and_then(|key| <[u8; KEY_SIZE]>::try_from(key).ok())
            .map(PublicKey)
            .ok_or_else(|| SecureError("the server sent no key".to_owned()))?;
        let channel = Channel::new(self.0.into_stateless_transport_mode()?);
        Ok((channel, payload, server))
    }
}

/// The server's half of a handshake whose first message checked out.
#[derive(Debug)]
pub struct Responder(HandshakeState);

impl Responder {
    /// Reads a client's first handshake message, as the server whose key is `server`.
    ///
    /// # Errors
    /// Fails if the message is malformed or was made with another key, as with a wrong password.
    pub fn accept(key: &Key, server: &ServerKey, message: &[u8]) -> Result<Self, SecureError> {
        let mut state = builder(key)?
            .local_private_key(&server.private)?
            .build_responder()?;
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
    use super::{
        Channel, Initiator, KEY_TEXT_LENGTH, Key, PublicKey, Responder, SecureError, ServerKey,
        derive_key, join_code, split_join_code,
    };

    const SALT: [u8; 16] = [7; 16];

    /// A handshake between two keys, as a client and server with these passwords would run it.
    fn connect(client: &Key, server: &Key) -> Result<(Channel, Channel, Vec<u8>), SecureError> {
        let (initiator, hello) = Initiator::start(client)?;
        let responder = Responder::accept(server, &ServerKey::generate()?, &hello)?;
        let (server_channel, reply) = responder.finish(b"welcome")?;
        let (client_channel, payload, _) = initiator.finish(&reply)?;
        Ok((client_channel, server_channel, payload))
    }

    /// The key a client meets at a server that has `server`, both with one password.
    fn key_met(server: &ServerKey) -> Result<PublicKey, SecureError> {
        let key = derive_key("hunter2", &SALT)?;
        let (initiator, hello) = Initiator::start(&key)?;
        let (_, reply) = Responder::accept(&key, server, &hello)?.finish(b"")?;
        let (_, _, met) = initiator.finish(&reply)?;
        Ok(met)
    }

    #[test]
    fn a_client_meets_the_servers_own_key_and_no_other_servers() -> Result<(), SecureError> {
        let server = ServerKey::generate()?;
        assert_eq!(key_met(&server)?, server.public());
        assert_eq!(key_met(&server)?, server.public(), "the same at every join");
        // Another server, though it has the password, is seen to be another.
        let impostor = ServerKey::generate()?;
        assert_ne!(key_met(&impostor)?, server.public());
        Ok(())
    }

    #[test]
    fn a_key_read_back_from_its_file_is_the_same_server() -> Result<(), SecureError> {
        let server = ServerKey::generate()?;
        let kept = server.private_text();
        let restored = ServerKey::from_private_text(&format!("{kept}\n"))?;
        assert_eq!(restored.public(), server.public());
        assert_eq!(key_met(&restored)?, server.public());
        assert!(!format!("{server:?}").contains(&kept), "shown in a log");
        assert!(ServerKey::from_private_text("not a key").is_err());
        Ok(())
    }

    #[test]
    fn a_public_key_is_read_from_its_text_and_from_nothing_else() -> Result<(), SecureError> {
        let key = ServerKey::generate()?.public();
        let text = key.to_string();
        assert_eq!(text.len(), KEY_TEXT_LENGTH);
        assert_eq!(text.parse::<PublicKey>()?, key);
        assert_eq!(text.to_lowercase().parse::<PublicKey>()?, key);

        let cut = text.get(1..).unwrap_or_default();
        assert!(cut.parse::<PublicKey>().is_err(), "too short");
        assert!(format!("{text}A").parse::<PublicKey>().is_err(), "too long");
        assert!(format!("1{cut}").parse::<PublicKey>().is_err(), "no such letter");
        // The last letter carries one bit of the key and four that no key sets: two texts
        // must never be read as one key.
        let all_ones = "7".repeat(KEY_TEXT_LENGTH);
        assert!(all_ones.parse::<PublicKey>().is_err(), "unused bits set");
        Ok(())
    }

    #[test]
    fn a_join_code_gives_the_address_and_the_key() -> Result<(), SecureError> {
        let key = ServerKey::generate()?.public();
        let code = join_code("play.example:7777", &key);
        assert_eq!(split_join_code(&code)?, ("play.example:7777", Some(key)));
        assert_eq!(split_join_code(" [::1]:7777 ")?, ("[::1]:7777", None));
        // A key that is not one is an error, never a join with no key to check.
        assert!(split_join_code("play.example:7777#oops").is_err());
        assert!(split_join_code("play.example:7777#").is_err());
        Ok(())
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
        assert!(Responder::accept(&right, &ServerKey::generate()?, &hello).is_err());
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
