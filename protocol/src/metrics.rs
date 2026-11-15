//! Link measurements shared by the bridge, the server and the test tools.
//!
//! Each end keeps a `LinkMeter` per link and turns it into a `LinkReport` every `REPORT_INTERVAL`,
//! so reports from both ends cover comparable periods.

use std::fmt;
use std::time::{Duration, Instant};

/// How often each end reports a link.
pub const REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// Packets and bytes counted in one direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traffic {
    pub packets: u64,
    pub bytes: u64,
}

impl Traffic {
    pub fn add(&mut self, bytes: usize) {
        self.packets += 1;
        self.bytes += u64::try_from(bytes).unwrap_or(u64::MAX);
    }
}

/// Arrivals counted from sequence numbers that the sender raises by one per packet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Losses {
    /// Packets that arrived, in order or late, not counting duplicates of the newest.
    pub received: u64,
    /// Sequence numbers skipped and never seen since.
    pub missing: u64,
    /// Packets that arrived after a newer one, and so were not used.
    pub late: u64,
}

impl Losses {
    /// The share of sent packets that never arrived, from 0 to 1.
    #[must_use]
    pub fn rate(&self) -> f64 {
        let sent = self.received + self.missing;
        if sent == 0 {
            0.0
        } else {
            to_f64(self.missing) / to_f64(sent)
        }
    }
}

/// Tracks the newest sequence number of a link, and counts what went missing on the way.
#[derive(Clone, Copy, Debug, Default)]
pub struct Arrivals {
    newest: Option<u32>,
    losses: Losses,
}

impl Arrivals {
    /// Records an arrival and returns whether it is newer than every earlier one.
    ///
    /// Sequence numbers wrap, so "newer" means less than half the range ahead.
    pub fn record(&mut self, sequence: u32) -> bool {
        let ahead = self
            .newest
            .map_or(1, |newest| sequence.wrapping_sub(newest));
        if ahead == 0 {
            return false;
        }
        self.losses.received += 1;
        if ahead < u32::MAX / 2 {
            if self.newest.is_some() {
                self.losses.missing += u64::from(ahead - 1);
            }
            self.newest = Some(sequence);
            true
        } else {
            // It was counted missing when a newer one skipped past it.
            self.losses.missing = self.losses.missing.saturating_sub(1);
            self.losses.late += 1;
            false
        }
    }

    /// Returns the counts so far and starts new ones, keeping the newest sequence number.
    pub fn take_losses(&mut self) -> Losses {
        std::mem::take(&mut self.losses)
    }
}

/// Durations measured over a period, such as round trips or the gaps between arrivals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub count: u32,
    total: Duration,
    pub min: Duration,
    pub max: Duration,
    /// The sum of differences between consecutive samples.
    variation: Duration,
    last: Option<Duration>,
}

impl Timings {
    pub fn record(&mut self, sample: Duration) {
        if self.count == 0 || sample < self.min {
            self.min = sample;
        }
        self.max = self.max.max(sample);
        self.total += sample;
        self.count += 1;
        if let Some(last) = self.last {
            self.variation += sample.abs_diff(last);
        }
        self.last = Some(sample);
    }

    #[must_use]
    pub fn mean(&self) -> Option<Duration> {
        self.total.checked_div(self.count)
    }

    /// The mean change from one sample to the next, which is what interpolation must absorb.
    #[must_use]
    pub fn jitter(&self) -> Option<Duration> {
        self.variation.checked_div(self.count.checked_sub(1)?)
    }
}

/// Counts everything about one link, from one end.
#[derive(Clone, Copy, Debug)]
pub struct LinkMeter {
    since: Instant,
    sent: Traffic,
    received: Traffic,
    arrivals: Arrivals,
    round_trips: Timings,
    gaps: Timings,
    last_newest: Option<Instant>,
}

impl LinkMeter {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            since: now,
            sent: Traffic::default(),
            received: Traffic::default(),
            arrivals: Arrivals::default(),
            round_trips: Timings::default(),
            gaps: Timings::default(),
            last_newest: None,
        }
    }

    pub fn sent(&mut self, bytes: usize) {
        self.sent.add(bytes);
    }

    pub fn received(&mut self, bytes: usize) {
        self.received.add(bytes);
    }

    /// Records a sequenced arrival at `at`, and returns whether it is the newest so far.
    pub fn arrived(&mut self, sequence: u32, at: Instant) -> bool {
        let newest = self.arrivals.record(sequence);
        if newest {
            if let Some(last) = self.last_newest {
                self.gaps.record(at.saturating_duration_since(last));
            }
            self.last_newest = Some(at);
        }
        newest
    }

    pub fn round_trip(&mut self, sample: Duration) {
        self.round_trips.record(sample);
    }

    /// Returns a report once `REPORT_INTERVAL` has passed, and starts counting the next one.
    pub fn report_if_due(&mut self, now: Instant) -> Option<LinkReport> {
        let elapsed = now.saturating_duration_since(self.since);
        if elapsed < REPORT_INTERVAL {
            return None;
        }
        let report = LinkReport {
            elapsed,
            sent: std::mem::take(&mut self.sent),
            received: std::mem::take(&mut self.received),
            losses: self.arrivals.take_losses(),
            round_trips: std::mem::take(&mut self.round_trips),
            gaps: std::mem::take(&mut self.gaps),
        };
        self.since = now;
        Some(report)
    }
}

/// One link over one reporting period, as one end saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkReport {
    pub elapsed: Duration,
    pub sent: Traffic,
    pub received: Traffic,
    /// Losses on the way in.
    pub losses: Losses,
    pub round_trips: Timings,
    /// Time between consecutive newest arrivals, which interpolation must cover.
    pub gaps: Timings,
}

