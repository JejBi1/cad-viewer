//! Cancellation and time limits of one scene build.
//!
//! A scene build can run for a long time on a small file (an enormous clipping outline, a hatch pattern with a hundred
//! thousand lines). [`Ctl`] is what the drawing code looks at between entities and inside the long loops: the flag of
//! the caller (cancel: the build stops and nothing is kept) and the deadline of the whole scene (the build stops and
//! what has been drawn so far is shown, with a note).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Time each of the two stages of a scene may take: going through all entities (with all their hatches) and making the
/// geometry of what was recorded. Single hatches have their own, shorter limit (`hatchdraw::HATCHING_TIMEOUT`).
pub const SCENE_TIME_LIMIT: Duration = Duration::from_secs(120);

/// Why a build stopped early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    /// The caller asked to stop: the result is not wanted.
    Cancelled,
    /// The time limit of the scene ran out: what is there is shown.
    TimedOut,
}

/// Cancel flag and deadline of a build; cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct Ctl {
    cancel: Option<Arc<AtomicBool>>,
    deadline: Option<Instant>,
}

impl Ctl {
    pub fn new(cancel: Option<Arc<AtomicBool>>, deadline: Option<Instant>) -> Ctl {
        Ctl { cancel, deadline }
    }

    /// No cancel flag, a deadline `limit` from now.
    pub fn with_limit(limit: Duration) -> Ctl {
        Ctl { cancel: None, deadline: Instant::now().checked_add(limit) }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
    }

    pub fn is_expired(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    /// Cancel first (it wins over the time limit), then the deadline.
    pub fn halt(&self) -> Option<Halt> {
        if self.is_cancelled() {
            Some(Halt::Cancelled)
        } else if self.is_expired() {
            Some(Halt::TimedOut)
        } else {
            None
        }
    }

    /// The same flag with a deadline that is at most `limit` from now (a hatch inside a scene).
    pub fn tighter(&self, limit: Duration) -> Ctl {
        let mine = Instant::now().checked_add(limit);
        let deadline = match (self.deadline, mine) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Ctl { cancel: self.cancel.clone(), deadline }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_wins_over_the_deadline_and_the_default_never_stops() {
        assert_eq!(Ctl::default().halt(), None);
        let flag = Arc::new(AtomicBool::new(false));
        let c = Ctl::new(Some(flag.clone()), Some(Instant::now()));
        assert_eq!(c.halt(), Some(Halt::TimedOut));
        flag.store(true, Ordering::Relaxed);
        assert_eq!(c.halt(), Some(Halt::Cancelled));
    }

    #[test]
    fn tighter_keeps_the_earlier_deadline_and_the_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let outer = Ctl::new(Some(flag.clone()), Instant::now().checked_add(Duration::from_secs(1)));
        let long = outer.tighter(Duration::from_secs(3600));
        assert_eq!(long.deadline(), outer.deadline());
        let short = Ctl::default().tighter(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert!(short.is_expired());
        flag.store(true, Ordering::Relaxed);
        assert!(long.is_cancelled());
    }
}
