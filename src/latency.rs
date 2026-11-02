//! angel-niri: less input lag.
//!
//! **Late compose** (KWin's render-time estimation, Sway's `max_render_time`): a queued redraw
//! waits until just before the VBlank it aims at, so a client buffer or a pointer move that lands
//! in the wait still makes this refresh instead of the next one. The budget is the slowest recent
//! redraw plus a margin; a missed VBlank widens it, and if that keeps happening the output goes
//! back to drawing at once for a while. Only on a real display (TTY), never with VRR or while a
//! game tears (those flip at once anyway), and only while the output keeps presenting (an idle
//! output's VBlank estimate drifts). `niri msg action set-late-compose --on false` turns it off.
//!
//! Not done, measured on the stand (02.11): the compositor thread realtime like KWin (SCHED_RR).
//! Under sched_ext cosmos a normal thread already kept every frame on time with 16 busy loops on
//! its CPU, while a realtime one stalled for 70–370 ms now and then, spinning on a driver thread
//! it starved.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use smithay::output::Output;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::niri::Niri;
use crate::utils::get_monotonic_time;

static LATE: AtomicBool = AtomicBool::new(true);

pub fn set_late_compose(on: bool) {
    LATE.store(on, Ordering::Relaxed);
}

/// Room on top of the slowest recent redraw: the timer waking up, the flip being committed.
const MARGIN: Duration = Duration::from_micros(1500);
/// Not worth a timer when composing would start this soon anyway.
const MIN_WAIT: Duration = Duration::from_micros(300);
/// The VBlank estimate is trusted this long after the last real one.
const FRESH: Duration = Duration::from_millis(100);
/// After the learned room grows past what late composing can win, how long to draw at once.
const BACK_OFF: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
pub struct Late {
    /// The slowest recent redraw, sinking slowly towards the usual one.
    cost: Duration,
    /// Extra room learned from missed VBlanks, shrinking while frames make it.
    extra: Duration,
    /// The armed wait ends here.
    until: Option<Duration>,
    /// The VBlank the frame composed late aims at, until it's presented.
    aimed: Option<Duration>,
    /// When the last VBlank came.
    last_vblank: Option<Duration>,
    off_until: Option<Duration>,
    announced: bool,
}

impl Late {
    /// Plans the wait for a redraw queued at `now`: when to compose for `vblank`, if worth it.
    fn plan(&mut self, now: Duration, refresh: Duration, vblank: Duration) -> Option<Duration> {
        self.aimed = None;
        if self.off_until.is_some_and(|t| now < t)
            || !self.last_vblank.is_some_and(|t| now < t + FRESH)
        {
            return None;
        }
        let budget = self.budget();
        if budget * 4 >= refresh * 3 {
            return None;
        }
        let at = vblank.saturating_sub(budget);
        if at < now + MIN_WAIT {
            return None;
        }
        self.until = Some(at);
        self.aimed = Some(vblank);
        Some(at)
    }

    fn budget(&self) -> Duration {
        self.cost + MARGIN + self.extra
    }

    /// A submitted redraw took `took`.
    fn learn(&mut self, took: Duration) {
        self.cost = if took >= self.cost {
            took
        } else {
            self.cost - (self.cost - took) / 32
        };
    }

    /// A VBlank presented at `presented` (zero: unknown). Returns whether the frame composed late
    /// for it came a refresh too late.
    fn judge(&mut self, presented: Duration, refresh: Duration, now: Duration) -> bool {
        self.last_vblank = Some(if presented.is_zero() { now } else { presented });
        let Some(aimed) = self.aimed.take() else {
            return false;
        };
        if presented.is_zero() {
            return false;
        }
        if presented < aimed + refresh / 2 {
            self.extra = self.extra.saturating_sub(Duration::from_micros(5));
            return false;
        }
        self.extra = (self.extra * 2).max(Duration::from_micros(500));
        if self.budget() * 4 >= refresh * 3 {
            self.extra = Duration::ZERO;
            self.cost = Duration::ZERO;
            self.off_until = Some(now + BACK_OFF);
        }
        true
    }
}

