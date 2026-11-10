//! The session state machine.
//!
//! The engine never reads a clock. The caller feeds it timestamps in
//! milliseconds (from any monotonic source) through [`Engine::start`],
//! [`Engine::tick`] and [`Engine::press`], and gets back the [`Event`]s that
//! became due. Transitions happen at their scheduled deadlines, not at the
//! moment of the call, so late or sparse ticks neither drift nor skip trials.

use serde::{Deserialize, Serialize};

use crate::scoring::{BLOCK_SIZE, Counts, Outcome, adapt_exposure, clamp_exposure, isi_ms};
use crate::session::{SessionConfig, SessionPlan, Trial, TrialKind};

/// The countdown before the first face starts from this number.
pub const COUNTDOWN_FROM: u8 = 3;
/// Duration of one countdown step.
pub const COUNTDOWN_STEP_MS: u64 = 800;
/// Extra pause after a punished trial, so the flash is noticed.
pub const PUNISHMENT_PAUSE_MS: u64 = 320;

/// A punished mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mistake {
    /// Pressed on a new face.
    FalseAlarm,
    /// Let a repeat pass.
    Miss,
}

/// Something the UI has to do or show.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Show a countdown number (3, 2, 1).
    Countdown(u8),
    /// Show the face of trial `index` for `exposure_ms`.
    ShowFace {
        index: usize,
        trial: Trial,
        exposure_ms: u32,
    },
    /// Replace the face with the fixation cross.
    HideFace,
    /// A repeat was caught `rt_ms` after the face appeared.
    Hit { rt_ms: u32 },
    /// Punish the user.
    Punish(Mistake),
    /// The exposure changed after a block; applies from the next face.
    ExposureChanged { exposure_ms: u32 },
    /// The session is over.
    Finished(Box<SessionResult>),
}

/// What happened on one trial.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrialRecord {
    pub trial: Trial,
    pub outcome: Outcome,
    /// Time from face onset to the press, if there was one.
    pub rt_ms: Option<u32>,
    /// Exposure this trial was shown at.
    pub exposure_ms: u32,
}

/// Summary of a finished session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionResult {
    pub config: SessionConfig,
    pub counts: Counts,
    pub balanced_accuracy: f64,
    pub d_prime: f64,
    /// Mean reaction time over hits; `None` without hits.
    pub mean_rt_ms: Option<f64>,
    pub exposure_start_ms: u32,
    pub exposure_end_ms: u32,
    pub records: Vec<TrialRecord>,
}

/// Coarse state of the engine, for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    /// Created, not started.
    Idle,
    Countdown,
    Running,
    Paused,
    Finished,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Countdown {
        remaining: u8,
        next_at: u64,
    },
    Face {
        hide_at: u64,
        end_at: u64,
    },
    Blank {
        end_at: u64,
    },
    /// Between trials; lasts longer after a punishment.
    Gap {
        until: u64,
    },
    Finished,
    Aborted,
}

/// Deterministic state machine that runs one session.
#[derive(Debug, Clone)]
pub struct Engine {
    plan: SessionPlan,
    phase: Phase,
    exposure_ms: u32,
    exposure_start_ms: u32,
    /// Trials whose face has been shown.
    started: usize,
    /// Onset of the current trial, in engine time.
    trial_start: u64,
    /// Reaction time of the press on the current trial, if any.
    pressed: Option<u32>,
    counts: Counts,
    block: Counts,
    records: Vec<TrialRecord>,
    /// Caller timestamp of the ongoing pause.
    paused_at: Option<u64>,
    /// Total time spent paused; engine time is caller time minus this.
    paused_total: u64,
    /// Latest engine time seen, to ignore timestamps going backwards.
    last_time: u64,
}

impl Engine {
    /// Creates an engine for `plan`, starting at `start_exposure_ms` (clamped
    /// to the allowed range).
    #[must_use]
    pub fn new(plan: SessionPlan, start_exposure_ms: u32) -> Self {
        let exposure_ms = clamp_exposure(start_exposure_ms);
        Self {
            records: Vec::with_capacity(plan.trials.len()),
            plan,
            phase: Phase::Idle,
            exposure_ms,
            exposure_start_ms: exposure_ms,
            started: 0,
            trial_start: 0,
            pressed: None,
            counts: Counts::default(),
            block: Counts::default(),
            paused_at: None,
            paused_total: 0,
            last_time: 0,
        }
    }

