//! The session state machine: timing, scoring of presses, adaptation.

mod common;

use familiar_core::engine::{COUNTDOWN_STEP_MS, PUNISHMENT_PAUSE_MS};
use familiar_core::scoring::{MAX_EXPOSURE_MS, MIN_EXPOSURE_MS};
use familiar_core::{
    Counts, Distortion, Engine, EngineState, Event, Level, Mistake, Outcome, Progress,
    SessionConfig, SessionPlan, SessionRecord, SessionResult, Trial, TrialKind, generate,
};

use TrialKind::{New, Repeat};

/// Time from `start` to the first face.
const FIRST_FACE: u64 = 3 * COUNTDOWN_STEP_MS;

/// A hand-made plan: only the kinds matter to the engine.
fn plan(kinds: &[TrialKind]) -> SessionPlan {
    let trials = kinds
        .iter()
        .enumerate()
        .map(|(i, &kind)| Trial {
            person_id: format!("p{i}"),
            photo_file: format!("p{i}/1.jpg"),
            kind,
            lookalike: false,
            distortion: Distortion::NONE,
        })
        .collect();
    SessionPlan {
        config: SessionConfig {
            trials: kinds.len(),
            level: Level::MIN,
            seed: 0,
        },
        trials,
    }
}

/// A plan of `blocks` blocks of ten trials, each with three repeats.
fn blocks(blocks: usize) -> SessionPlan {
    let block = [New, New, New, Repeat, New, New, Repeat, New, Repeat, New];
    plan(&block.repeat(blocks))
}

/// Plays a whole session millisecond by millisecond. `respond` decides, when a
/// face appears, whether to press and how long after its onset.
fn play(
    engine: &mut Engine,
    mut respond: impl FnMut(usize, &Trial) -> Option<u64>,
) -> (Vec<(u64, Event)>, SessionResult) {
    let mut log: Vec<(u64, Event)> = engine.start(0).into_iter().map(|e| (0, e)).collect();
    let mut press_at = None;
    for now in 1..10_000_000 {
        let mut events = engine.tick(now);
        if press_at == Some(now) {
            press_at = None;
            events.extend(engine.press(now));
        }
        for event in events {
            if let Event::ShowFace { index, trial, .. } = &event {
                press_at = respond(*index, trial).map(|rt| now + rt);
            }
            if let Event::Finished(result) = &event {
                let result = (**result).clone();
                log.push((now, event));
                return (log, result);
            }
            log.push((now, event));
        }
    }
    panic!("the session never finished");
}

fn outcomes(result: &SessionResult) -> Vec<Outcome> {
    result.records.iter().map(|r| r.outcome).collect()
}

fn face_onsets(log: &[(u64, Event)]) -> Vec<u64> {
    log.iter()
        .filter(|(_, e)| matches!(e, Event::ShowFace { .. }))
        .map(|(t, _)| *t)
        .collect()
}

#[test]
fn countdown_runs_before_the_first_face() {
    let mut engine = Engine::new(plan(&[New]), 1500);
    assert_eq!(engine.state(), EngineState::Idle);
    assert!(engine.tick(500).is_empty(), "nothing happens before start");

    assert_eq!(engine.start(1000), [Event::Countdown(3)]);
    assert_eq!(engine.state(), EngineState::Countdown);
    assert!(engine.start(1100).is_empty(), "a second start is ignored");
    assert!(engine.tick(1799).is_empty());
    assert_eq!(engine.tick(1800), [Event::Countdown(2)]);
    assert_eq!(engine.tick(2600), [Event::Countdown(1)]);
    assert!(
        engine.press(2700).is_empty(),
        "presses during the countdown are ignored"
    );

    let events = engine.tick(3400);
    assert!(
        matches!(&events[..], [Event::ShowFace { index: 0, exposure_ms: 1500, trial }] if trial.person_id == "p0")
    );
    assert_eq!(engine.state(), EngineState::Running);
}

