//! Coalesced, interaction-aware content rebuilds (port of `FlyoutRebuildScheduler`) and the
//! flyout's rebuild hold rule (`VolumeFlyoutWindow.IsContentInteractionActive` +
//! `IsWindowDragActive`).

use std::cell::Cell;
use std::rc::{Rc, Weak};

/// What the scheduler asks of its owner.
pub trait RebuildHooks {
    /// The window is open and visible, warm-priming, or has no content yet.
    fn can_show_rebuild(&self) -> bool;
    /// An interaction holds the current content.
    fn is_held(&self) -> bool;
    /// Builds and commits a replacement; `Err` keeps the current content.
    fn rebuild(&self) -> Result<(), String>;
    /// Runs after each successful rebuild (`QueuePositionNearTray`).
    fn committed(&self);
}

/// Posts a job to run later on the UI thread (a zero-delay `slint::Timer` in production).
pub type PostFn = Box<dyn Fn(Box<dyn FnOnce()>)>;

pub struct RebuildScheduler {
    hooks: Weak<dyn RebuildHooks>,
    post: PostFn,
    this: Weak<RebuildScheduler>,
    pending: Cell<bool>,
    queued: Cell<bool>,
    rebuilding: Cell<bool>,
    disposed: Cell<bool>,
}

impl RebuildScheduler {
    pub fn new(hooks: Weak<dyn RebuildHooks>, post: PostFn) -> Rc<Self> {
        Rc::new_cyclic(|this| Self {
            hooks,
            post,
            this: this.clone(),
            pending: Cell::new(false),
            queued: Cell::new(false),
            rebuilding: Cell::new(false),
            disposed: Cell::new(false),
        })
    }

    pub fn is_rebuilding(&self) -> bool {
        self.rebuilding.get()
    }

    /// A rebuild was asked for and has not run yet.
    pub fn is_pending(&self) -> bool {
        self.pending.get() || self.queued.get()
    }

    /// Queues one coalesced rebuild.
    pub fn queue(&self) {
        let Some(hooks) = self.live_hooks() else { return };
        if !hooks.can_show_rebuild() || self.rebuilding.get() || hooks.is_held() {
            self.pending.set(true);
            return;
        }
        if self.queued.replace(true) {
            return;
        }
        let this = self.this.clone();
        (self.post)(Box::new(move || {
            let Some(this) = this.upgrade() else { return };
            // A synchronous rebuild since queueing (such as a show) already published this change.
            if this.disposed.get() || !this.queued.get() {
                return;
            }
            this.queued.set(false);
            this.rebuild_now();
        }));
    }

    /// Rebuilds synchronously; returns whether a replacement was committed.
    pub fn rebuild_now(&self) -> bool {
        let Some(hooks) = self.live_hooks() else { return false };
        if self.rebuilding.get() || !hooks.can_show_rebuild() || hooks.is_held() {
            self.pending.set(true);
            return false;
        }
        self.queued.set(false);
        self.pending.set(false);
        self.rebuilding.set(true);
        let result = hooks.rebuild();
        self.rebuilding.set(false);
        if let Err(error) = result {
            log::error!("VolumeFlyoutWindow.Rebuild: {error}");
            // The stale content stays on screen, so the request survives for the next flush, queue or show.
            if !self.disposed.get() {
                self.pending.set(true);
            }
            return false;
        }
        hooks.committed();
        self.flush();
        true
    }

    /// Queues the pending rebuild once nothing holds the content any more.
    pub fn flush(&self) {
        let Some(hooks) = self.live_hooks() else { return };
        if !self.pending.get() || self.rebuilding.get() || hooks.is_held() || !hooks.can_show_rebuild() {
            return;
        }
        self.pending.set(false);
        self.queue();
    }

    pub fn dispose(&self) {
        self.disposed.set(true);
        self.pending.set(false);
        self.queued.set(false);
    }

    fn live_hooks(&self) -> Option<Rc<dyn RebuildHooks>> {
        if self.disposed.get() {
            return None;
        }
        self.hooks.upgrade()
    }
}

/// The interactions that hold rebuilds (`IsContentInteractionActive` + `IsWindowDragActive`).
/// Wheel and keyboard slider adjustments do not hold rebuilds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InteractionState {
    pub slider_drags: u32,
    pub percent_editors: u32,
    pub rename_active: bool,
    pub menu_open: bool,
    pub window_drag: bool,
    pub dialog_open: bool,
}

