//! Pure parts of default-device resolution (`AudioDeviceManager.UpdateAllDefaults`,
//! `LookupDefault`, `FindNotifiedDefault`, `PersistLastKnownDefaults`, `FindFallbackDefault`).

use vtadn_model::snapshot::Flow;

/// The two roles the manager tracks (eConsole notifications are ignored).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Multimedia,
    Communications,
}

/// The four (flow, role) slots, in `UpdateAllDefaults` order.
pub const SLOTS: [(Flow, Role); 4] = [
    (Flow::Render, Role::Multimedia),
    (Flow::Render, Role::Communications),
    (Flow::Capture, Role::Multimedia),
    (Flow::Capture, Role::Communications),
];

const fn slot_index(flow: Flow, role: Role) -> usize {
    match (flow, role) {
        (Flow::Render, Role::Multimedia) => 0,
        (Flow::Render, Role::Communications) => 1,
        (Flow::Capture, Role::Multimedia) => 2,
        (Flow::Capture, Role::Communications) => 3,
    }
}

/// What resolution needs to know about one tracked endpoint.
#[derive(Clone, Copy, Debug)]
pub struct EndpointRef<'a> {
    pub id: &'a str,
    pub flow: Flow,
    pub active: bool,
}

/// One id per (flow, role) slot: the `_notified*DeviceID` fields and the persisted
/// `LastKnownDefault*DeviceID` settings share this shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleIds([Option<String>; 4]);

impl RoleIds {
    pub fn new(
        render: Option<String>,
        render_comms: Option<String>,
        capture: Option<String>,
        capture_comms: Option<String>,
    ) -> Self {
        Self([render, render_comms, capture, capture_comms])
    }

    pub fn get(&self, flow: Flow, role: Role) -> Option<&str> {
        self.0[slot_index(flow, role)].as_deref()
    }

    pub fn set(&mut self, flow: Flow, role: Role, id: Option<String>) {
        self.0[slot_index(flow, role)] = id;
    }

    pub fn clear(&mut self) {
        self.0 = Default::default();
    }

    /// (playback, comms playback, recording, comms recording).
    pub fn into_parts(self) -> [Option<String>; 4] {
        self.0
    }
}

