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
    Attached {
        target: WifiTarget,
        device_name: String,
    },
    DebouncingLost {
        target: WifiTarget,
        device_name: String,
    },
    Reconnecting {
        target: WifiTarget,
        device_name: String,
        attempt: u32,
    },
}

impl State {
    pub fn new(setup: ManagerSetup) -> Self {
        State::Attached {
            target: setup.target,
            device_name: setup.device_name,
        }
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
        (
            State::Attached {
                target,
                device_name,
            },
            Input::DeviceLost { serial },
        ) if serial == target.serial() => {
            let outs = vec![Output::ScheduleDebounce(Duration::from_millis(500))];
            (
                State::DebouncingLost {
                    target,
                    device_name,
                },
                outs,
            )
        }
        (s @ State::Attached { .. }, Input::DeviceLost { .. }) => (s, vec![]),
        (
            State::Attached {
                mut target,
                device_name,
            },
            Input::IpDiscovered { new_ip },
        ) if new_ip != target.ip => {
            let old_ip = target.ip.clone();
            target.ip = new_ip.clone();
            let serial = target.serial();
            let outs = vec![
                Output::Emit(DeviceEvent::IpChanged {
                    serial,
                    old_ip,
                    new_ip,
                }),
                Output::AttemptConnect(target.clone()),
            ];
            (
                State::Attached {
                    target,
                    device_name,
                },
                outs,
            )
        }
        (s @ State::Attached { .. }, _) => (s, vec![]),

        // ===== DebouncingLost =====
        (
            State::DebouncingLost {
                target,
                device_name,
            },
            Input::DeviceDiscovered { serial },
        ) if serial == target.serial() => (
            State::Attached {
                target,
                device_name,
            },
            vec![],
        ),
        (
            State::DebouncingLost {
                target,
                device_name,
            },
            Input::DebounceExpired,
        ) => {
            let attempt = 0;
            let outs = vec![
                Output::Emit(DeviceEvent::WifiReconnecting { attempt }),
                Output::ScheduleBackoff(backoff_delay(attempt)),
            ];
            (
                State::Reconnecting {
                    target,
                    device_name,
                    attempt,
                },
                outs,
            )
        }
        (s @ State::DebouncingLost { .. }, _) => (s, vec![]),

        // ===== Reconnecting =====
        (
            State::Reconnecting {
                target,
                device_name,
                ..
            },
            Input::DeviceDiscovered { serial },
        ) if serial == target.serial() => {
            let outs = vec![Output::Emit(DeviceEvent::WifiReconnected)];
            (
                State::Attached {
                    target,
                    device_name,
                },
                outs,
            )
        }
        (
            State::Reconnecting {
                target,
                device_name,
                attempt,
            },
            Input::BackoffTick,
        ) => {
            let outs = vec![Output::AttemptConnect(target.clone())];
            (
                State::Reconnecting {
                    target,
                    device_name,
                    attempt,
                },
                outs,
            )
        }
        (
            State::Reconnecting {
                target,
                device_name,
                attempt,
            },
            Input::ConnectResult { ok: false },
        ) => {
            let next_attempt = attempt.saturating_add(1);
            let outs = vec![
                Output::Emit(DeviceEvent::WifiReconnecting {
                    attempt: next_attempt,
                }),
                Output::ScheduleBackoff(backoff_delay(next_attempt)),
            ];
            (
                State::Reconnecting {
                    target,
                    device_name,
                    attempt: next_attempt,
                },
                outs,
            )
        }
        (
            State::Reconnecting {
                target,
                device_name,
                attempt,
            },
            Input::ConnectResult { ok: true },
        ) => {
            // Success will be confirmed by a subsequent DeviceDiscovered; stay here meanwhile.
            (
                State::Reconnecting {
                    target,
                    device_name,
                    attempt,
                },
                vec![],
            )
        }
        (
            State::Reconnecting {
                mut target,
                device_name,
                attempt,
            },
            Input::IpDiscovered { new_ip },
        ) if new_ip != target.ip => {
            let old_ip = target.ip.clone();
            target.ip = new_ip.clone();
            let serial = target.serial();
            let outs = vec![
                Output::Emit(DeviceEvent::IpChanged {
                    serial,
                    old_ip,
                    new_ip,
                }),
                Output::AttemptConnect(target.clone()),
            ];
            (
                State::Reconnecting {
                    target,
                    device_name,
                    attempt,
                },
                outs,
            )
        }
        (
            State::Reconnecting {
                target,
                device_name,
                attempt,
            },
            Input::ForceReconnect,
        ) => {
            let outs = vec![Output::AttemptConnect(target.clone())];
            (
                State::Reconnecting {
                    target,
                    device_name,
                    attempt,
                },
                outs,
            )
        }
        (s @ State::Reconnecting { .. }, _) => (s, vec![]),
    }
}

use crate::runner::CommandRunner;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct ManagerHandle {
    pub input_tx: mpsc::Sender<Input>,
    pub task: JoinHandle<()>,
}

