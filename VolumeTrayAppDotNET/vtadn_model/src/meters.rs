//! Raw peak samples shared between the meter sampler (audio service, `MeterPeakSampleRate` Hz while
//! the flyout is visible) and the flyout render tick (`MeterPeakFps` Hz), which interpolates with
//! `policy::meter::MeterLerp`.
//!
//! Values are `(min, max)` channel peaks 0..1 after `MeterReader` weighting (unified meter: both
//! equal `(lo*M + hi)/(M+1)`). Groups aggregate their sessions: (max of session mins, max of
//! session maxes). Pinned-to-silence endpoints publish `(0, 0)`.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::snapshot::{AppId, DeviceId};

#[derive(Debug, Default)]
pub struct MeterFrame {
    /// Incremented by every [`MeterBoard::write`]: a changed value means "new sample".
    pub seq: u64,
    pub devices: HashMap<DeviceId, (f32, f32)>,
    pub groups: HashMap<(DeviceId, AppId), (f32, f32)>,
}

#[derive(Debug, Default)]
pub struct MeterBoard {
    frame: Mutex<MeterFrame>,
}

impl MeterBoard {
    /// Sampler side: mutate the frame in place (reuse its maps), then `seq` is incremented.
    pub fn write(&self, f: impl FnOnce(&mut MeterFrame)) {
        let mut frame = self.frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut frame);
        frame.seq = frame.seq.wrapping_add(1);
    }

    /// UI side: read the latest frame.
    pub fn read<R>(&self, f: impl FnOnce(&MeterFrame) -> R) -> R {
        let frame = self.frame.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&frame)
    }
}