#[test]
fn a_trial_is_exposure_then_blank_interval() {
    let mut engine = Engine::new(plan(&[New, New]), 1500);
    engine.start(0);
    engine.tick(FIRST_FACE);
    assert!(engine.tick(FIRST_FACE + 1499).is_empty());
    assert_eq!(engine.tick(FIRST_FACE + 1500), [Event::HideFace]);
    // ISI = max(150, 0.25 * 1500) = 375 ms.
    assert!(engine.tick(FIRST_FACE + 1874).is_empty());
    let next = engine.tick(FIRST_FACE + 1875);
    assert!(
        matches!(&next[..], [Event::ShowFace { index: 1, .. }]),
        "{next:?}"
    );
    assert_eq!(engine.progress(), (1, 2));

    // The blank interval never drops below 150 ms.
    let mut fast = Engine::new(plan(&[New, New]), 400);
    fast.start(0);
    fast.tick(FIRST_FACE);
    assert_eq!(fast.tick(FIRST_FACE + 400), [Event::HideFace]);
    assert!(fast.tick(FIRST_FACE + 549).is_empty());
    assert!(matches!(
        &fast.tick(FIRST_FACE + 550)[..],
        [Event::ShowFace { index: 1, .. }]
    ));
}

#[test]
fn every_combination_of_trial_kind_and_press_is_scored() {
    let kinds = [New, New, New, Repeat, Repeat, New];
    let mut engine = Engine::new(plan(&kinds), 1000);
    // Press on trials 1, 3 and 5.
    let (log, result) = play(&mut engine, |index, _| (index % 2 == 1).then_some(400));

    use Outcome::{CorrectRejection, FalseAlarm, Hit, Miss};
    assert_eq!(
        outcomes(&result),
        [
            CorrectRejection,
            FalseAlarm,
            CorrectRejection,
            Hit,
            Miss,
            FalseAlarm
        ]
    );
    assert_eq!(
        result.counts,
        Counts {
            hits: 1,
            misses: 1,
            false_alarms: 2,
            correct_rejections: 2
        }
    );
    assert_eq!(result.balanced_accuracy, 0.5);
    assert_eq!(result.mean_rt_ms, Some(400.0));
    let rts: Vec<_> = result.records.iter().map(|r| r.rt_ms).collect();
    assert_eq!(rts, [None, Some(400), None, Some(400), None, Some(400)]);

    let feedback: Vec<&Event> = log
        .iter()
        .map(|(_, e)| e)
        .filter(|e| matches!(e, Event::Hit { .. } | Event::Punish(_)))
        .collect();
    assert_eq!(
        feedback,
        [
            &Event::Punish(Mistake::FalseAlarm),
            &Event::Hit { rt_ms: 400 },
            &Event::Punish(Mistake::Miss),
            &Event::Punish(Mistake::FalseAlarm),
        ]
    );
    assert_eq!(engine.state(), EngineState::Finished);
}

#[test]
fn false_alarm_is_punished_at_the_press_and_miss_at_the_end_of_the_window() {
    let mut engine = Engine::new(plan(&[New, New, New, Repeat, New]), 1000);
    let (log, _) = play(&mut engine, |index, _| (index == 1).then_some(250));
    let onsets = face_onsets(&log);
    let at = |wanted: &Event| {
        log.iter()
            .find(|(_, e)| e == wanted)
            .map(|(t, _)| *t)
            .unwrap()
    };

    assert_eq!(at(&Event::Punish(Mistake::FalseAlarm)), onsets[1] + 250);
    // The window is exposure + ISI = 1000 + 250 ms.
    assert_eq!(at(&Event::Punish(Mistake::Miss)), onsets[3] + 1250);
}

