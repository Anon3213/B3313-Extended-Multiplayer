//! Connects a running B3313 emulator to multiplayer sessions.

pub mod client;
pub mod interpolation;
pub mod keyboard;
pub mod layout;
pub mod mailbox;
pub mod object_motion;
pub mod objects;
pub mod pacing;
#[cfg(windows)]
mod people;
#[cfg(windows)]
pub mod process;
pub mod progress;
pub mod ram;
#[cfg(windows)]
pub mod session;
pub mod text;
pub mod typing;
