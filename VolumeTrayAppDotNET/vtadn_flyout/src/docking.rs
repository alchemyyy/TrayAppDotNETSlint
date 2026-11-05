//! Dock state, position persistence and snap-to-redock (port of `FlyoutDockingController` and
//! `FlyoutWindowDragHelper`). All positions are physical screen pixels.

/// The persisted settings a dockable flyout reads and writes (`IFlyoutDockSettings`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DockSettings {
    pub allow_undock: bool,
    pub restore_undocked_on_startup: bool,
    pub clamp_undocked_to_screen: bool,
    pub undocked: bool,
    pub has_saved_position: bool,
    pub left: f64,
    pub top: f64,
}

/// A persisted docking transition (`FlyoutDockStateChange`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockChange {
    Undocked,
    UndockedFromDrag,
    Redocked,
    PositionSaved,
}

/// What the controller needs from its window (`FlyoutDockingOptions`).
pub trait DockHost {
    fn dock_settings(&self) -> DockSettings;
    /// Writes the runtime dock fields (`SetState`, persisted without a settings-changed event).
    fn save_dock_settings(&self, settings: DockSettings);
    fn current_position(&self) -> (i32, i32);
    fn set_position(&self, position: (i32, i32));
    fn docked_position(&self) -> (i32, i32);
    /// Clamps a saved floating position into the monitor that holds it.
    fn clamp_saved_position(&self, position: (i32, i32)) -> (i32, i32);
    fn snap_tolerance(&self) -> i32;
}

/// Pixel-space drag math with snap back to the docked position (`FlyoutWindowDragHelper`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DragHelper {
    grab_offset: (i32, i32),
    start_position: (i32, i32),
    docked_position: (i32, i32),
    snap_tolerance: i32,
    is_currently_snapped: bool,
}

impl DragHelper {
    pub fn is_currently_snapped(&self) -> bool {
        self.is_currently_snapped
    }

    pub fn begin_drag(&mut self, pointer: (i32, i32), window: (i32, i32), docked: (i32, i32), snap_tolerance: i32) {
        self.grab_offset = (pointer.0 - window.0, pointer.1 - window.1);
        self.start_position = window;
        self.docked_position = docked;
        self.snap_tolerance = snap_tolerance.max(0);
        self.is_currently_snapped = self.is_within_snap_tolerance(window);
    }

    /// Window position that keeps the grab point under the pointer.
    pub fn compute_natural(&self, pointer: (i32, i32)) -> (i32, i32) {
        (pointer.0 - self.grab_offset.0, pointer.1 - self.grab_offset.1)
    }

    pub fn exceeds_threshold(&self, natural: (i32, i32), threshold: f64) -> bool {
        let dx = f64::from(natural.0 - self.start_position.0);
        let dy = f64::from(natural.1 - self.start_position.1);
        dx * dx + dy * dy >= threshold * threshold
    }

    /// The position to place the window at for `natural`: the docked position while within the snap
    /// tolerance of it.
    pub fn apply_drag_position(&mut self, natural: (i32, i32)) -> (i32, i32) {
        if self.is_within_snap_tolerance(natural) {
            self.is_currently_snapped = true;
            return self.docked_position;
        }
        self.is_currently_snapped = false;
        natural
    }

    fn is_within_snap_tolerance(&self, position: (i32, i32)) -> bool {
        (position.0 - self.docked_position.0).abs() <= self.snap_tolerance
            && (position.1 - self.docked_position.1).abs() <= self.snap_tolerance
    }
}

/// `FlyoutDockingController`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DockingController {
    is_undocked: bool,
    pub drag: DragHelper,
}

impl DockingController {
    pub fn new(settings: &DockSettings) -> Self {
        Self { is_undocked: Self::should_restore_on_startup(settings), drag: DragHelper::default() }
    }

    /// Persisted settings request a visible undocked flyout at startup.
    pub fn should_restore_on_startup(settings: &DockSettings) -> bool {
        settings.allow_undock
            && settings.restore_undocked_on_startup
            && settings.undocked
            && settings.has_saved_position
    }

    pub fn is_undocked(&self) -> bool {
        self.is_undocked
    }

    /// The saved position while undocked with one, else the docked position.
    pub fn resolve_position(&self, host: &dyn DockHost) -> (i32, i32) {
        let settings = host.dock_settings();
        if !self.is_undocked || !settings.has_saved_position {
            return host.docked_position();
        }
        Self::resolved_saved_position(host, &settings)
    }

    /// The dock target and snap tolerance at the start of a drag.
    pub fn capture_docked_position(&self, host: &dyn DockHost) -> ((i32, i32), i32) {
        (host.docked_position(), host.snap_tolerance().max(0))
    }