#[test]
fn a_punished_trial_is_followed_by_an_extra_pause() {
    let mut engine = Engine::new(plan(&[New, New, New, Repeat, New]), 1000);
    let (log, _) = play(&mut engine, |index, _| (index == 1).then_some(250));
    let onsets = face_onsets(&log);
    let gaps: Vec<u64> = onsets.windows(2).map(|w| w[1] - w[0]).collect();
    // Correct rejection, false alarm, correct rejection, miss.
    assert_eq!(
        gaps,
        [
            1250,
            1250 + PUNISHMENT_PAUSE_MS,
            1250,
            1250 + PUNISHMENT_PAUSE_MS
        ]
    );
    assert_eq!(onsets[0], FIRST_FACE);
}

#[test]
fn only_the_first_press_in_a_window_counts() {
    let mut engine = Engine::new(plan(&[New, New, New, Repeat, New]), 1000);
    engine.start(0);
    // Jump to the onset of the repeat: three correct rejections of 1250 ms each.
    let onset = FIRST_FACE + 3 * 1250;
    let shown = engine.tick(onset);
    assert!(
        matches!(shown.last(), Some(Event::ShowFace { index: 3, .. })),
        "{shown:?}"
    );

    assert_eq!(engine.press(onset + 300), [Event::Hit { rt_ms: 300 }]);
    assert!(engine.press(onset + 350).is_empty());
    assert!(engine.press(onset + 900).is_empty());

    // Same on a new face: one false alarm, not three.
    let next = onset + 1250;
    engine.tick(next);
    assert_eq!(
        engine.press(next + 100),
        [Event::Punish(Mistake::FalseAlarm)]
    );
    assert!(engine.press(next + 200).is_empty());
    assert_eq!(engine.counts().hits, 1);
    engine.tick(next + 5000);
    assert_eq!(engine.counts().false_alarms, 1);
}

#[test]
fn a_press_during_the_blank_interval_still_counts() {
    let mut engine = Engine::new(plan(&[New, New, New, Repeat]), 1000);
    // 1100 ms: the face is hidden at 1000, the window closes at 1250.
    let (log, result) = play(&mut engine, |index, _| (index == 3).then_some(1100));
    assert_eq!(result.counts.hits, 1);
    assert!(log.iter().any(|(_, e)| *e == Event::Hit { rt_ms: 1100 }));
}

#[test]
fn a_press_in_the_punishment_pause_is_ignored() {
    let mut engine = Engine::new(plan(&[New, New, New, Repeat, New]), 1000);
    engine.start(0);
    let onset = FIRST_FACE + 3 * 1250;
    engine.tick(onset);
    // The repeat is missed; its window closed at +1250, the pause lasts 320 ms.
    assert_eq!(
        engine.press(onset + 1300),
        [Event::HideFace, Event::Punish(Mistake::Miss)]
    );
    assert!(engine.press(onset + 1400).is_empty());
    assert_eq!(
        engine.counts(),
        Counts {
            misses: 1,
            correct_rejections: 3,
            ..Counts::default()
        }
    );
}

#[test]
fn perfect_play_speeds_up_after_every_block() {
    let mut engine = Engine::new(blocks(8), 1500);
    let (log, result) = play(&mut engine, |_, trial| {
        (trial.kind == Repeat).then_some(400)
    });

    assert_eq!(
        result.counts,
        Counts {
            hits: 24,
            misses: 0,
            false_alarms: 0,
            correct_rejections: 56
        }
    );
    assert_eq!(result.balanced_accuracy, 1.0);
    assert_eq!(result.exposure_start_ms, 1500);

    // 1500 * 0.88 per block, rounded each time.
    let expected = [1500, 1320, 1162, 1023, 900, 792, 697, 613];
    for (block, exposure) in expected.into_iter().enumerate() {
        for record in &result.records[block * 10..(block + 1) * 10] {
            assert_eq!(record.exposure_ms, exposure, "block {block}");
        }
    }
    assert_eq!(result.exposure_end_ms, 539);
    assert_eq!(engine.exposure_ms(), 539);

    let changes: Vec<u32> = log
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ExposureChanged { exposure_ms } => Some(*exposure_ms),
            _ => None,
        })
        .collect();
    assert_eq!(changes, [1320, 1162, 1023, 900, 792, 697, 613, 539]);
}

