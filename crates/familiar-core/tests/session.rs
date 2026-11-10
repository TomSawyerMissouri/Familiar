//! Session generation: repeats, lag, determinism and photo choice per level.

mod common;

use std::collections::{BTreeSet, HashMap, HashSet};

use familiar_core::session::{MIN_LAG, REPEAT_SHARE, WARMUP_TRIALS, repeat_count};
use familiar_core::{
    Appearance, Catalog, Error, Level, SessionConfig, SessionPlan, Trial, TrialKind, generate,
};

const LENGTHS: [usize; 4] = [40, 80, 120, 200];

fn plan(catalog: &Catalog, trials: usize, level: u8, seed: u64) -> SessionPlan {
    generate(
        catalog,
        &SessionConfig {
            trials,
            level: Level::new(level),
            seed,
        },
    )
    .unwrap()
}

fn repeats(plan: &SessionPlan) -> impl Iterator<Item = (usize, &Trial)> {
    plan.trials
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TrialKind::Repeat)
}

/// For every repeat: the photo chosen, the photos of that person shown before
/// it, and the photos not shown yet (the candidates it was chosen from).
struct RepeatChoice<'a> {
    chosen: &'a str,
    shown: Vec<&'a str>,
    unshown: Vec<&'a str>,
}

fn repeat_choices<'a>(catalog: &'a Catalog, plan: &'a SessionPlan) -> Vec<RepeatChoice<'a>> {
    let photos: HashMap<&str, Vec<&str>> = catalog
        .people()
        .iter()
        .map(|p| {
            (
                p.id.as_str(),
                p.photos.iter().map(|ph| ph.file.as_str()).collect(),
            )
        })
        .collect();
    let mut shown: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut choices = Vec::new();
    for trial in &plan.trials {
        let before = shown.entry(trial.person_id.as_str()).or_default();
        if trial.kind == TrialKind::Repeat {
            let unshown = photos[trial.person_id.as_str()]
                .iter()
                .copied()
                .filter(|f| !before.contains(f));
            choices.push(RepeatChoice {
                chosen: &trial.photo_file,
                shown: before.clone(),
                unshown: unshown.collect(),
            });
        }
        before.push(&trial.photo_file);
    }
    choices
}

/// Appearance distance from `photo` to the nearest photo in `shown`.
fn distance(looks: &HashMap<String, Appearance>, photo: &str, shown: &[&str]) -> u32 {
    shown
        .iter()
        .map(|s| looks[photo].distance(looks[*s]))
        .min()
        .unwrap()
}

#[test]
fn repeat_share_lag_and_warmup_hold_over_many_sessions() {
    let catalog = common::synthetic_catalog(160, 12);
    for trials in LENGTHS {
        for seed in 0..150 {
            let plan = plan(&catalog, trials, 3, seed);
            assert_eq!(plan.trials.len(), trials);

            let share = repeats(&plan).count() as f64 / trials as f64;
            assert!(
                (share - REPEAT_SHARE).abs() <= 0.02,
                "share {share} at {trials}/{seed}"
            );
            assert_eq!(repeats(&plan).count(), repeat_count(trials));

            let mut last_seen: HashMap<&str, usize> = HashMap::new();
            for (index, trial) in plan.trials.iter().enumerate() {
                let previous = last_seen.insert(&trial.person_id, index);
                match trial.kind {
                    TrialKind::New => assert_eq!(previous, None, "new person shown twice"),
                    TrialKind::Repeat => {
                        assert!(index >= WARMUP_TRIALS, "repeat during warm-up at {index}");
                        let previous = previous.expect("a repeat must have been shown before");
                        let between = index - previous - 1;
                        assert!(between >= MIN_LAG, "only {between} faces between showings");
                    }
                }
            }
        }
    }
}

#[test]
fn people_can_repeat_more_than_once() {
    let catalog = common::synthetic_catalog(160, 12);
    let plan = plan(&catalog, 200, 3, 1);
    let mut showings: HashMap<&str, usize> = HashMap::new();
    for trial in &plan.trials {
        *showings.entry(&trial.person_id).or_default() += 1;
    }
    assert!(showings.values().any(|&n| n >= 3));
}

#[test]
fn same_seed_gives_the_same_session() {
    let catalog = common::synthetic_catalog(160, 12);
    for level in 1..=5 {
        assert_eq!(plan(&catalog, 80, level, 42), plan(&catalog, 80, level, 42));
        assert_ne!(
            plan(&catalog, 80, level, 42).trials,
            plan(&catalog, 80, level, 43).trials
        );
    }
}