impl Niri {
    /// Whether `output`'s queued redraw waits to compose later; arms the timer that ends the wait.
    pub fn compose_wait(&mut self, tty: bool, output: &Output) -> bool {
        if !tty || !LATE.load(Ordering::Relaxed) || self.tearing_now(output) {
            return false;
        }
        let now = get_monotonic_time();
        let state = self.output_state.get_mut(output).unwrap();
        let late = &mut state.late;
        if let Some(until) = late.until {
            if now < until {
                return true;
            }
            late.until = None;
            return false;
        }
        let Some(refresh) = state.frame_clock.refresh_interval() else {
            return false;
        };
        if state.frame_clock.vrr() {
            late.aimed = None;
            return false;
        }
        let vblank = state.frame_clock.next_presentation_time();
        let Some(at) = late.plan(now, refresh, vblank) else {
            return false;
        };

        if !late.announced {
            late.announced = true;
            info!(
                "late compose on {}: frames start {:?} before VBlank",
                output.name(),
                vblank - at
            );
        }
        // The loop redraws the queued outputs after every wake-up, this one included.
        self.event_loop
            .insert_source(Timer::from_duration(at - now), |_, _, _| {
                TimeoutAction::Drop
            })
            .unwrap();
        true
    }

    /// After a redraw that began at `started`: learn how long it took.
    pub fn composed(&mut self, output: &Output, submitted: bool, started: Duration) {
        let Some(state) = self.output_state.get_mut(output) else {
            return;
        };
        if submitted {
            state
                .late
                .learn(get_monotonic_time().saturating_sub(started));
        } else {
            // No flip, no VBlank to judge it by.
            state.late.aimed = None;
        }
    }

    /// A VBlank came: was the frame composed late on time?
    pub fn compose_presented(&mut self, output: &Output, presentation_time: Duration) {
        let Some(state) = self.output_state.get_mut(output) else {
            return;
        };
        let Some(refresh) = state.frame_clock.refresh_interval() else {
            return;
        };
        let late = &mut state.late;
        let now = get_monotonic_time();
        if !late.judge(presentation_time, refresh, now) {
            return;
        }
        // Judged only frames composed late, so a pause from now on is a fresh back-off.
        if late.off_until.is_some_and(|until| until > now) {
            warn!(
                "late compose keeps missing VBlanks on {}, drawing at once for {BACK_OFF:?}",
                output.name()
            );
        } else {
            debug!(
                "late compose missed a VBlank on {}, room now {:?}",
                output.name(),
                late.budget()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);
    // 200 Hz
    const REFRESH: Duration = Duration::from_micros(5000);

    fn presented(late: &mut Late, at: Duration) {
        late.judge(at, REFRESH, at);
    }

    #[test]
    fn waits_until_just_before_vblank() {
        let mut late = Late::default();
        let t = 100 * MS;
        // Stale or no VBlank: compose at once.
        assert_eq!(late.plan(t, REFRESH, t + REFRESH), None);
        presented(&mut late, t);
        late.learn(MS);
        let at = late.plan(t, REFRESH, t + REFRESH).unwrap();
        assert_eq!(at, t + REFRESH - MS - MARGIN);
        // Made it: no miss, nothing learned against it.
        assert!(!late.judge(t + REFRESH, REFRESH, t + REFRESH));
        assert_eq!(late.extra, Duration::ZERO);
    }

    #[test]
    fn misses_widen_the_room_then_back_off() {
        let mut late = Late::default();
        let mut t = 100 * MS;
        presented(&mut late, t);
        late.learn(MS);
        let mut misses = 0;
        while late.plan(t, REFRESH, t + REFRESH).is_some() {
            // Presented a refresh late.
            t += 2 * REFRESH;
            assert!(late.judge(t, REFRESH, t));
            misses += 1;
            assert!(misses < 10);
        }
        assert!(late.off_until.is_some_and(|until| until > t));
        assert!(misses >= 2);
        // Back after the pause.
        t += BACK_OFF;
        presented(&mut late, t);
        assert!(late.plan(t, REFRESH, t + REFRESH).is_some());
    }

    #[test]
    fn slow_redraws_mean_no_wait() {
        let mut late = Late::default();
        let t = 100 * MS;
        presented(&mut late, t);
        late.learn(3 * MS);
        // 3 ms + 1.5 ms margin of a 5 ms refresh: nothing to win.
        assert_eq!(late.plan(t, REFRESH, t + REFRESH), None);
        // The cost sinks slowly as redraws get fast again.
        for _ in 0..200 {
            late.learn(MS / 2);
        }
        assert!(late.plan(t, REFRESH, t + REFRESH).is_some());
    }
}
