//! Reconnect state-machine for the WiFi serial of a single active device.
//!
//! Pure state transitions; no I/O. See `spawn` (next task) for the runtime.

use fl_core::DeviceEvent;
use std::time::Duration;

use crate::pair::WifiTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerSetup {
    pub target: WifiTarget,
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Attached { target: WifiTarget, device_name: String },
    DebouncingLost { target: WifiTarget, device_name: String },
    Reconnecting { target: WifiTarget, device_name: String, attempt: u32 },
}

impl State {
    pub fn new(setup: ManagerSetup) -> Self {
        State::Attached { target: setup.target, device_name: setup.device_name }
    }
    pub fn target(&self) -> &WifiTarget {
        match self {
            State::Attached { target, .. }
            | State::DebouncingLost { target, .. }
            | State::Reconnecting { target, .. } => target,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    DeviceLost { serial: String },
    DeviceDiscovered { serial: String },
    DebounceExpired,
    BackoffTick,
    ConnectResult { ok: bool },
    IpDiscovered { new_ip: String },
    ForceReconnect,
}

#[derive(Debug, Clone)]
pub enum Output {
    Emit(DeviceEvent),
    ScheduleDebounce(Duration),
    AttemptConnect(WifiTarget),
    ScheduleBackoff(Duration),
}

/// `delay(0) = 1`, `delay(4) = 16`, `delay(5..) = 30`.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX).min(30);
    Duration::from_secs(secs)
}

pub fn transition(state: State, input: Input) -> (State, Vec<Output>) {
    match (state, input) {
        // ===== Attached =====
        (State::Attached { target, device_name }, Input::DeviceLost { serial })
            if serial == target.serial() =>
        {
            let outs = vec![Output::ScheduleDebounce(Duration::from_millis(500))];
            (State::DebouncingLost { target, device_name }, outs)
        }
        (s @ State::Attached { .. }, Input::DeviceLost { .. }) => (s, vec![]),
        (State::Attached { mut target, device_name }, Input::IpDiscovered { new_ip })
            if new_ip != target.ip =>
        {
            let old_ip = target.ip.clone();
            target.ip = new_ip.clone();
            let serial = target.serial();
            let outs = vec![
                Output::Emit(DeviceEvent::IpChanged { serial, old_ip, new_ip }),
                Output::AttemptConnect(target.clone()),
            ];
            (State::Attached { target, device_name }, outs)
        }
        (s @ State::Attached { .. }, _) => (s, vec![]),

        // ===== DebouncingLost =====
        (State::DebouncingLost { target, device_name }, Input::DeviceDiscovered { serial })
            if serial == target.serial() =>
        {
            (State::Attached { target, device_name }, vec![])
        }
        (State::DebouncingLost { target, device_name }, Input::DebounceExpired) => {
            let attempt = 0;
            let outs = vec![
                Output::Emit(DeviceEvent::WifiReconnecting { attempt }),
                Output::ScheduleBackoff(backoff_delay(attempt)),
            ];
            (State::Reconnecting { target, device_name, attempt }, outs)
        }
        (s @ State::DebouncingLost { .. }, _) => (s, vec![]),

        // ===== Reconnecting =====
        (State::Reconnecting { target, device_name, .. }, Input::DeviceDiscovered { serial })
            if serial == target.serial() =>
        {
            let outs = vec![Output::Emit(DeviceEvent::WifiReconnected)];
            (State::Attached { target, device_name }, outs)
        }
        (State::Reconnecting { target, device_name, attempt }, Input::BackoffTick) => {
            let outs = vec![Output::AttemptConnect(target.clone())];
            (State::Reconnecting { target, device_name, attempt }, outs)
        }
        (State::Reconnecting { target, device_name, attempt }, Input::ConnectResult { ok: false }) => {
            let next_attempt = attempt.saturating_add(1);
            let outs = vec![
                Output::Emit(DeviceEvent::WifiReconnecting { attempt: next_attempt }),
                Output::ScheduleBackoff(backoff_delay(next_attempt)),
            ];
            (State::Reconnecting { target, device_name, attempt: next_attempt }, outs)
        }
        (State::Reconnecting { target, device_name, attempt }, Input::ConnectResult { ok: true }) => {
            // Success will be confirmed by a subsequent DeviceDiscovered; stay here meanwhile.
            (State::Reconnecting { target, device_name, attempt }, vec![])
        }
        (State::Reconnecting { mut target, device_name, attempt }, Input::IpDiscovered { new_ip })
            if new_ip != target.ip =>
        {
            let old_ip = target.ip.clone();
            target.ip = new_ip.clone();
            let serial = target.serial();
            let outs = vec![
                Output::Emit(DeviceEvent::IpChanged { serial, old_ip, new_ip }),
                Output::AttemptConnect(target.clone()),
            ];
            (State::Reconnecting { target, device_name, attempt }, outs)
        }
        (State::Reconnecting { target, device_name, attempt }, Input::ForceReconnect) => {
            let outs = vec![Output::AttemptConnect(target.clone())];
            (State::Reconnecting { target, device_name, attempt }, outs)
        }
        (s @ State::Reconnecting { .. }, _) => (s, vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> ManagerSetup {
        ManagerSetup {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "Pixel 8".into(),
        }
    }

    #[test]
    fn backoff_delays_match_spec() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(3), Duration::from_secs(8));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
        assert_eq!(backoff_delay(5), Duration::from_secs(30));
        assert_eq!(backoff_delay(20), Duration::from_secs(30));
    }

    #[test]
    fn attached_lost_target_serial_enters_debouncing() {
        let s = State::new(setup());
        let (s, outs) = transition(s, Input::DeviceLost { serial: "1.2.3.4:5555".into() });
        assert!(matches!(s, State::DebouncingLost { .. }));
        assert_eq!(outs.len(), 1);
        assert!(matches!(outs[0], Output::ScheduleDebounce(_)));
    }

    #[test]
    fn attached_ignores_lost_for_other_serial() {
        let s = State::new(setup());
        let (s, outs) = transition(s, Input::DeviceLost { serial: "other".into() });
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn debouncing_discovered_cancels_reconnect() {
        let s = State::DebouncingLost {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
        };
        let (s, outs) = transition(s, Input::DeviceDiscovered { serial: "1.2.3.4:5555".into() });
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn debouncing_expired_starts_reconnecting_at_attempt_0() {
        let s = State::DebouncingLost {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
        };
        let (s, outs) = transition(s, Input::DebounceExpired);
        if let State::Reconnecting { attempt, .. } = s {
            assert_eq!(attempt, 0);
        } else {
            panic!("expected Reconnecting, got {s:?}");
        }
        assert!(matches!(&outs[0], Output::Emit(DeviceEvent::WifiReconnecting { attempt: 0 })));
        assert!(matches!(&outs[1], Output::ScheduleBackoff(d) if *d == Duration::from_secs(1)));
    }

    #[test]
    fn reconnecting_tick_attempts_connect() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 2,
        };
        let (_, outs) = transition(s, Input::BackoffTick);
        assert!(matches!(&outs[0], Output::AttemptConnect(t) if t.ip == "1.2.3.4"));
    }

    #[test]
    fn reconnecting_failure_increments_and_schedules_next() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 1,
        };
        let (s, outs) = transition(s, Input::ConnectResult { ok: false });
        if let State::Reconnecting { attempt, .. } = s {
            assert_eq!(attempt, 2);
        } else {
            panic!();
        }
        assert!(matches!(&outs[0], Output::Emit(DeviceEvent::WifiReconnecting { attempt: 2 })));
        assert!(matches!(&outs[1], Output::ScheduleBackoff(d) if *d == Duration::from_secs(4)));
    }

    #[test]
    fn reconnecting_discovered_emits_reconnected_and_returns_to_attached() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 5,
        };
        let (s, outs) = transition(s, Input::DeviceDiscovered { serial: "1.2.3.4:5555".into() });
        assert!(matches!(s, State::Attached { .. }));
        assert_eq!(outs.len(), 1);
        assert!(matches!(outs[0], Output::Emit(DeviceEvent::WifiReconnected)));
    }

    #[test]
    fn ip_discovered_in_attached_updates_target_and_emits() {
        let s = State::new(setup());
        let (s, outs) = transition(s, Input::IpDiscovered { new_ip: "10.0.0.5".into() });
        if let State::Attached { target, .. } = &s {
            assert_eq!(target.ip, "10.0.0.5");
        } else {
            panic!();
        }
        assert!(matches!(&outs[0], Output::Emit(DeviceEvent::IpChanged { new_ip, .. }) if new_ip == "10.0.0.5"));
        assert!(matches!(&outs[1], Output::AttemptConnect(t) if t.ip == "10.0.0.5"));
    }

    #[test]
    fn ip_discovered_same_ip_is_noop() {
        let s = State::new(setup());
        let (s, outs) = transition(s, Input::IpDiscovered { new_ip: "1.2.3.4".into() });
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn ip_discovered_in_reconnecting_short_circuits_with_connect() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 3,
        };
        let (_, outs) = transition(s, Input::IpDiscovered { new_ip: "10.0.0.5".into() });
        assert!(matches!(&outs[1], Output::AttemptConnect(t) if t.ip == "10.0.0.5"));
    }

    #[test]
    fn force_reconnect_in_reconnecting_attempts_immediately() {
        let s = State::Reconnecting {
            target: WifiTarget { ip: "1.2.3.4".into(), port: 5555 },
            device_name: "P".into(),
            attempt: 0,
        };
        let (_, outs) = transition(s, Input::ForceReconnect);
        assert!(matches!(&outs[0], Output::AttemptConnect(_)));
    }
}