#[test]
fn short_sessions_are_valid() {
    let catalog = common::synthetic_catalog(10, 3);
    for trials in 1..=6 {
        let plan = plan(&catalog, trials, 2, 5);
        assert_eq!(plan.trials.len(), trials);
        assert!(
            plan.trials
                .iter()
                .take(WARMUP_TRIALS)
                .all(|t| t.kind == TrialKind::New)
        );
    }
}

#[test]
fn impossible_sessions_are_rejected() {
    let catalog = common::synthetic_catalog(10, 3);
    let config = |trials| SessionConfig {
        trials,
        level: Level::MIN,
        seed: 0,
    };
    assert!(matches!(
        generate(&catalog, &config(0)),
        Err(Error::InvalidConfig(_))
    ));
    // 80 trials need 52 different people.
    let err = generate(&catalog, &config(80)).unwrap_err();
    assert!(
        matches!(
            err,
            Error::NotEnoughPeople {
                needed: 52,
                available: 10
            }
        ),
        "{err}"
    );
}

#[test]
fn excluded_people_never_appear() {
    let excluded: BTreeSet<String> = (0..100).map(|p| p.to_string()).collect();
    let catalog = Catalog::new(common::synthetic_manifest(160, 12), &excluded);
    for level in [1, 5] {
        for seed in 0..20 {
            let plan = plan(&catalog, 80, level, seed);
            assert!(plan.trials.iter().all(|t| !excluded.contains(&t.person_id)));
        }
    }
}

#[test]
fn first_photo_of_a_person_is_sharp_when_possible() {
    let catalog = common::synthetic_catalog(160, 12);
    let blurry: HashSet<&str> = catalog
        .people()
        .iter()
        .flat_map(|p| &p.photos)
        .filter(|p| p.blurry)
        .map(|p| p.file.as_str())
        .collect();
    assert!(!blurry.is_empty());
    for seed in 0..20 {
        let plan = plan(&catalog, 200, 1, seed);
        assert!(
            plan.trials
                .iter()
                .all(|t| !blurry.contains(t.photo_file.as_str()))
        );
    }
}

#[test]
fn level_1_repeats_the_very_same_photo() {
    let catalog = common::synthetic_catalog(160, 12);
    for seed in 0..20 {
        let plan = plan(&catalog, 200, 1, seed);
        for choice in repeat_choices(&catalog, &plan) {
            assert_eq!(choice.chosen, choice.shown[0]);
        }
    }
}

#[test]
fn level_2_shows_another_photo_with_a_similar_look() {
    let catalog = common::synthetic_catalog(160, 12);
    let looks = common::looks(&catalog);
    let mut within_two = 0;
    for seed in 0..20 {
        let plan = plan(&catalog, 200, 2, seed);
        for choice in repeat_choices(&catalog, &plan) {
            assert!(
                !choice.shown.contains(&choice.chosen),
                "must be a different photo"
            );
            let chosen = distance(&looks, choice.chosen, &choice.shown);
            let nearest = choice
                .unshown
                .iter()
                .map(|c| distance(&looks, c, &choice.shown))
                .min()
                .unwrap();
            // Within 2 attributes, or the closest available when nothing is that close.
            assert!(
                chosen <= nearest.max(2),
                "distance {chosen}, nearest {nearest}"
            );
            within_two += usize::from(chosen <= 2);
        }
    }
    assert!(within_two > 0);
}

#[test]
fn level_3_prefers_a_clearly_different_look() {
    let catalog = common::synthetic_catalog(160, 12);
    let looks = common::looks(&catalog);
    for seed in 0..20 {
        let plan = plan(&catalog, 200, 3, seed);
        for choice in repeat_choices(&catalog, &plan) {
            assert!(
                !choice.shown.contains(&choice.chosen),
                "must be a different photo"
            );
            let chosen = distance(&looks, choice.chosen, &choice.shown);
            let farthest = choice
                .unshown
                .iter()
                .map(|c| distance(&looks, c, &choice.shown))
                .max()
                .unwrap();
            assert!(
                chosen >= 3 || farthest < 3,
                "distance {chosen}, farthest {farthest}"
            );
        }
    }
}

