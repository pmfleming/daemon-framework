//! Suspend-aware resume detection. Wall-clock changes and awake stalls are not resumes.
use std::time::Duration;

use futures::StreamExt;
use rustix::time::{ClockId, clock_gettime};
use tokio::{
    sync::{mpsc, watch},
    time::{MissedTickBehavior, interval, sleep},
};

use crate::task::{AbortOnDrop, spawn_named};

/// BOOTTIME includes suspend, MONOTONIC does not. Discard preempted samples.
#[must_use]
pub fn suspend_offset() -> Option<i128> {
    fn ns(id: ClockId) -> i128 {
        let t = clock_gettime(id);
        i128::from(t.tv_sec) * 1_000_000_000 + i128::from(t.tv_nsec)
    }
    let before = ns(ClockId::Monotonic);
    let boot = ns(ClockId::Boottime);
    let after = ns(ClockId::Monotonic);
    (after - before < 50_000_000).then_some(boot - (before + after) / 2)
}

#[derive(Debug, Default)]
pub struct ResumeDetector {
    offset: Option<i128>,
    reported_before_signal: bool,
}

impl ResumeDetector {
    fn changed(&mut self, offset: Option<i128>) -> bool {
        let Some(offset) = offset else {
            return false;
        };
        let changed = self
            .offset
            .is_some_and(|previous| offset - previous > 250_000_000);
        self.offset = Some(offset);
        changed
    }

    pub fn poll(&mut self, offset: Option<i128>) -> bool {
        let changed = self.changed(offset);
        self.reported_before_signal |= changed;
        changed
    }

    pub fn signal(&mut self, offset: Option<i128>) -> bool {
        let changed = self.changed(offset);
        let announce = changed || !self.reported_before_signal;
        self.reported_before_signal = false;
        announce
    }

    pub fn resumed(&mut self) -> bool {
        self.poll(suspend_offset())
    }
}

/// Read-only sleep preparation and a monotonic resume generation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SleepObservation {
    pub preparing: bool,
    pub resume_generation: u64,
}

/// Shared transition semantics for observers and policy owners. Initial readback
/// is not a resume; real signals and suspend-clock changes are deduplicated.
#[derive(Default)]
pub struct SleepTransitionDetector {
    state: SleepObservation,
    resumes: ResumeDetector,
}
impl SleepTransitionDetector {
    #[must_use]
    pub fn state(&self) -> SleepObservation {
        self.state
    }
    pub fn poll(&mut self, offset: Option<i128>) -> SleepObservation {
        if self.resumes.poll(offset) {
            self.resumed();
        }
        self.state
    }
    pub fn signal(&mut self, preparing: bool, offset: Option<i128>) -> SleepObservation {
        if !preparing && self.resumes.signal(offset) {
            self.resumed();
        }
        self.state.preparing = preparing;
        self.state
    }
    pub fn readback(&mut self, preparing: bool, offset: Option<i128>) -> SleepObservation {
        if self.state.preparing && !preparing {
            return self.signal(false, offset);
        }
        self.state.preparing = preparing;
        self.state
    }
    fn resumed(&mut self) {
        self.state.preparing = false;
        self.state.resume_generation = self.state.resume_generation.saturating_add(1);
    }
}

enum SleepInput {
    Readback(bool),
    Signal(bool),
}

/// Publishes preparation and resume changes without power policy or telemetry.
/// Dropping the last receiver cancels bus connection, reads and reconnect delay.
pub async fn monitor_sleep_transitions(sender: watch::Sender<SleepObservation>) {
    let (events, mut signals) = mpsc::channel(16);
    let task = spawn_named("logind-sleep-observation", logind_events(events));
    let _logind = AbortOnDrop(task.abort_handle());
    let mut poll = interval(Duration::from_secs(2));
    poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut detector = SleepTransitionDetector {
        state: *sender.borrow(),
        ..Default::default()
    };
    detector.poll(suspend_offset());
    loop {
        let state = tokio::select! {
            _ = sender.closed() => return,
            _ = poll.tick() => detector.poll(suspend_offset()),
            event = signals.recv() => match event {
                Some(SleepInput::Signal(preparing)) => detector.signal(preparing, suspend_offset()),
                Some(SleepInput::Readback(preparing)) => detector.readback(preparing, suspend_offset()),
                None => return,
            },
        };
        sender.send_if_modified(|current| {
            if *current == state {
                false
            } else {
                *current = state;
                true
            }
        });
    }
}