pub fn spawn<R>(
    setup: ManagerSetup,
    runner: Arc<R>,
    out_tx: mpsc::Sender<DeviceEvent>,
) -> ManagerHandle
where
    R: CommandRunner + 'static,
{
    let (input_tx, mut input_rx) = mpsc::channel::<Input>(64);
    let internal_tx = input_tx.clone();
    let task = tokio::spawn(async move {
        let mut state = State::new(setup);
        while let Some(input) = input_rx.recv().await {
            let (next, outs) = transition(state, input);
            state = next;
            for out in outs {
                match out {
                    Output::Emit(ev) => {
                        out_tx.send(ev).await.ok();
                    }
                    Output::ScheduleDebounce(d) => {
                        let tx = internal_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(d).await;
                            tx.send(Input::DebounceExpired).await.ok();
                        });
                    }
                    Output::ScheduleBackoff(d) => {
                        let tx = internal_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(d).await;
                            tx.send(Input::BackoffTick).await.ok();
                        });
                    }
                    Output::AttemptConnect(target) => {
                        let tx = internal_tx.clone();
                        let runner = runner.clone();
                        tokio::spawn(async move {
                            let serial = target.serial();
                            let res = runner.run("adb", &["connect", &serial]).await;
                            let ok = match res {
                                Ok(o) => {
                                    o.status == 0
                                        && !o.stdout.contains("failed to connect")
                                        && !o.stdout.contains("cannot connect")
                                }
                                Err(_) => false,
                            };
                            tx.send(Input::ConnectResult { ok }).await.ok();
                        });
                    }
                }
            }
        }
    });
    ManagerHandle { input_tx, task }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> ManagerSetup {
        ManagerSetup {
            target: WifiTarget {
                ip: "1.2.3.4".into(),
                port: 5555,
            },
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
        let (s, outs) = transition(
            s,
            Input::DeviceLost {
                serial: "1.2.3.4:5555".into(),
            },
        );
        assert!(matches!(s, State::DebouncingLost { .. }));
        assert_eq!(outs.len(), 1);
        assert!(matches!(outs[0], Output::ScheduleDebounce(_)));
    }

    #[test]
    fn attached_ignores_lost_for_other_serial() {
        let s = State::new(setup());
        let (s, outs) = transition(
            s,
            Input::DeviceLost {
                serial: "other".into(),
            },
        );
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn debouncing_discovered_cancels_reconnect() {
        let s = State::DebouncingLost {
            target: WifiTarget {
                ip: "1.2.3.4".into(),
                port: 5555,
            },
            device_name: "P".into(),
        };
        let (s, outs) = transition(
            s,
            Input::DeviceDiscovered {
                serial: "1.2.3.4:5555".into(),
            },
        );
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn debouncing_expired_starts_reconnecting_at_attempt_0() {
        let s = State::DebouncingLost {
            target: WifiTarget {
                ip: "1.2.3.4".into(),
                port: 5555,
            },
            device_name: "P".into(),
        };
        let (s, outs) = transition(s, Input::DebounceExpired);
        if let State::Reconnecting { attempt, .. } = s {
            assert_eq!(attempt, 0);
        } else {
            panic!("expected Reconnecting, got {s:?}");
        }
        assert!(matches!(
            &outs[0],
            Output::Emit(DeviceEvent::WifiReconnecting { attempt: 0 })
        ));
        assert!(matches!(&outs[1], Output::ScheduleBackoff(d) if *d == Duration::from_secs(1)));
    }

    #[test]
    fn reconnecting_tick_attempts_connect() {
        let s = State::Reconnecting {
            target: WifiTarget {
                ip: "1.2.3.4".into(),
                port: 5555,
            },
            device_name: "P".into(),
            attempt: 2,
        };
        let (_, outs) = transition(s, Input::BackoffTick);
        assert!(matches!(&outs[0], Output::AttemptConnect(t) if t.ip == "1.2.3.4"));
    }

    #[test]
    fn reconnecting_failure_increments_and_schedules_next() {
        let s = State::Reconnecting {
            target: WifiTarget {
                ip: "1.2.3.4".into(),
                port: 5555,
            },
            device_name: "P".into(),
            attempt: 1,
        };
        let (s, outs) = transition(s, Input::ConnectResult { ok: false });
        if let State::Reconnecting { attempt, .. } = s {
            assert_eq!(attempt, 2);
        } else {
            panic!();
        }
        assert!(matches!(
            &outs[0],
            Output::Emit(DeviceEvent::WifiReconnecting { attempt: 2 })
        ));
        assert!(matches!(&outs[1], Output::ScheduleBackoff(d) if *d == Duration::from_secs(4)));
    }

    #[test]
    fn reconnecting_discovered_emits_reconnected_and_returns_to_attached() {
        let s = State::Reconnecting {
            target: WifiTarget {
                ip: "1.2.3.4".into(),
                port: 5555,
            },
            device_name: "P".into(),
            attempt: 5,
        };
        let (s, outs) = transition(
            s,
            Input::DeviceDiscovered {
                serial: "1.2.3.4:5555".into(),
            },
        );
        assert!(matches!(s, State::Attached { .. }));
        assert_eq!(outs.len(), 1);
        assert!(matches!(
            outs[0],
            Output::Emit(DeviceEvent::WifiReconnected)
        ));
    }

    #[test]
    fn ip_discovered_in_attached_updates_target_and_emits() {
        let s = State::new(setup());
        let (s, outs) = transition(
            s,
            Input::IpDiscovered {
                new_ip: "10.0.0.5".into(),
            },
        );
        if let State::Attached { target, .. } = &s {
            assert_eq!(target.ip, "10.0.0.5");
        } else {
            panic!();
        }
        assert!(
            matches!(&outs[0], Output::Emit(DeviceEvent::IpChanged { new_ip, .. }) if new_ip == "10.0.0.5")
        );
        assert!(matches!(&outs[1], Output::AttemptConnect(t) if t.ip == "10.0.0.5"));
    }

    #[test]
    fn ip_discovered_same_ip_is_noop() {
        let s = State::new(setup());
        let (s, outs) = transition(
            s,
            Input::IpDiscovered {
                new_ip: "1.2.3.4".into(),
            },
        );
        assert!(matches!(s, State::Attached { .. }));
        assert!(outs.is_empty());
    }

    #[test]
    fn ip_discovered_in_reconnecting_short_circuits_with_connect() {
        let s = State::Reconnecting {
            target: WifiTarget {
                ip: "1.2.3.4".into(),
                port: 5555,
            },
            device_name: "P".into(),
            attempt: 3,
        };
        let (_, outs) = transition(
            s,
            Input::IpDiscovered {
                new_ip: "10.0.0.5".into(),
            },
        );
        assert!(matches!(&outs[1], Output::AttemptConnect(t) if t.ip == "10.0.0.5"));
    }

    #[test]
    fn force_reconnect_in_reconnecting_attempts_immediately() {
        let s = State::Reconnecting {
            target: WifiTarget {
                ip: "1.2.3.4".into(),
                port: 5555,
            },
            device_name: "P".into(),
            attempt: 0,
        };
        let (_, outs) = transition(s, Input::ForceReconnect);
        assert!(matches!(&outs[0], Output::AttemptConnect(_)));
    }

    use crate::runner::{CommandOutput, MockRunner};
    #[allow(unused_imports)]
    use tokio::time::{advance, pause, sleep, Duration as TDuration};

    fn arc_mock() -> Arc<MockRunner> {
        Arc::new(MockRunner::new())
    }

    async fn drain(rx: &mut mpsc::Receiver<DeviceEvent>) -> Vec<DeviceEvent> {
        let mut v = Vec::new();
        while let Ok(Some(e)) = tokio::time::timeout(TDuration::from_millis(50), rx.recv()).await {
            v.push(e);
        }
        v
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_emits_reconnecting_after_debounce_and_first_backoff() {
        let runner = arc_mock();
        runner.expect(
            "adb connect 1.2.3.4:5555",
            CommandOutput {
                stdout: "failed to connect to 1.2.3.4:5555\n".into(),
                stderr: String::new(),
                status: 0,
            },
        );

        let (out_tx, mut out_rx) = mpsc::channel(16);
        let h = spawn(
            ManagerSetup {
                target: WifiTarget {
                    ip: "1.2.3.4".into(),
                    port: 5555,
                },
                device_name: "P".into(),
            },
            runner.clone(),
            out_tx,
        );

        h.input_tx
            .send(Input::DeviceLost {
                serial: "1.2.3.4:5555".into(),
            })
            .await
            .unwrap();
        // Advance past debounce (500 ms)
        advance(TDuration::from_millis(600)).await;
        // Advance past first backoff (1 s) so connect runs and ConnectResult comes back
        advance(TDuration::from_secs(2)).await;
        // Yield so spawned tasks can run.
        sleep(TDuration::from_millis(1)).await;

        let evs = drain(&mut out_rx).await;
        let reconnecting_count = evs
            .iter()
            .filter(|e| matches!(e, DeviceEvent::WifiReconnecting { .. }))
            .count();
        assert!(
            reconnecting_count >= 1,
            "expected at least one WifiReconnecting, got {evs:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_discovered_during_debounce_cancels_reconnect() {
        let runner = arc_mock();
        let (out_tx, mut out_rx) = mpsc::channel(16);
        let h = spawn(
            ManagerSetup {
                target: WifiTarget {
                    ip: "1.2.3.4".into(),
                    port: 5555,
                },
                device_name: "P".into(),
            },
            runner,
            out_tx,
        );

        h.input_tx
            .send(Input::DeviceLost {
                serial: "1.2.3.4:5555".into(),
            })
            .await
            .unwrap();
        advance(TDuration::from_millis(200)).await;
        h.input_tx
            .send(Input::DeviceDiscovered {
                serial: "1.2.3.4:5555".into(),
            })
            .await
            .unwrap();
        advance(TDuration::from_millis(800)).await;
        sleep(TDuration::from_millis(1)).await;

        let evs = drain(&mut out_rx).await;
        assert!(
            evs.iter()
                .all(|e| !matches!(e, DeviceEvent::WifiReconnecting { .. })),
            "expected no Reconnecting after cancellation, got {evs:?}"
        );
    }
}