/// `FindNotifiedDefault`: the remembered notification id counts only when its wrapper exists with
/// the same flow and is Active.
pub fn notified_default(endpoints: &[EndpointRef<'_>], flow: Flow, notified: Option<&str>) -> Option<usize> {
    let id = notified.filter(|id| !id.is_empty())?;
    let index = endpoints.iter().position(|e| e.id == id)?;
    let endpoint = &endpoints[index];
    (endpoint.flow == flow && endpoint.active).then_some(index)
}

/// `LookupDefault` step 2 match: the OS default id must belong to a tracked wrapper of the same
/// flow (the caller then refreshes its state and requires Active).
pub fn os_default_candidate(endpoints: &[EndpointRef<'_>], flow: Flow, os_id: &str) -> Option<usize> {
    if os_id.is_empty() {
        return None;
    }
    let index = endpoints.iter().position(|e| e.id == os_id)?;
    (endpoints[index].flow == flow).then_some(index)
}

/// `PersistLastKnownDefaults`: every non-null result whose id changed overwrites the persisted id;
/// a null result never clears it. Returns true when anything changed.
pub fn persist_last_known(last_known: &mut RoleIds, results: [Option<&str>; 4]) -> bool {
    let mut dirty = false;
    for (slot, result) in last_known.0.iter_mut().zip(results) {
        if let Some(id) = result
            && slot.as_deref() != Some(id)
        {
            *slot = Some(id.to_owned());
            dirty = true;
        }
    }
    dirty
}

/// Effective default: the active result, else the wrapper of the persisted id (any state).
pub fn effective_default(
    endpoints: &[EndpointRef<'_>],
    active: Option<usize>,
    last_known: Option<&str>,
) -> Option<usize> {
    active.or_else(|| {
        let id = last_known.filter(|id| !id.is_empty())?;
        endpoints.iter().position(|e| e.id == id)
    })
}

/// `IsDefault` / `IsDefaultCommunications` for every endpoint from the four effective results
/// (`[render, render comms, capture, capture comms]`).
pub fn default_flags(endpoints: &[EndpointRef<'_>], effective: [Option<usize>; 4]) -> Vec<(bool, bool)> {
    endpoints
        .iter()
        .enumerate()
        .map(|(index, endpoint)| {
            let (default, comms) = match endpoint.flow {
                Flow::Render => (effective[0], effective[1]),
                Flow::Capture => (effective[2], effective[3]),
            };
            (default == Some(index), comms == Some(index))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoints() -> Vec<EndpointRef<'static>> {
        vec![
            EndpointRef { id: "r-active", flow: Flow::Render, active: true },
            EndpointRef { id: "r-disabled", flow: Flow::Render, active: false },
            EndpointRef { id: "c-active", flow: Flow::Capture, active: true },
            EndpointRef { id: "c-unplugged", flow: Flow::Capture, active: false },
        ]
    }

    #[test]
    fn notified_requires_same_flow_and_active() {
        let list = endpoints();
        assert_eq!(notified_default(&list, Flow::Render, Some("r-active")), Some(0));
        assert_eq!(notified_default(&list, Flow::Render, Some("r-disabled")), None);
        assert_eq!(notified_default(&list, Flow::Capture, Some("r-active")), None);
        assert_eq!(notified_default(&list, Flow::Render, Some("missing")), None);
        assert_eq!(notified_default(&list, Flow::Render, Some("")), None);
        assert_eq!(notified_default(&list, Flow::Render, None), None);
    }

    #[test]
    fn os_candidate_rejects_other_flow() {
        let list = endpoints();
        assert_eq!(os_default_candidate(&list, Flow::Capture, "c-unplugged"), Some(3));
        assert_eq!(os_default_candidate(&list, Flow::Render, "c-active"), None);
        assert_eq!(os_default_candidate(&list, Flow::Render, ""), None);
    }

    #[test]
    fn fallback_uses_persisted_id_only_without_active_result() {
        let list = endpoints();
        // Active result wins.
        assert_eq!(effective_default(&list, Some(0), Some("r-disabled")), Some(0));
        // No active default: the disabled last-known default keeps the flag.
        assert_eq!(effective_default(&list, None, Some("r-disabled")), Some(1));
        // Persisted id gone or empty: no default.
        assert_eq!(effective_default(&list, None, Some("removed")), None);
        assert_eq!(effective_default(&list, None, Some("")), None);
        assert_eq!(effective_default(&list, None, None), None);

        let flags = default_flags(&list, [None, Some(0), Some(2), effective_default(&list, None, Some("c-unplugged"))]);
        assert_eq!(flags, vec![(false, true), (false, false), (true, false), (false, true)]);
    }

    #[test]
    fn persist_ignores_null_results_and_reports_changes() {
        let mut last = RoleIds::new(Some("r-old".into()), None, Some("c-active".into()), None);
        assert!(!persist_last_known(&mut last, [None, None, Some("c-active"), None]));
        assert!(persist_last_known(&mut last, [Some("r-active"), Some("r-active"), None, None]));
        assert_eq!(last.get(Flow::Render, Role::Multimedia), Some("r-active"));
        assert_eq!(last.get(Flow::Render, Role::Communications), Some("r-active"));
        assert_eq!(last.get(Flow::Capture, Role::Multimedia), Some("c-active"));
        assert_eq!(last.get(Flow::Capture, Role::Communications), None);
    }

    #[test]
    fn role_ids_slots() {
        let mut notified = RoleIds::default();
        notified.set(Flow::Capture, Role::Communications, Some("x".into()));
        assert_eq!(notified.get(Flow::Capture, Role::Communications), Some("x"));
        assert_eq!(notified.get(Flow::Capture, Role::Multimedia), None);
        notified.clear();
        assert_eq!(notified, RoleIds::default());
        assert_eq!(SLOTS.map(|(f, r)| slot_index(f, r)), [0, 1, 2, 3]);
    }
}