/// Publishes a monotonic generation; stops when the last consumer leaves.
pub async fn monitor_resumes(sender: watch::Sender<u64>) {
    let base = *sender.borrow();
    let (observations, mut changes) = watch::channel(SleepObservation::default());
    let forward = async {
        loop {
            tokio::select! { _ = sender.closed() => return, result = changes.changed() => if result.is_err() { return; } }
            let generation = base.saturating_add(changes.borrow_and_update().resume_generation);
            sender.send_if_modified(|current| {
                if *current == generation {
                    false
                } else {
                    *current = generation;
                    true
                }
            });
        }
    };
    tokio::select! { _ = monitor_sleep_transitions(observations) => {}, _ = forward => {} }
}

async fn logind_events(sender: mpsc::Sender<SleepInput>) {
    let reconnect = async {
        loop {
            if let Err(error) = logind_connection(&sender).await {
                tracing::debug!(%error, "logind observation unavailable; clock fallback remains active");
            }
            sleep(Duration::from_secs(3)).await;
        }
    };
    tokio::select! { _ = sender.closed() => {}, _ = reconnect => {} }
}

async fn logind_connection(sender: &mpsc::Sender<SleepInput>) -> anyhow::Result<()> {
    let connection = zbus::connection::Builder::system()?
        .method_timeout(Duration::from_secs(3))
        .build()
        .await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?;
    let mut signals = proxy.receive_signal("PrepareForSleep").await?;
    match proxy.get_property::<bool>("PreparingForSleep").await {
        Ok(preparing) => {
            sender.send(SleepInput::Readback(preparing)).await?;
        }
        Err(error) => {
            tracing::debug!(%error, "initial sleep read unavailable; observing signals and clock")
        }
    }
    while let Some(signal) = signals.next().await {
        let (preparing,): (bool,) = signal.body().deserialize()?;
        sender.send(SleepInput::Signal(preparing)).await?;
    }
    anyhow::bail!("logind sleep stream ended")
}

#[cfg(test)]
mod tests {
    use super::{ResumeDetector, SleepTransitionDetector};
    #[test]
    fn readback_does_not_resume_and_clock_and_signals_share_one_generation() {
        let mut detector = SleepTransitionDetector::default();
        detector.poll(Some(0));
        assert_eq!(detector.readback(false, Some(0)).resume_generation, 0);
        assert!(detector.signal(true, Some(0)).preparing);
        assert_eq!(detector.poll(Some(1_000_000_000)).resume_generation, 1);
        assert_eq!(
            detector
                .signal(false, Some(1_000_000_000))
                .resume_generation,
            1
        );
        detector.signal(true, Some(1_000_000_000));
        assert_eq!(
            detector
                .signal(false, Some(2_000_000_000))
                .resume_generation,
            2
        );
        assert_eq!(detector.poll(Some(2_000_000_000)).resume_generation, 2);
    }
    #[test]
    fn resume_detection_ignores_stalls_and_deduplicates_signal_and_clock() {
        let mut detector = ResumeDetector::default();
        assert!(!detector.poll(Some(0)));
        assert!(!detector.poll(Some(0)));
        assert!(!detector.poll(None));
        assert!(detector.signal(Some(100_000_000)));
        assert!(!detector.poll(Some(100_000_000)));
        assert!(detector.poll(Some(5_100_000_000)));
        assert!(!detector.signal(Some(5_100_000_000)));
        assert!(detector.signal(Some(10_100_000_000)));
    }
}
