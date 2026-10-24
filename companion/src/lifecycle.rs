use crate::protocol::{ErrorCode, StatusState};
use std::{collections::VecDeque, time::Duration};

const IDLE_GRACE: Duration = Duration::from_secs(20);
const READY_TIMEOUT: Duration = Duration::from_secs(45);
const RETRY_WINDOW: Duration = Duration::from_secs(300);
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Active,
    Idle,
    GameExited,
    BackendReady,
    BackendFailed(ErrorCode),
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    StartBackend,
    StopBackend,
    Publish {
        state: StatusState,
        error_code: Option<ErrorCode>,
    },
    Exit,
}

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Waiting,
    Starting(Duration),
    Ready,
    Failed,
    Exited,
}

/// Pure policy. The caller executes actions in order and scopes every stop to
/// its owned backend. Construct only after confirming the current game session.
#[derive(Debug, Default)]
pub struct Lifecycle {
    phase: Phase,
    active: bool,
    idle_since: Option<Duration>,
    retry_at: Option<Duration>,
    retry_starts: VecDeque<Duration>,
    recovering: bool,
    last_now: Duration,
}

impl Lifecycle {
    pub fn step(&mut self, event: Event, now: Duration) -> Vec<Action> {
        if matches!(self.phase, Phase::Exited) {
            return Vec::new();
        }
        // Caller supplies monotonic elapsed time; defensively clamp regressions.
        let now = now.max(self.last_now);
        self.last_now = now;
        self.prune_retries(now);
        if event == Event::GameExited {
            self.phase = Phase::Exited;
            self.retry_at = None;
            self.idle_since = None;
            self.active = false;
            return vec![
                publish(StatusState::Stopping, None),
                Action::StopBackend,
                Action::Exit,
            ];
        }
        let mut actions = Vec::new();
        match event {
            Event::Active => {
                self.active = true;
                self.idle_since = None;
                match self.phase {
                    Phase::Waiting => self.start(now, false, &mut actions),
                    Phase::Failed if self.retry_at.is_none() => {
                        self.schedule_retry(now);
                    }
                    _ => {}
                }
            }
            Event::Idle => {
                let was_active = self.active;
                self.active = false;
                self.retry_at = None;
                if matches!(self.phase, Phase::Starting(_) | Phase::Ready) {
                    self.idle_since.get_or_insert(now);
                } else if matches!(self.phase, Phase::Failed) && was_active {
                    // Retain failure history across map transitions; the next
                    // Active must still use the retry policy rather than a free start.
                    actions.push(publish(StatusState::Waiting, None));
                }
            }
            Event::BackendReady => {
                if let Phase::Starting(started) = self.phase {
                    if now.saturating_sub(started) < READY_TIMEOUT {
                        self.phase = Phase::Ready;
                        self.recovering = false;
                        actions.push(publish(StatusState::Ready, None));
                    }
                }
            }
            Event::BackendFailed(code) => {
                if matches!(self.phase, Phase::Starting(_) | Phase::Ready) {
                    self.fail(code, now, &mut actions);
                }
            }
            Event::Tick | Event::GameExited => {}
        }
        if self
            .idle_since
            .is_some_and(|since| now.saturating_sub(since) >= IDLE_GRACE)
        {
            self.idle_since = None;
            self.phase = if self.recovering {
                Phase::Failed
            } else {
                Phase::Waiting
            };
            actions.extend([
                publish(StatusState::Stopping, None),
                Action::StopBackend,
                publish(StatusState::Waiting, None),
            ]);
        }
        if let Phase::Starting(started) = self.phase {
            if now.saturating_sub(started) >= READY_TIMEOUT {
                self.fail(ErrorCode::BackendReadyTimeout, now, &mut actions);
            }
        }
        if self.active && self.retry_at.is_some_and(|deadline| now >= deadline) {
            self.retry_at = None;
            if self.retry_starts.len() < RETRY_DELAYS.len() {
                self.start(now, true, &mut actions);
            }
        }
        actions
    }

    fn start(&mut self, now: Duration, retry: bool, actions: &mut Vec<Action>) {
        self.phase = Phase::Starting(now);
        if retry {
            self.retry_starts.push_back(now);
        }
        actions.extend([publish(StatusState::Starting, None), Action::StartBackend]);
    }

    fn fail(&mut self, code: ErrorCode, now: Duration, actions: &mut Vec<Action>) {
        self.phase = Phase::Failed;
        self.recovering = true;
        self.idle_since = None;
        actions.push(Action::StopBackend);
        if !self.active {
            actions.push(publish(StatusState::Waiting, None));
        } else {
            let scheduled = self.schedule_retry(now);
            actions.push(publish(
                StatusState::Error,
                Some(if scheduled {
                    code
                } else {
                    ErrorCode::RetryLimit
                }),
            ));
        }
    }

    fn schedule_retry(&mut self, now: Duration) -> bool {
        if let Some(delay) = RETRY_DELAYS.get(self.retry_starts.len()) {
            self.retry_at = Some(now.saturating_add(*delay));
            true
        } else {
            false
        }
    }

    fn prune_retries(&mut self, now: Duration) {
        while self
            .retry_starts
            .front()
            .is_some_and(|started| now.saturating_sub(*started) >= RETRY_WINDOW)
        {
            self.retry_starts.pop_front();
        }
    }
}

fn publish(state: StatusState, error_code: Option<ErrorCode>) -> Action {
    Action::Publish { state, error_code }
}
