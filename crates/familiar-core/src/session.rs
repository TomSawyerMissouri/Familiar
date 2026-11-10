//! Session generation: who is shown when, on which photo, with which distortion.
//!
//! A session is planned in full before it starts, from a seed, so the same
//! seed over the same catalog always yields the same [`SessionPlan`].

use rand::seq::{IndexedRandom, SliceRandom};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::manifest::Person;
use crate::progress::Level;

/// Share of trials that repeat a person already shown in the session.
pub const REPEAT_SHARE: f64 = 0.35;
/// The first trials of a session are always new faces.
pub const WARMUP_TRIALS: usize = 3;
/// Minimum number of other faces between two showings of one person.
pub const MIN_LAG: usize = 2;
/// At level 5, the share of new faces picked to resemble someone already shown.
pub const LOOKALIKE_SHARE: f64 = 0.5;
/// A lookalike is drawn from this many best matches, for variety.
const LOOKALIKE_POOL: usize = 3;

/// Parameters of one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Number of trials.
    pub trials: usize,
    pub level: Level,
    /// Seed of the session's random generator.
    pub seed: u64,
}

/// Whether a trial shows a new person or one already seen in the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialKind {
    New,
    Repeat,
}

/// How a photo is altered when drawn. Pure data: the UI applies it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Distortion {
    /// Flip horizontally.
    pub mirror: bool,
    /// Rotation in degrees, positive is clockwise.
    pub rotation_deg: f32,
    /// Magnification, 1.0 shows the whole photo.
    pub zoom: f32,
    /// Where the zoomed crop sits, -1.0 (left edge) to 1.0 (right edge).
    pub offset_x: f32,
    /// Where the zoomed crop sits, -1.0 (top edge) to 1.0 (bottom edge).
    pub offset_y: f32,
    /// Brightness multiplier, 1.0 is unchanged.
    pub brightness: f32,
    pub grayscale: bool,
}

impl Distortion {
    /// The photo as it is.
    pub const NONE: Self = Self {
        mirror: false,
        rotation_deg: 0.0,
        zoom: 1.0,
        offset_x: 0.0,
        offset_y: 0.0,
        brightness: 1.0,
        grayscale: false,
    };

    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }
}

/// One presentation of a face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trial {
    pub person_id: String,
    /// Photo path relative to the `faces` directory.
    pub photo_file: String,
    pub kind: TrialKind,
    /// A new person chosen because they resemble someone already shown.
    pub lookalike: bool,
    pub distortion: Distortion,
}

/// A fully planned session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPlan {
    pub config: SessionConfig,
    pub trials: Vec<Trial>,
}

/// Number of repeat trials in a session of `trials` presentations.
#[must_use]
pub fn repeat_count(trials: usize) -> usize {
    let target = (trials as f64 * REPEAT_SHARE).round() as usize;
    target.min(trials.saturating_sub(WARMUP_TRIALS))
}

/// A person already shown in the session being planned.
struct Shown {
    person: usize,
    last_trial: usize,
    /// Photo indices in the order they were shown.
    photos: Vec<usize>,
}

/// Plans a session over `catalog`.
pub fn generate(catalog: &Catalog, config: &SessionConfig) -> Result<SessionPlan> {
    if config.trials == 0 {
        return Err(Error::InvalidConfig(
            "a session needs at least one trial".into(),
        ));
    }
    let repeats = repeat_count(config.trials);
    let needed = config.trials - repeats;
    if catalog.len() < needed {
        return Err(Error::NotEnoughPeople {
            needed,
            available: catalog.len(),
        });
    }

    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
    let level = config.level.get();

    let mut is_repeat = vec![false; config.trials];
    let mut slots: Vec<usize> = (WARMUP_TRIALS.min(config.trials)..config.trials).collect();
    slots.shuffle(&mut rng);
    for slot in slots.into_iter().take(repeats) {
        is_repeat[slot] = true;
    }

    let mut queue: Vec<usize> = (0..catalog.len()).collect();
    queue.shuffle(&mut rng);
    let mut used = vec![false; catalog.len()];
    let mut shown: Vec<Shown> = Vec::with_capacity(needed);
    let mut trials = Vec::with_capacity(config.trials);

    for (index, &wants_repeat) in is_repeat.iter().enumerate() {
        // A person may come back once MIN_LAG other faces have followed them.
        let eligible: Vec<usize> = if wants_repeat {
            (0..shown.len())
                .filter(|&s| shown[s].last_trial + MIN_LAG < index)
                .collect()
        } else {
            Vec::new()
        };

        let trial = if let Some(&slot) = eligible.choose(&mut rng) {
            let entry = &mut shown[slot];
            let person = &catalog.people()[entry.person];
            let photo = pick_repeat_photo(level, person, &entry.photos, &mut rng);
            entry.last_trial = index;
            entry.photos.push(photo);
            make_trial(person, photo, TrialKind::Repeat, false, level, &mut rng)
        } else {
            let lookalike = if level >= 5 && !shown.is_empty() && rng.random_bool(LOOKALIKE_SHARE) {
                pick_lookalike(catalog, &shown, &used, &mut rng)
            } else {
                None
            };
            let (index_in_catalog, is_lookalike) = match lookalike {
                Some(found) => (found, true),
                None => {
                    let next = queue.iter().copied().find(|&p| !used[p]).ok_or(
                        Error::NotEnoughPeople {
                            needed,
                            available: catalog.len(),
                        },
                    )?;
                    (next, false)
                }
            };
            used[index_in_catalog] = true;
            let person = &catalog.people()[index_in_catalog];
            let photo = pick_first_photo(person, &mut rng);
            shown.push(Shown {
                person: index_in_catalog,
                last_trial: index,
                photos: vec![photo],
            });
            make_trial(person, photo, TrialKind::New, is_lookalike, level, &mut rng)
        };
        trials.push(trial);
    }

    Ok(SessionPlan {
        config: *config,
        trials,
    })
}

