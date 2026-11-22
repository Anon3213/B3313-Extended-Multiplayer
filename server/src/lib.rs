//! Session server: admits up to `MAX_PLAYERS` bridges and relays each one's state to the others.
//!
//! `Session` holds all the rules and does no I/O, so tests can drive it directly; `serve`
//! connects it to a socket, through the gate that every packet passes first. What a session
//! does is reported as `Event`s, and `commands` is the one list of what can be typed at it.

pub mod bans;
mod coins;
pub mod commands;
pub mod config;
pub mod console;
mod event;
mod gate;
pub mod key_file;
mod limits;
pub mod movement;
pub mod objects;
pub mod people;
pub mod progress;
mod serve;
mod session;

pub use config::Config;
pub use console::Command;
pub use event::{Event, HitRejection, TickReport};
pub use serve::{StopOnDrop, serve, serve_with_console};
pub use session::{
    FAR_UPDATE_TICKS, INTERACTION_REACH, OBJECT_REFRESH_TICKS, QUIET_RESEND_TICKS, SERVER_NAME,
    SETTLING_RESENDS, Session, TICK_INTERVAL, WORLD_RESEND_TICKS,
};
