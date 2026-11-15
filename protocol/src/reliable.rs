//! Events that must arrive: hits, touches on objects, coins and chat.
//!
//! UDP may lose a packet, and a poor connection loses several in a row, so an event is sent again
//! in every packet of its stream until the receiver's acknowledgement reaches it. Each stream
//! numbers its events; a receiver takes one only if its number is newer than the last it took,
//! which makes repeats harmless, and acknowledges that last number whenever a packet of the
//! stream arrives, so a lost acknowledgement is made good by the next repeat.

use crate::is_newer_id;
use std::collections::VecDeque;

/// The streams of events, each numbered apart in each direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// `Hits` to the server, `Hurts` from it.
    Attacks,
    /// `Interactions` to the server; nothing comes back but who owns what.
    Interactions,
    /// `Coins` to the server, `CoinsCollected` from it.
    Coins,
    /// `Say` to the server, `Said` from it.
    Chat,
}

impl Stream {
    pub const ALL: [Self; 4] = [Self::Attacks, Self::Interactions, Self::Coins, Self::Chat];

    /// The stream's code on the wire.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Attacks => 1,
            Self::Interactions => 2,
            Self::Coins => 3,
            Self::Chat => 4,
        }
    }

    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Attacks),
            2 => Some(Self::Interactions),
            3 => Some(Self::Coins),
            4 => Some(Self::Chat),
            _ => None,
        }
    }
}

/// The receiver's word that it has taken every event of `stream` up to number `id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    pub stream: Stream,
    pub id: u16,
}

/// The most events a stream keeps for a receiver that does not acknowledge them; the oldest go
/// first. A receiver silent for long enough to fill it is dropped from the session anyway.
pub const OUTBOX_MOST: usize = 256;

/// One stream's events on their way out: each stays until acknowledged.
#[derive(Clone, Debug)]
pub struct Outbox<T> {
    waiting: VecDeque<(u16, T)>,
    next_id: u16,
}

impl<T> Default for Outbox<T> {
    fn default() -> Self {
        Self {
            waiting: VecDeque::new(),
            next_id: 0,
        }
    }
}

impl<T: Clone> Outbox<T> {
    /// Adds the event `make` builds from the number it is given.
    pub fn push(&mut self, make: impl FnOnce(u16) -> T) {
        let id = self.next_id;
        self.next_id = id.wrapping_add(1);
        if self.waiting.len() >= OUTBOX_MOST {
            self.waiting.pop_front();
        }
        self.waiting.push_back((id, make(id)));
    }

    /// The oldest events not yet acknowledged, up to `most`, for the next packet. Always the
    /// oldest, so the receiver, which takes only newer numbers, never passes one over.
    #[must_use]
    pub fn batch(&self, most: usize) -> Vec<T> {
        let events = self.waiting.iter().take(most);
        events.map(|(_, event)| event.clone()).collect()
    }

    /// Forgets every event up to number `id`, which the receiver has taken.
    pub fn acknowledge(&mut self, id: u16) {
        self.waiting
            .retain(|&(waiting, _)| is_newer_id(waiting, Some(id)));
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.waiting.len()
    }
}

/// One stream's events on their way in: which number was taken last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inbox {
    last: Option<u16>,
}

impl Inbox {
    /// Whether the event numbered `id` is new; if so, it is taken.
    pub fn take(&mut self, id: u16) -> bool {
        let is_new = is_newer_id(id, self.last);
        if is_new {
            self.last = Some(id);
        }
        is_new
    }

    /// What to acknowledge when a packet of `stream` has arrived, once anything has been taken.
    #[must_use]
    pub fn ack(&self, stream: Stream) -> Option<Ack> {
        self.last.map(|id| Ack { stream, id })
    }
}

#[cfg(test)]
mod tests {
    use super::{Inbox, OUTBOX_MOST, Outbox, Stream};

    #[test]
    fn an_event_is_sent_until_it_is_acknowledged() {
        let mut out: Outbox<u16> = Outbox::default();
        out.push(|id| id + 100);
        out.push(|id| id + 100);
        assert_eq!(out.batch(8), [100, 101]);
        // Nothing heard: the same again.
        assert_eq!(out.batch(8), [100, 101]);
        out.acknowledge(0);
        assert_eq!(out.batch(8), [101]);
        // A late acknowledgement of an earlier number changes nothing.
        out.acknowledge(0);
        assert_eq!(out.batch(8), [101]);
        out.acknowledge(1);
        assert!(out.is_empty());
    }

    #[test]
    fn a_packet_carries_the_oldest_first_so_none_is_passed_over() {
        let mut out: Outbox<u16> = Outbox::default();
        let mut inbox = Inbox::default();
        for _ in 0..12 {
            out.push(|id| id);
        }
        // The first packet is lost; the second is the same eight.
        assert_eq!(out.batch(8), (0..8).collect::<Vec<_>>());
        let taken: Vec<u16> = out
            .batch(8)
            .into_iter()
            .filter(|&id| inbox.take(id))
            .collect();
        assert_eq!(taken, (0..8).collect::<Vec<_>>());
        // Repeats are not taken twice, and are acknowledged all the same.
        assert!(out.batch(8).into_iter().all(|id| !inbox.take(id)));
        let ack = inbox.ack(Stream::Coins).map(|ack| ack.id);
        assert_eq!(ack, Some(7));
        out.acknowledge(7);
        assert_eq!(out.batch(8), [8, 9, 10, 11]);
    }

    #[test]
    fn numbers_wrap_round() {
        let mut out: Outbox<u16> = Outbox::default();
        let mut inbox = Inbox::default();
        for _ in 0..=u16::MAX {
            out.push(|id| id);
            let sent = out.batch(1);
            assert!(sent.iter().all(|&id| inbox.take(id)));
            out.acknowledge(sent.first().copied().unwrap_or_default());
        }
        out.push(|id| id);
        assert_eq!(out.batch(1), [0]);
        assert!(inbox.take(0));
    }

    #[test]
    fn a_receiver_that_never_answers_cannot_grow_the_queue() {
        let mut out: Outbox<u16> = Outbox::default();
        for _ in 0..OUTBOX_MOST + 10 {
            out.push(|id| id);
        }
        assert_eq!(out.len(), OUTBOX_MOST);
        assert_eq!(out.batch(1), [10]);
    }

    #[test]
    fn nothing_is_acknowledged_before_anything_is_taken() {
        assert_eq!(Inbox::default().ack(Stream::Chat), None);
        for stream in Stream::ALL {
            assert_eq!(Stream::from_code(stream.code()), Some(stream));
        }
        assert_eq!(Stream::from_code(0), None);
    }
}