impl LinkReport {
    /// Bytes per second sent and received.
    #[must_use]
    pub fn byte_rates(&self) -> (f64, f64) {
        (
            self.per_second(self.sent.bytes),
            self.per_second(self.received.bytes),
        )
    }

    fn per_second(&self, count: u64) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if seconds > 0.0 {
            to_f64(count) / seconds
        } else {
            0.0
        }
    }
}

impl fmt::Display for LinkReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (sent, received) = self.byte_rates();
        write!(
            f,
            "sent {sent:.0} B/s ({:.1} packets/s), received {received:.0} B/s ({:.1} packets/s), \
             lost {:.1}% on the way in ({} of {}, {} late)",
            self.per_second(self.sent.packets),
            self.per_second(self.received.packets),
            self.losses.rate() * 100.0,
            self.losses.missing,
            self.losses.received + self.losses.missing,
            self.losses.late,
        )?;
        write_timings(f, "gap between arrivals", &self.gaps)?;
        write_timings(f, "round trip", &self.round_trips)
    }
}

fn write_timings(f: &mut fmt::Formatter<'_>, name: &str, timings: &Timings) -> fmt::Result {
    match (timings.mean(), timings.jitter()) {
        (Some(mean), Some(jitter)) => write!(
            f,
            ", {name} {} (min {}, max {}, jitter {})",
            millis(mean),
            millis(timings.min),
            millis(timings.max),
            millis(jitter),
        ),
        _ => Ok(()),
    }
}

fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// Counts here are of one reporting period and stay far below 2^32, so the conversion is exact;
/// a larger one would be given as that.
fn to_f64(count: u64) -> f64 {
    u32::try_from(count).map_or(f64::from(u32::MAX), f64::from)
}

#[cfg(test)]
mod tests {
    use super::{Arrivals, LinkMeter, Losses, REPORT_INTERVAL, Timings};
    use std::time::{Duration, Instant};

    fn record_all(arrivals: &mut Arrivals, sequences: &[u32]) -> Vec<bool> {
        sequences.iter().map(|&s| arrivals.record(s)).collect()
    }

    #[test]
    fn gaps_count_as_missing_until_the_packet_turns_up_late() {
        let mut arrivals = Arrivals::default();
        let newer = record_all(&mut arrivals, &[1, 2, 5, 3, 6, 6]);

        assert_eq!(newer, [true, true, true, false, true, false]);
        assert_eq!(
            arrivals.take_losses(),
            Losses {
                received: 5,
                missing: 1,
                late: 1
            }
        );
    }

    #[test]
    fn sequence_numbers_wrap_without_counting_a_huge_loss() {
        let mut arrivals = Arrivals::default();
        let newer = record_all(&mut arrivals, &[u32::MAX - 1, u32::MAX, 0, 1]);

        assert_eq!(newer, [true; 4]);
        assert_eq!(arrivals.take_losses().missing, 0);
    }

    #[test]
    fn taking_losses_keeps_the_newest_sequence() {
        let mut arrivals = Arrivals::default();
        arrivals.record(10);
        arrivals.take_losses();

        assert!(!arrivals.record(9));
        assert!(arrivals.record(11));
        assert_eq!(arrivals.take_losses().missing, 0);
    }

    #[test]
    fn loss_rate_is_missing_over_sent() {
        let losses = Losses {
            received: 3,
            missing: 1,
            late: 0,
        };
        assert!((losses.rate() - 0.25).abs() < 1e-9);
        assert!(Losses::default().rate().abs() < 1e-9);
    }

    #[test]
    fn timings_give_range_mean_and_jitter() {
        let mut trips = Timings::default();
        assert_eq!((trips.mean(), trips.jitter()), (None, None));
        for millis in [20, 30, 10] {
            trips.record(Duration::from_millis(millis));
        }

        assert_eq!(trips.min, Duration::from_millis(10));
        assert_eq!(trips.max, Duration::from_millis(30));
        assert_eq!(trips.mean(), Some(Duration::from_millis(20)));
        // Differences of 10 and 20 ms.
        assert_eq!(trips.jitter(), Some(Duration::from_millis(15)));
    }

    #[test]
    fn a_report_covers_one_interval_and_the_next_starts_empty() {
        let start = Instant::now();
        let mut meter = LinkMeter::new(start);
        for _ in 0..20 {
            meter.sent(50);
        }
        meter.received(100);

        assert_eq!(meter.report_if_due(start + REPORT_INTERVAL / 2), None);
        let report = meter.report_if_due(start + REPORT_INTERVAL);
        let (sent, received) = report.map(|report| report.byte_rates()).unwrap_or_default();
        assert!((sent - 100.0).abs() < 1e-9, "sent {sent} B/s");
        assert!((received - 10.0).abs() < 1e-9, "received {received} B/s");
        let later = start + REPORT_INTERVAL * 2;
        assert_eq!(meter.report_if_due(later).map(|r| r.sent.packets), Some(0));
    }

    #[test]
    fn gaps_are_timed_between_newest_arrivals_only() {
        let start = Instant::now();
        let mut meter = LinkMeter::new(start);
        let at = |millis| start + Duration::from_millis(millis);
        meter.arrived(1, at(0));
        meter.arrived(2, at(30));
        // Late, so it does not count as an arrival of anything new.
        meter.arrived(1, at(40));
        meter.arrived(4, at(100));

        let gaps = meter
            .report_if_due(start + REPORT_INTERVAL)
            .map(|report| report.gaps);
        let expected = (2, at(30) - start, at(70) - start);
        assert_eq!(gaps.map(|g| (g.count, g.min, g.max)), Some(expected));
    }
}
