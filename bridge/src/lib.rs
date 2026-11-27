//! Connects a running B3313 emulator to multiplayer sessions.

#[cfg(windows)]
pub mod assist;
pub mod bots;
pub mod client;
pub mod game;
pub mod interpolation;
pub mod json;
pub mod keyboard;
pub mod layout;
pub mod mailbox;
pub mod object_motion;
pub mod objects;
pub mod pacing;
#[cfg(windows)]
mod people;
pub mod pointer;
#[cfg(windows)]
pub mod process;
pub mod progress;
pub mod ram;
#[cfg(windows)]
pub mod session;
pub mod text;
pub mod travel;
pub mod typing;
