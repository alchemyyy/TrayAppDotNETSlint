//! Slider echo suppression (the `isUserAdjusting` / `hasDeferredVolume` closures of
//! `BuildDeviceSliderRow` / `BuildAppSliderRow`) plus the asynchronous-model guard the C# app did
//! not need: the audio service applies a volume write to its published model at once, but a
//! snapshot published before the write reached the service can still arrive after the UI moved on.
//! Values the UI wrote are remembered until the model shows the newest of them; an older one is a
//! stale echo and is ignored, any other value is an external change and wins.

/// Most write values remembered per slider; older ones are dropped (the newest decides).
const MAX_PENDING_WRITES: usize = 64;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VolumeSlot {
    /// The value the slider shows (endpoint scalar or session level).
    pub shown: f32,
    /// A drag is in progress: model updates are deferred until it ends.
    pub adjusting: bool,
    /// A model update arrived while adjusting.
    pub deferred: bool,
    pending: Vec<f32>,
}

impl VolumeSlot {
    pub fn new(model: f32) -> Self {
        Self { shown: model, ..Self::default() }
    }

    /// The user moved the slider to `value` and it was sent to the service.
    pub fn user_wrote(&mut self, value: f32) {
        self.shown = value;
        if self.pending.len() == MAX_PENDING_WRITES {
            self.pending.remove(0);
        }
        self.pending.push(value);
    }

    /// A snapshot reports `model`; returns the value to show.
    pub fn reconcile(&mut self, model: f32) -> f32 {
        if self.adjusting {
            if model != self.shown {
                self.deferred = true;
            }
            return self.shown;
        }
        if let Some(&newest) = self.pending.last() {
            if model == newest {
                self.pending.clear();
            } else if self.pending.contains(&model) {
                return self.shown;
            } else {
                self.pending.clear();
            }
        }
        self.shown = model;
        model
    }

    /// `UserAdjustmentStarted`.
    pub fn begin_adjust(&mut self) {
        self.adjusting = true;
    }

    /// `UserAdjustmentCompleted`: returns true when a deferred model value should now be applied
    /// (call [`VolumeSlot::reconcile`] with the latest model).
    pub fn end_adjust(&mut self) -> bool {
        self.adjusting = false;
        std::mem::take(&mut self.deferred)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_changes_apply_outside_adjustment() {
        let mut slot = VolumeSlot::new(0.5);
        assert_eq!(slot.reconcile(0.3), 0.3);
        assert_eq!(slot.shown, 0.3);
    }

    #[test]
    fn drag_defers_model_updates_until_it_ends() {
        let mut slot = VolumeSlot::new(0.5);
        slot.begin_adjust();
        slot.user_wrote(0.6);
        assert_eq!(slot.reconcile(0.55), 0.6);
        assert_eq!(slot.reconcile(0.6), 0.6);
        assert!(slot.end_adjust());
        assert!(!slot.end_adjust());
        assert_eq!(slot.reconcile(0.6), 0.6);
    }

    #[test]
    fn stale_echoes_are_ignored_and_the_newest_write_settles() {
        let mut slot = VolumeSlot::new(0.5);
        slot.user_wrote(0.52);
        slot.user_wrote(0.54);
        // A snapshot from before the writes, then one carrying the first write.
        assert_eq!(slot.reconcile(0.52), 0.54);
        assert_eq!(slot.reconcile(0.54), 0.54);
        // Settled: the next value is an ordinary model change.
        assert_eq!(slot.reconcile(0.52), 0.52);
    }

    #[test]
    fn an_external_change_wins_over_pending_writes() {
        let mut slot = VolumeSlot::new(0.5);
        slot.user_wrote(0.52);
        assert_eq!(slot.reconcile(0.9), 0.9);
        assert_eq!(slot.reconcile(0.52), 0.52);
    }

    #[test]
    fn pending_writes_are_bounded() {
        let mut slot = VolumeSlot::new(0.0);
        for i in 0..100 {
            slot.user_wrote(i as f32 / 100.0);
        }
        assert_eq!(slot.pending.len(), MAX_PENDING_WRITES);
        assert_eq!(slot.reconcile(0.99), 0.99);
    }
}