fn make_trial(
    person: &Person,
    photo: usize,
    kind: TrialKind,
    lookalike: bool,
    level: u8,
    rng: &mut ChaCha8Rng,
) -> Trial {
    Trial {
        person_id: person.id.clone(),
        photo_file: person.photos[photo].file.clone(),
        kind,
        lookalike,
        distortion: distortion_for(level, rng),
    }
}

/// First photo of a new person: any sharp one, or any at all if all are blurry.
fn pick_first_photo(person: &Person, rng: &mut ChaCha8Rng) -> usize {
    let sharp: Vec<usize> = (0..person.photos.len())
        .filter(|&i| !person.photos[i].blurry)
        .collect();
    match sharp.choose(rng) {
        Some(&photo) => photo,
        None => rng.random_range(0..person.photos.len()),
    }
}

/// Photo for a repeat, by disguise level.
///
/// The "distance" of a candidate is its appearance distance to the nearest
/// photo of this person already shown in the session.
fn pick_repeat_photo(level: u8, person: &Person, shown: &[usize], rng: &mut ChaCha8Rng) -> usize {
    let first = shown.first().copied().unwrap_or(0);
    if level <= 1 {
        return first;
    }

    // Prefer photos not shown yet; if every photo was used, anything but the last one.
    let mut candidates: Vec<usize> = (0..person.photos.len())
        .filter(|i| !shown.contains(i))
        .collect();
    if candidates.is_empty() {
        let last = shown.last().copied();
        candidates = (0..person.photos.len())
            .filter(|i| Some(*i) != last)
            .collect();
    }
    if candidates.is_empty() {
        return first;
    }

    let distance = |candidate: usize| -> u32 {
        let look = person.photos[candidate].appearance;
        shown
            .iter()
            .map(|&s| look.distance(person.photos[s].appearance))
            .min()
            .unwrap_or(0)
    };
    let nearest = candidates.iter().map(|&c| distance(c)).min().unwrap_or(0);
    let farthest = candidates.iter().map(|&c| distance(c)).max().unwrap_or(0);

    let keep: Box<dyn Fn(u32) -> bool> = match level {
        // Similar look: within 2 attributes, or the closest there is.
        2 => Box::new(move |d| d <= nearest.max(2)),
        // Different look: at least 3 attributes apart when possible.
        3 if farthest >= 3 => Box::new(|d| d >= 3),
        3 => Box::new(|_| true),
        // Strong change: as far as possible from everything shown.
        _ => Box::new(move |d| d == farthest),
    };
    let pool: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&c| keep(distance(c)))
        .collect();
    pool.choose(rng).copied().unwrap_or(first)
}

/// An unused person resembling a randomly chosen person already shown.
fn pick_lookalike(
    catalog: &Catalog,
    shown: &[Shown],
    used: &[bool],
    rng: &mut ChaCha8Rng,
) -> Option<usize> {
    let target = shown.choose(rng)?.person;
    let unused = (0..catalog.len()).filter(|&p| !used[p]);
    catalog
        .lookalikes(target, unused, LOOKALIKE_POOL)
        .choose(rng)
        .copied()
}

/// Distortion for one trial. Applied to new faces and repeats alike, so that a
/// distorted picture never gives a repeat away.
fn distortion_for(level: u8, rng: &mut ChaCha8Rng) -> Distortion {
    if level < 4 {
        return Distortion::NONE;
    }
    let harsh = level >= 5;
    let max_zoom = if harsh { 1.5 } else { 1.3 };
    let min_brightness = if harsh { 0.55 } else { 0.8 };
    Distortion {
        mirror: rng.random_bool(0.5),
        rotation_deg: rng.random_range(-8.0f32..=8.0),
        zoom: rng.random_range(1.0f32..=max_zoom),
        offset_x: rng.random_range(-1.0f32..=1.0),
        offset_y: rng.random_range(-1.0f32..=1.0),
        brightness: rng.random_range(min_brightness..=1.2f32),
        grayscale: harsh && rng.random_bool(0.35),
    }
}