impl InteractionState {
    pub fn holds_rebuild(&self) -> bool {
        self.slider_drags > 0
            || self.menu_open
            || self.rename_active
            || self.percent_editors > 0
            || self.window_drag
            || self.dialog_open
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct Harness {
        can_show: Cell<bool>,
        held: Cell<bool>,
        failures_left: Cell<u32>,
        attempts: Cell<u32>,
        commits: Cell<u32>,
    }

    impl RebuildHooks for Harness {
        fn can_show_rebuild(&self) -> bool {
            self.can_show.get()
        }
        fn is_held(&self) -> bool {
            self.held.get()
        }
        fn rebuild(&self) -> Result<(), String> {
            self.attempts.set(self.attempts.get() + 1);
            if self.failures_left.get() == 0 {
                return Ok(());
            }
            self.failures_left.set(self.failures_left.get() - 1);
            Err("Simulated build failure.".into())
        }
        fn committed(&self) {
            self.commits.set(self.commits.get() + 1);
        }
    }

    type Jobs = Rc<RefCell<Vec<Box<dyn FnOnce()>>>>;

    fn setup(f: impl FnOnce(&Harness)) -> (Rc<Harness>, Rc<RebuildScheduler>, Jobs) {
        let harness = Rc::new(Harness::default());
        harness.can_show.set(true);
        f(&harness);
        let jobs: Jobs = Rc::default();
        let queue = jobs.clone();
        let hooks: Rc<dyn RebuildHooks> = harness.clone();
        let scheduler = RebuildScheduler::new(Rc::downgrade(&hooks), Box::new(move |job| queue.borrow_mut().push(job)));
        (harness, scheduler, jobs)
    }

    fn run_jobs(jobs: &Jobs) {
        loop {
            let batch: Vec<_> = jobs.borrow_mut().drain(..).collect();
            if batch.is_empty() {
                return;
            }
            for job in batch {
                job();
            }
        }
    }

    #[test]
    fn a_failed_rebuild_stays_pending_and_the_next_flush_retries_it_once() {
        let (harness, scheduler, jobs) = setup(|h| h.failures_left.set(1));
        assert!(!scheduler.rebuild_now());
        assert!(scheduler.is_pending());
        assert_eq!(harness.attempts.get(), 1);
        // The failure itself schedules nothing, so a rebuild that keeps failing cannot spin.
        run_jobs(&jobs);
        assert_eq!(harness.attempts.get(), 1);
        scheduler.flush();
        run_jobs(&jobs);
        assert_eq!(harness.attempts.get(), 2);
        assert_eq!(harness.commits.get(), 1);
        assert!(!scheduler.is_pending());
    }

    #[test]
    fn a_queued_rebuild_that_fails_stays_pending() {
        let (harness, scheduler, jobs) = setup(|h| h.failures_left.set(1));
        scheduler.queue();
        run_jobs(&jobs);
        assert_eq!(harness.attempts.get(), 1);
        assert_eq!(harness.commits.get(), 0);
        assert!(scheduler.is_pending());
    }

    #[test]
    fn a_held_rebuild_waits_and_runs_on_the_flush_after_the_hold_ends() {
        let (harness, scheduler, jobs) = setup(|h| h.held.set(true));
        scheduler.queue();
        assert!(!scheduler.rebuild_now());
        run_jobs(&jobs);
        assert_eq!(harness.attempts.get(), 0);
        assert!(scheduler.is_pending());
        // A flush while the interaction still holds the content keeps waiting.
        scheduler.flush();
        run_jobs(&jobs);
        assert_eq!(harness.attempts.get(), 0);
        harness.held.set(false);
        scheduler.flush();
        run_jobs(&jobs);
        assert_eq!(harness.commits.get(), 1);
        assert!(!scheduler.is_pending());
    }

    #[test]
    fn a_hidden_window_only_marks_its_content_stale() {
        let (harness, scheduler, jobs) = setup(|h| h.can_show.set(false));
        scheduler.queue();
        run_jobs(&jobs);
        assert_eq!(harness.attempts.get(), 0);
        assert!(scheduler.is_pending());
        harness.can_show.set(true);
        assert!(scheduler.rebuild_now());
        assert_eq!(harness.commits.get(), 1);
    }

    #[test]
    fn nothing_rebuilds_after_the_window_closed() {
        let (harness, scheduler, jobs) = setup(|h| h.failures_left.set(1));
        assert!(!scheduler.rebuild_now());
        scheduler.queue();
        scheduler.dispose();
        scheduler.flush();
        assert!(!scheduler.rebuild_now());
        run_jobs(&jobs);
        assert_eq!(harness.attempts.get(), 1);
        assert!(!scheduler.is_pending());
    }

    #[test]
    fn queued_rebuilds_coalesce_and_a_synchronous_rebuild_cancels_the_queued_one() {
        let (harness, scheduler, jobs) = setup(|_| {});
        scheduler.queue();
        scheduler.queue();
        assert_eq!(jobs.borrow().len(), 1);
        assert!(scheduler.rebuild_now());
        run_jobs(&jobs);
        assert_eq!(harness.attempts.get(), 1);
    }

    #[test]
    fn interactions_that_hold_rebuilds() {
        let idle = InteractionState::default();
        assert!(!idle.holds_rebuild());
        for state in [
            InteractionState { slider_drags: 1, ..idle },
            InteractionState { percent_editors: 2, ..idle },
            InteractionState { rename_active: true, ..idle },
            InteractionState { menu_open: true, ..idle },
            InteractionState { window_drag: true, ..idle },
            InteractionState { dialog_open: true, ..idle },
        ] {
            assert!(state.holds_rebuild(), "{state:?}");
        }
    }
}