#[test]
fn poor_play_slows_down_and_middling_play_holds() {
    // Never pressing: hit rate 0, correct rejection rate 1, accuracy 0.5.
    let mut never = Engine::new(blocks(3), 1000);
    let (_, result) = play(&mut never, |_, _| None);
    assert_eq!(result.counts.misses, 9);
    assert_eq!(result.balanced_accuracy, 0.5);
    assert_eq!(result.exposure_end_ms, 1404); // 1000 -> 1120 -> 1254 -> 1404, rounded per block
    assert_eq!(result.mean_rt_ms, None);

    // Two hits of three per block: accuracy (2/3 + 1) / 2 = 0.83, no change.
    let mut seen = 0;
    let mut middling = Engine::new(blocks(3), 1000);
    let (log, result) = play(&mut middling, |_, trial| {
        seen += usize::from(trial.kind == Repeat);
        (trial.kind == Repeat && seen % 3 != 0).then_some(300)
    });
    assert_eq!(result.exposure_end_ms, 1000);
    assert!(
        !log.iter()
            .any(|(_, e)| matches!(e, Event::ExposureChanged { .. }))
    );
}

#[test]
fn exposure_never_leaves_its_bounds() {
    let mut fast = Engine::new(blocks(4), 270);
    let (_, result) = play(&mut fast, |_, trial| (trial.kind == Repeat).then_some(100));
    assert_eq!(result.exposure_end_ms, MIN_EXPOSURE_MS);
    assert!(
        result
            .records
            .iter()
            .all(|r| r.exposure_ms >= MIN_EXPOSURE_MS)
    );

    let mut slow = Engine::new(blocks(4), 2900);
    let (_, result) = play(&mut slow, |_, trial| (trial.kind == New).then_some(100));
    assert_eq!(result.exposure_end_ms, MAX_EXPOSURE_MS);
    assert!(
        result
            .records
            .iter()
            .all(|r| r.exposure_ms <= MAX_EXPOSURE_MS)
    );

    assert_eq!(Engine::new(blocks(1), 10).exposure_ms(), MIN_EXPOSURE_MS);
    assert_eq!(
        Engine::new(blocks(1), 99_999).exposure_ms(),
        MAX_EXPOSURE_MS
    );
}

#[test]
fn one_late_tick_replays_everything_that_was_due() {
    let mut engine = Engine::new(blocks(2), 1000);
    engine.start(0);
    let events = engine.tick(10_000_000);

    let faces = events
        .iter()
        .filter(|e| matches!(e, Event::ShowFace { .. }))
        .count();
    assert_eq!(faces, 20);
    let Some(Event::Finished(result)) = events.last() else {
        panic!("the last event must be Finished: {:?}", events.last());
    };
    assert_eq!(
        result.counts,
        Counts {
            hits: 0,
            misses: 6,
            false_alarms: 0,
            correct_rejections: 14
        }
    );
    assert_eq!(engine.state(), EngineState::Finished);
    assert!(engine.tick(20_000_000).is_empty());
    assert!(engine.press(20_000_001).is_empty());
}

#[test]
fn sparse_ticks_do_not_drift() {
    // Ticking every 7 ms or every 1 ms must give the same reaction times.
    let run = |step: usize| {
        let mut engine = Engine::new(plan(&[New, New, New, Repeat]), 1000);
        engine.start(0);
        let onset = FIRST_FACE + 3 * 1250;
        let mut events = Vec::new();
        for now in (0..onset + 500).step_by(step) {
            events.extend(engine.tick(now));
        }
        events.extend(engine.press(onset + 500));
        events
    };
    assert_eq!(run(7), run(1));
    assert_eq!(run(7).last(), Some(&Event::Hit { rt_ms: 500 }));
}