    pub fn toggle_undocked(&mut self, host: &dyn DockHost) -> Option<DockChange> {
        if self.is_undocked { self.redock(host) } else { self.undock_to_saved_position(host) }
    }

    pub fn redock(&mut self, host: &dyn DockHost) -> Option<DockChange> {
        if !self.is_undocked {
            return None;
        }
        self.is_undocked = false;
        let mut settings = host.dock_settings();
        settings.undocked = false;
        host.save_dock_settings(settings);
        Some(DockChange::Redocked)
    }

    /// Undocks and moves to the saved floating position when one exists.
    pub fn undock_to_saved_position(&mut self, host: &dyn DockHost) -> Option<DockChange> {
        let mut settings = host.dock_settings();
        if self.is_undocked || !settings.allow_undock {
            return None;
        }
        self.is_undocked = true;
        settings.undocked = true;
        host.save_dock_settings(settings);
        if settings.has_saved_position {
            host.set_position(Self::resolved_saved_position(host, &settings));
        }
        Some(DockChange::Undocked)
    }

    /// The drag threshold was crossed: undock without persisting until the drag commits.
    pub fn set_undocked_from_drag(&mut self, host: &dyn DockHost) -> Option<DockChange> {
        if self.is_undocked || !host.dock_settings().allow_undock {
            return None;
        }
        self.is_undocked = true;
        Some(DockChange::UndockedFromDrag)
    }

    /// Redocks a snapped drag or persists its floating position.
    pub fn commit_drag_position(&mut self, host: &dyn DockHost) -> Option<DockChange> {
        let mut settings = host.dock_settings();
        if !settings.allow_undock || self.drag.is_currently_snapped() {
            return self.redock(host);
        }
        if !self.is_undocked {
            return None;
        }
        let (x, y) = host.current_position();
        settings.undocked = true;
        settings.has_saved_position = true;
        settings.left = f64::from(x);
        settings.top = f64::from(y);
        host.save_dock_settings(settings);
        Some(DockChange::PositionSaved)
    }

    pub fn redock_if_undocking_disabled(&mut self, host: &dyn DockHost) -> Option<DockChange> {
        if host.dock_settings().allow_undock { None } else { self.redock(host) }
    }