#[test]
fn levels_4_and_5_pick_the_most_different_look() {
    let catalog = common::synthetic_catalog(160, 12);
    let looks = common::looks(&catalog);
    for level in [4, 5] {
        for seed in 0..20 {
            let plan = plan(&catalog, 200, level, seed);
            for choice in repeat_choices(&catalog, &plan) {
                assert!(
                    !choice.shown.contains(&choice.chosen),
                    "must be a different photo"
                );
                let chosen = distance(&looks, choice.chosen, &choice.shown);
                let farthest = choice
                    .unshown
                    .iter()
                    .map(|c| distance(&looks, c, &choice.shown))
                    .max()
                    .unwrap();
                assert_eq!(chosen, farthest);
            }
        }
    }
}

#[test]
fn a_person_with_one_photo_repeats_that_photo_on_every_level() {
    let catalog = common::synthetic_catalog(60, 1);
    for level in 1..=5 {
        let plan = plan(&catalog, 80, level, 3);
        for choice in repeat_choices(&catalog, &plan) {
            assert_eq!(choice.chosen, choice.shown[0]);
        }
    }
}

#[test]
fn distortions_start_at_level_4_and_get_harsher_at_level_5() {
    let catalog = common::synthetic_catalog(160, 12);
    for level in 1..=3 {
        assert!(
            plan(&catalog, 200, level, 9)
                .trials
                .iter()
                .all(|t| t.distortion.is_none())
        );
    }

    for (level, max_zoom, min_brightness) in [(4, 1.3, 0.8), (5, 1.5, 0.55)] {
        let plan = plan(&catalog, 200, level, 9);
        for trial in &plan.trials {
            let d = trial.distortion;
            assert!((-8.0..=8.0).contains(&d.rotation_deg), "{d:?}");
            assert!((1.0..=max_zoom).contains(&d.zoom), "{d:?}");
            assert!((min_brightness..=1.2).contains(&d.brightness), "{d:?}");
            assert!((-1.0..=1.0).contains(&d.offset_x) && (-1.0..=1.0).contains(&d.offset_y));
            assert!(level == 5 || !d.grayscale, "grayscale only at level 5");
        }
        // New faces are distorted as much as repeats, so distortion is no cue.
        for kind in [TrialKind::New, TrialKind::Repeat] {
            let of_kind = || plan.trials.iter().filter(move |t| t.kind == kind);
            assert!(of_kind().all(|t| !t.distortion.is_none()));
            assert!(of_kind().any(|t| t.distortion.mirror));
            assert!(of_kind().any(|t| !t.distortion.mirror));
        }
    }

    let harsh = plan(&catalog, 200, 5, 9);
    assert!(harsh.trials.iter().any(|t| t.distortion.grayscale));
    assert!(harsh.trials.iter().any(|t| t.distortion.zoom > 1.3));
    assert!(harsh.trials.iter().any(|t| t.distortion.brightness < 0.8));
}

#[test]
fn lookalikes_appear_only_at_level_5_and_resemble_someone_shown() {
    let catalog = common::synthetic_catalog(400, 6);
    let index_of: HashMap<&str, usize> = catalog
        .people()
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();

    for level in 1..=4 {
        assert!(
            plan(&catalog, 200, level, 4)
                .trials
                .iter()
                .all(|t| !t.lookalike)
        );
    }

    let mut total = 0;
    // Distance to the most similar person shown before, for lookalikes and for
    // ordinary new faces.
    let (mut lookalike_distances, mut stranger_distances) = (Vec::new(), Vec::new());
    for seed in 0..10 {
        let plan = plan(&catalog, 80, 5, seed);
        let mut shown: Vec<usize> = Vec::new();
        for trial in &plan.trials {
            let person = index_of[trial.person_id.as_str()];
            let nearest = shown
                .iter()
                .filter_map(|&s| catalog.lookalike_distance(s, person))
                .min();
            if trial.lookalike {
                total += 1;
                assert_eq!(trial.kind, TrialKind::New, "a lookalike is a new person");
                let nearest = nearest.expect("same sex and age group as somebody shown");
                lookalike_distances.push(f64::from(nearest));
            } else if let (TrialKind::New, Some(nearest)) = (trial.kind, nearest) {
                stranger_distances.push(f64::from(nearest));
            }
            if trial.kind == TrialKind::New {
                shown.push(person);
            }
        }
    }
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    let (lookalikes, strangers) = (mean(&lookalike_distances), mean(&stranger_distances));
    assert!(
        lookalikes + 2.0 < strangers,
        "lookalikes {lookalikes}, strangers {strangers}"
    );
    // About half of the new faces after the first are lookalikes.
    assert!(
        (150..=350).contains(&total),
        "{total} lookalikes in 10 sessions"
    );
}