#[test]
fn pause_freezes_the_session_and_resume_shifts_the_deadlines() {
    let mut engine = Engine::new(plan(&[New, New, New, Repeat]), 1000);
    engine.start(0);
    engine.tick(FIRST_FACE);
    engine.pause(FIRST_FACE + 600);
    assert_eq!(engine.state(), EngineState::Paused);
    assert!(engine.tick(FIRST_FACE + 5000).is_empty());
    assert!(engine.press(FIRST_FACE + 5000).is_empty());

    // Paused for 10 s: the face had 400 ms of exposure left.
    engine.resume(FIRST_FACE + 10_600);
    assert_eq!(engine.state(), EngineState::Running);
    assert!(engine.tick(FIRST_FACE + 10_999).is_empty());
    assert_eq!(engine.tick(FIRST_FACE + 11_000), [Event::HideFace]);
    assert_eq!(
        engine.counts(),
        Counts::default(),
        "the ignored press was not scored"
    );

    // Reaction times do not include the pause.
    let onset = FIRST_FACE + 10_000 + 3 * 1250;
    engine.tick(onset);
    engine.pause(onset + 100);
    engine.resume(onset + 2100);
    assert_eq!(engine.press(onset + 2300), [Event::Hit { rt_ms: 300 }]);
}

#[test]
fn an_aborted_session_reports_nothing() {
    let mut engine = Engine::new(blocks(1), 1000);
    engine.start(0);
    engine.tick(FIRST_FACE + 100);
    engine.abort();
    assert_eq!(engine.state(), EngineState::Aborted);
    assert!(engine.tick(10_000_000).is_empty());
    assert!(engine.press(10_000_001).is_empty());
    assert!(engine.start(10_000_002).is_empty());
}

#[test]
fn upcoming_lists_the_trials_to_preload() {
    let mut engine = Engine::new(plan(&[New, New, New, Repeat, New]), 1000);
    let ids = |engine: &Engine, n| -> Vec<String> {
        engine
            .upcoming(n)
            .iter()
            .map(|t| t.person_id.clone())
            .collect()
    };
    assert_eq!(ids(&engine, 2), ["p0", "p1"]);
    engine.start(0);
    engine.tick(FIRST_FACE);
    assert_eq!(
        ids(&engine, 2),
        ["p1", "p2"],
        "the face on screen is no longer upcoming"
    );
    assert_eq!(ids(&engine, 99), ["p1", "p2", "p3", "p4"]);
    engine.tick(10_000_000);
    assert!(engine.upcoming(3).is_empty());
    assert_eq!(engine.progress(), (5, 5));
}

#[test]
fn a_generated_session_plays_through_to_history_and_progress() {
    let catalog = common::synthetic_catalog(160, 12);
    let config = SessionConfig {
        trials: 80,
        level: Level::new(2),
        seed: 2026,
    };
    let play_once = || {
        let plan = generate(&catalog, &config).unwrap();
        let mut engine = Engine::new(plan, 1500);
        play(&mut engine, |_, trial| {
            (trial.kind == Repeat).then_some(350)
        })
        .1
    };
    let result = play_once();
    assert_eq!(
        result,
        play_once(),
        "same seed and same presses give the same result"
    );

    assert_eq!(result.counts.total(), 80);
    assert_eq!(result.counts.hits, 28);
    assert_eq!(result.balanced_accuracy, 1.0);
    assert!(result.d_prime > 4.0);
    assert_eq!(result.mean_rt_ms, Some(350.0));

    let record = SessionRecord::from_result(&result, 1_794_300_000_000);
    assert_eq!(
        (record.seed, record.level, record.trials),
        (2026, Level::new(2), 80)
    );
    assert_eq!(record.exposure_end_ms, result.exposure_end_ms);

    let start = Progress {
        level: Level::new(2),
        exposure_ms: 1500,
    };
    let next = start.after_session(result.balanced_accuracy, result.exposure_end_ms, true);
    assert_eq!(next.level, Level::new(3));
    assert_eq!(next.exposure_ms, 620); // 539 * 1.15
}