    /// Starts the countdown. Does nothing if the session was already started.
    pub fn start(&mut self, now_ms: u64) -> Vec<Event> {
        if self.phase != Phase::Idle {
            return Vec::new();
        }
        let now = self.engine_time(now_ms);
        self.phase = Phase::Countdown {
            remaining: COUNTDOWN_FROM,
            next_at: now + COUNTDOWN_STEP_MS,
        };
        vec![Event::Countdown(COUNTDOWN_FROM)]
    }

    /// Advances the session to `now_ms` and returns everything that became due.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Event> {
        let mut events = Vec::new();
        if self.paused_at.is_some() {
            return events;
        }
        let now = self.engine_time(now_ms);
        loop {
            match self.phase {
                Phase::Countdown { remaining, next_at } if now >= next_at => {
                    if remaining > 1 {
                        self.phase = Phase::Countdown {
                            remaining: remaining - 1,
                            next_at: next_at + COUNTDOWN_STEP_MS,
                        };
                        events.push(Event::Countdown(remaining - 1));
                    } else {
                        self.begin_trial(next_at, &mut events);
                    }
                }
                Phase::Face { hide_at, end_at } if now >= hide_at => {
                    self.phase = Phase::Blank { end_at };
                    events.push(Event::HideFace);
                }
                Phase::Blank { end_at } if now >= end_at => self.finish_trial(end_at, &mut events),
                Phase::Gap { until } if now >= until => self.begin_trial(until, &mut events),
                _ => break,
            }
        }
        events
    }

    /// Registers a press at `now_ms`.
    ///
    /// The session is first advanced to `now_ms`, so the returned events may
    /// include transitions that became due. Only the first press inside a
    /// response window counts; presses outside any window are ignored.
    pub fn press(&mut self, now_ms: u64) -> Vec<Event> {
        let mut events = self.tick(now_ms);
        let in_window = matches!(self.phase, Phase::Face { .. } | Phase::Blank { .. });
        if self.paused_at.is_some() || !in_window || self.pressed.is_some() {
            return events;
        }
        let Some(kind) = self.current_trial().map(|trial| trial.kind) else {
            return events;
        };
        let elapsed = self.engine_time(now_ms).saturating_sub(self.trial_start);
        let rt_ms = u32::try_from(elapsed).unwrap_or(u32::MAX);
        self.pressed = Some(rt_ms);
        events.push(match kind {
            TrialKind::Repeat => Event::Hit { rt_ms },
            TrialKind::New => Event::Punish(Mistake::FalseAlarm),
        });
        events
    }

    /// Freezes the session, e.g. when the window loses focus. While paused,
    /// ticks and presses do nothing.
    pub fn pause(&mut self, now_ms: u64) {
        let running = !matches!(self.phase, Phase::Idle | Phase::Finished | Phase::Aborted);
        if running && self.paused_at.is_none() {
            self.paused_at = Some(now_ms);
        }
    }

    /// Continues a paused session; every pending deadline shifts by the time
    /// spent paused.
    pub fn resume(&mut self, now_ms: u64) {
        if let Some(paused_at) = self.paused_at.take() {
            self.paused_total += now_ms.saturating_sub(paused_at);
        }
    }

    /// Stops the session for good. Nothing is reported for an aborted session.
    pub fn abort(&mut self) {
        if self.phase != Phase::Finished {
            self.phase = Phase::Aborted;
            self.paused_at = None;
        }
    }

    #[must_use]
    pub fn state(&self) -> EngineState {
        match self.phase {
            Phase::Idle => EngineState::Idle,
            Phase::Finished => EngineState::Finished,
            Phase::Aborted => EngineState::Aborted,
            _ if self.paused_at.is_some() => EngineState::Paused,
            Phase::Countdown { .. } => EngineState::Countdown,
            _ => EngineState::Running,
        }
    }

    /// The next `n` trials not shown yet, for preloading their photos.
    #[must_use]
    pub fn upcoming(&self, n: usize) -> &[Trial] {
        let end = self.started.saturating_add(n).min(self.plan.trials.len());
        &self.plan.trials[self.started.min(end)..end]
    }

    /// Completed and total number of trials.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        (self.records.len(), self.plan.trials.len())
    }

    /// Exposure the next face will be shown at.
    #[must_use]
    pub fn exposure_ms(&self) -> u32 {
        self.exposure_ms
    }

    /// Outcome counts so far.
    #[must_use]
    pub fn counts(&self) -> Counts {
        self.counts
    }

    #[must_use]
    pub fn plan(&self) -> &SessionPlan {
        &self.plan
    }

    /// The trial whose response window is open, if any was shown yet.
    fn current_trial(&self) -> Option<&Trial> {
        self.started
            .checked_sub(1)
            .and_then(|i| self.plan.trials.get(i))
    }

    fn engine_time(&mut self, now_ms: u64) -> u64 {
        self.last_time = self.last_time.max(now_ms.saturating_sub(self.paused_total));
        self.last_time
    }

    /// Shows the next face at engine time `at`, or ends the session if none is left.
    fn begin_trial(&mut self, at: u64, events: &mut Vec<Event>) {
        let Some(trial) = self.plan.trials.get(self.started) else {
            self.phase = Phase::Finished;
            events.push(Event::Finished(Box::new(self.result())));
            return;
        };
        let hide_at = at + u64::from(self.exposure_ms);
        self.phase = Phase::Face {
            hide_at,
            end_at: hide_at + u64::from(isi_ms(self.exposure_ms)),
        };
        self.trial_start = at;
        self.pressed = None;
        events.push(Event::ShowFace {
            index: self.started,
            trial: trial.clone(),
            exposure_ms: self.exposure_ms,
        });
        self.started += 1;
    }

    /// Closes the response window of the current trial at engine time `at`.
    fn finish_trial(&mut self, at: u64, events: &mut Vec<Event>) {
        let Some(trial) = self.current_trial().cloned() else {
            self.phase = Phase::Gap { until: at };
            return;
        };
        let outcome = match (trial.kind, self.pressed) {
            (TrialKind::Repeat, Some(_)) => Outcome::Hit,
            (TrialKind::Repeat, None) => Outcome::Miss,
            (TrialKind::New, Some(_)) => Outcome::FalseAlarm,
            (TrialKind::New, None) => Outcome::CorrectRejection,
        };
        if outcome == Outcome::Miss {
            events.push(Event::Punish(Mistake::Miss));
        }
        self.records.push(TrialRecord {
            trial,
            outcome,
            rt_ms: self.pressed,
            exposure_ms: self.exposure_ms,
        });
        self.counts.record(outcome);
        self.block.record(outcome);

        if self.records.len() % BLOCK_SIZE == 0 {
            let adapted = adapt_exposure(self.exposure_ms, self.block.balanced_accuracy());
            self.block = Counts::default();
            if adapted != self.exposure_ms {
                self.exposure_ms = adapted;
                events.push(Event::ExposureChanged {
                    exposure_ms: adapted,
                });
            }
        }

        let pause = if outcome.is_mistake() {
            PUNISHMENT_PAUSE_MS
        } else {
            0
        };
        self.phase = Phase::Gap { until: at + pause };
    }

    fn result(&self) -> SessionResult {
        let hit_times: Vec<f64> = self
            .records
            .iter()
            .filter(|r| r.outcome == Outcome::Hit)
            .filter_map(|r| r.rt_ms.map(f64::from))
            .collect();
        let mean_rt_ms =
            (!hit_times.is_empty()).then(|| hit_times.iter().sum::<f64>() / hit_times.len() as f64);
        SessionResult {
            config: self.plan.config,
            counts: self.counts,
            balanced_accuracy: self.counts.balanced_accuracy(),
            d_prime: self.counts.d_prime(),
            mean_rt_ms,
            exposure_start_ms: self.exposure_start_ms,
            exposure_end_ms: self.exposure_ms,
            records: self.records.clone(),
        }
    }
}
