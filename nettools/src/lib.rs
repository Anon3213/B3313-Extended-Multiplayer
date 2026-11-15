//! Network test tools for B3313 sessions.
//!
//! `netsim` makes a bad connection on purpose, so loss and jitter are tested before players meet
//! them. The load test lives in `src/bin/loadtest.rs`.

pub mod motion;
pub mod netsim;
pub mod options;