    fn resolved_saved_position(host: &dyn DockHost, settings: &DockSettings) -> (i32, i32) {
        let saved = crate::placement::saved_position(settings.left, settings.top);
        if settings.clamp_undocked_to_screen { host.clamp_saved_position(saved) } else { saved }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    struct Harness {
        settings: RefCell<DockSettings>,
        saves: Cell<u32>,
        position: Cell<(i32, i32)>,
    }

    impl Harness {
        fn new(f: impl FnOnce(&mut DockSettings)) -> Self {
            let mut settings = DockSettings {
                allow_undock: true,
                restore_undocked_on_startup: true,
                clamp_undocked_to_screen: true,
                ..DockSettings::default()
            };
            f(&mut settings);
            Self { settings: RefCell::new(settings), saves: Cell::new(0), position: Cell::new((100, 100)) }
        }
    }

    impl DockHost for Harness {
        fn dock_settings(&self) -> DockSettings {
            *self.settings.borrow()
        }
        fn save_dock_settings(&self, settings: DockSettings) {
            *self.settings.borrow_mut() = settings;
            self.saves.set(self.saves.get() + 1);
        }
        fn current_position(&self) -> (i32, i32) {
            self.position.get()
        }
        fn set_position(&self, position: (i32, i32)) {
            self.position.set(position);
        }
        fn docked_position(&self) -> (i32, i32) {
            (100, 100)
        }
        fn clamp_saved_position(&self, position: (i32, i32)) -> (i32, i32) {
            (position.0 + 10, position.1 + 20)
        }
        fn snap_tolerance(&self) -> i32 {
            20
        }
    }

    #[test]
    fn constructor_restores_only_complete_undocked_state() {
        let restored = Harness::new(|s| {
            s.undocked = true;
            s.has_saved_position = true;
        });
        let missing_position = Harness::new(|s| s.undocked = true);
        assert!(DockingController::new(&restored.dock_settings()).is_undocked());
        assert!(!DockingController::new(&missing_position.dock_settings()).is_undocked());
        let disallowed = Harness::new(|s| {
            s.undocked = true;
            s.has_saved_position = true;
            s.allow_undock = false;
        });
        assert!(!DockingController::new(&disallowed.dock_settings()).is_undocked());
    }

    #[test]
    fn explicit_undock_persists_state_and_restores_resolved_position() {
        let harness = Harness::new(|s| {
            s.has_saved_position = true;
            s.left = 400.0;
            s.top = 300.0;
        });
        let mut controller = DockingController::new(&harness.dock_settings());
        assert_eq!(controller.undock_to_saved_position(&harness), Some(DockChange::Undocked));
        assert!(controller.is_undocked());
        assert!(harness.dock_settings().undocked);
        assert_eq!(harness.position.get(), (410, 320));
        assert_eq!(harness.saves.get(), 1);
        assert_eq!(controller.resolve_position(&harness), (410, 320));
    }

    #[test]
    fn disabled_clamping_restores_raw_saved_position() {
        let harness = Harness::new(|s| {
            s.clamp_undocked_to_screen = false;
            s.has_saved_position = true;
            s.left = 400.0;
            s.top = 300.0;
        });
        let mut controller = DockingController::new(&harness.dock_settings());
        controller.undock_to_saved_position(&harness);
        assert_eq!(harness.position.get(), (400, 300));
    }

    #[test]
    fn undock_without_saved_position_stays_in_place() {
        let harness = Harness::new(|_| {});
        let mut controller = DockingController::new(&harness.dock_settings());
        harness.position.set((7, 8));
        assert_eq!(controller.toggle_undocked(&harness), Some(DockChange::Undocked));
        assert_eq!(harness.position.get(), (7, 8));
        // Undocked without a saved position still resolves to the docked position.
        assert_eq!(controller.resolve_position(&harness), (100, 100));
        assert_eq!(controller.toggle_undocked(&harness), Some(DockChange::Redocked));
        assert!(!harness.dock_settings().undocked);
        assert_eq!(harness.saves.get(), 2);
    }

    #[test]
    fn drag_undock_defers_persistence_until_release() {
        let harness = Harness::new(|_| {});
        let mut controller = DockingController::new(&harness.dock_settings());
        controller.drag.begin_drag((0, 0), (200, 200), (0, 0), 20);
        assert_eq!(controller.set_undocked_from_drag(&harness), Some(DockChange::UndockedFromDrag));
        assert!(controller.is_undocked());
        assert!(!harness.dock_settings().undocked);
        assert_eq!(harness.saves.get(), 0);

        harness.position.set((700, 500));
        assert_eq!(controller.commit_drag_position(&harness), Some(DockChange::PositionSaved));
        let settings = harness.dock_settings();
        assert!(settings.undocked && settings.has_saved_position);
        assert_eq!((settings.left, settings.top), (700.0, 500.0));
        assert_eq!(harness.saves.get(), 1);
    }

    #[test]
    fn snapped_drag_redocks_without_saving_floating_coordinates() {
        let harness = Harness::new(|_| {});
        let mut controller = DockingController::new(&harness.dock_settings());
        controller.drag.begin_drag((110, 110), (100, 100), (100, 100), 20);
        controller.set_undocked_from_drag(&harness);
        assert_eq!(controller.commit_drag_position(&harness), Some(DockChange::Redocked));
        assert!(!controller.is_undocked());
        let settings = harness.dock_settings();
        assert!(!settings.undocked && !settings.has_saved_position);
        assert_eq!(harness.saves.get(), 1);
    }

    #[test]
    fn disabled_undocking_rejects_explicit_and_drag_transitions() {
        let harness = Harness::new(|s| s.allow_undock = false);
        let mut controller = DockingController::new(&harness.dock_settings());
        assert_eq!(controller.undock_to_saved_position(&harness), None);
        assert_eq!(controller.set_undocked_from_drag(&harness), None);
        assert!(!controller.is_undocked());
        assert_eq!(harness.saves.get(), 0);
    }

    #[test]
    fn disabling_undock_redocks_once() {
        let harness = Harness::new(|_| {});
        let mut controller = DockingController::new(&harness.dock_settings());
        controller.undock_to_saved_position(&harness);
        assert_eq!(controller.redock_if_undocking_disabled(&harness), None);
        harness.settings.borrow_mut().allow_undock = false;
        assert_eq!(controller.redock_if_undocking_disabled(&harness), Some(DockChange::Redocked));
        assert_eq!(controller.redock_if_undocking_disabled(&harness), None);
    }

    #[test]
    fn drag_helper_snaps_within_tolerance_and_tracks_threshold() {
        let mut drag = DragHelper::default();
        drag.begin_drag((520, 610), (500, 600), (500, 600), 21);
        assert!(drag.is_currently_snapped());
        let natural = drag.compute_natural((523, 612));
        assert_eq!(natural, (503, 602));
        assert!(!drag.exceeds_threshold(natural, 4.0));
        assert!(drag.exceeds_threshold((504, 600), 4.0));
        assert_eq!(drag.apply_drag_position((515, 590)), (500, 600));
        assert!(drag.is_currently_snapped());
        assert_eq!(drag.apply_drag_position((400, 590)), (400, 590));
        assert!(!drag.is_currently_snapped());
    }
}
