//! Deadline-driven equivalents of the C# `AsyncThrottler` uses (pure timing logic; the audio
//! thread polls `deadline()` and calls `take_due` when it passes).

use std::time::{Duration, Instant};

/// `VolumeThrottle` over `AsyncThrottler(cooldown)`: the first write after an idle period runs at
/// once; later writes inside the cooldown replace the pending value (latest wins) and run when the
/// cooldown since the previous write has elapsed.
#[derive(Debug)]
pub struct WriteThrottle<T> {
    cooldown: Duration,
    last_run: Option<Instant>,
    pending: Option<(T, Instant)>,
}

impl<T> WriteThrottle<T> {
    pub const fn new(cooldown: Duration) -> Self {
        Self { cooldown, last_run: None, pending: None }
    }

    /// Queues `value`, replacing any pending one (which keeps its original due time).
    pub fn request(&mut self, value: T, now: Instant) {
        let due = match (&self.pending, self.last_run) {
            (Some((_, due)), _) => *due,
            (None, Some(last)) => now.max(last + self.cooldown),
            (None, None) => now,
        };
        self.pending = Some((value, due));
    }

    /// When the pending value may be written (None when nothing is pending).
    pub fn deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|(_, due)| *due)
    }

    /// Returns the pending value when its deadline passed and records the write time.
    pub fn take_due(&mut self, now: Instant) -> Option<T> {
        match self.pending {
            Some((_, due)) if now >= due => {
                self.last_run = Some(now);
                self.pending.take().map(|(value, _)| value)
            }
            _ => None,
        }
    }

    /// `VolumeThrottle.Drop`: discards the pending write.
    pub fn drop_pending(&mut self) {
        self.pending = None;
    }
}

/// Trailing debounce (`AsyncThrottler` payload that dwells, then bails on `HasReplacement`): fires
/// `dwell` after the most recent `schedule`.
#[derive(Debug, Default)]
pub struct Debounce {
    deadline: Option<Instant>,
}

impl Debounce {
    pub fn schedule(&mut self, now: Instant, dwell: Duration) {
        self.deadline = Some(now + dwell);
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// True once when the deadline passed.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.deadline {
            Some(deadline) if now >= deadline => {
                self.deadline = None;
                true
            }
            _ => false,
        }
    }
}

/// Fixed-rate schedule without backlog: after a late tick the next one is one interval from now
/// (C# AutoReset timer + `_peakSampleTickActive` overlap guard: missed ticks are skipped).
#[derive(Debug, Default)]
pub struct Ticker {
    next: Option<Instant>,
    interval: Duration,
}

impl Ticker {
    pub fn start(&mut self, now: Instant, interval: Duration) {
        self.interval = interval;
        self.next = Some(now);
    }

    pub fn stop(&mut self) {
        self.next = None;
    }

    pub fn is_running(&self) -> bool {
        self.next.is_some()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.next
    }

    /// True when a tick is due; advances the schedule.
    pub fn take_due(&mut self, now: Instant) -> bool {
        let Some(next) = self.next else { return false };
        if now < next {
            return false;
        }
        let mut following = next + self.interval;
        if following <= now {
            following = now + self.interval;
        }
        self.next = Some(following);
        true
    }
}

/// `1000 / MeterPeakSampleRate` ms with the C# `rate < 1 → 1` guard.
pub fn sample_interval(rate: i32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(rate.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn first_write_is_immediate_then_cooldown_latest_wins() {
        let t0 = Instant::now();
        let mut throttle = WriteThrottle::new(30 * MS);
        assert!(throttle.deadline().is_none());

        throttle.request(0.1_f32, t0);
        assert_eq!(throttle.deadline(), Some(t0));
        assert_eq!(throttle.take_due(t0), Some(0.1));

        // Inside the cooldown: replaced values collapse into the latest one.
        throttle.request(0.2, t0 + 5 * MS);
        throttle.request(0.3, t0 + 8 * MS);
        assert_eq!(throttle.deadline(), Some(t0 + 30 * MS));
        assert_eq!(throttle.take_due(t0 + 10 * MS), None);
        assert_eq!(throttle.take_due(t0 + 30 * MS), Some(0.3));
        assert_eq!(throttle.deadline(), None);

        // After an idle period longer than the cooldown, a write is due at once.
        throttle.request(0.4, t0 + 100 * MS);
        assert_eq!(throttle.deadline(), Some(t0 + 100 * MS));
        assert_eq!(throttle.take_due(t0 + 100 * MS), Some(0.4));
    }

    #[test]
    fn dropped_write_never_runs() {
        let t0 = Instant::now();
        let mut throttle = WriteThrottle::new(30 * MS);
        assert_eq!(throttle.take_due(t0), None);
        throttle.request(1, t0);
        throttle.drop_pending();
        assert_eq!(throttle.deadline(), None);
        assert_eq!(throttle.take_due(t0 + 100 * MS), None);
    }

    #[test]
    fn debounce_fires_after_the_last_schedule() {
        let t0 = Instant::now();
        let mut debounce = Debounce::default();
        debounce.schedule(t0, 50 * MS);
        debounce.schedule(t0 + 40 * MS, 50 * MS);
        assert!(!debounce.take_due(t0 + 60 * MS));
        assert!(debounce.take_due(t0 + 90 * MS));
        assert!(!debounce.take_due(t0 + 200 * MS));
    }

    #[test]
    fn ticker_skips_missed_ticks_instead_of_bursting() {
        let t0 = Instant::now();
        let mut ticker = Ticker::default();
        ticker.start(t0, 10 * MS);
        assert!(ticker.take_due(t0));
        assert!(!ticker.take_due(t0 + 5 * MS));
        assert!(ticker.take_due(t0 + 10 * MS));
        assert_eq!(ticker.deadline(), Some(t0 + 20 * MS));
        // Late by several intervals: one tick, next one interval later.
        assert!(ticker.take_due(t0 + 55 * MS));
        assert_eq!(ticker.deadline(), Some(t0 + 65 * MS));
        assert!(!ticker.take_due(t0 + 60 * MS));
        ticker.stop();
        assert!(!ticker.take_due(t0 + 1000 * MS));
    }

    #[test]
    fn sample_interval_guards_rate() {
        assert_eq!(sample_interval(0), Duration::from_secs(1));
        assert_eq!(sample_interval(-5), Duration::from_secs(1));
        assert_eq!(sample_interval(1000), Duration::from_millis(1));
        let ninety = sample_interval(90).as_secs_f64();
        assert!((ninety - 1.0 / 90.0).abs() < 1e-9);
    }
}
