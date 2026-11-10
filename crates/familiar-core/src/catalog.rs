//! The set of people a session may draw from.
//!
//! A [`Catalog`] is a manifest minus the people the user excluded ("I know
//! this person"), plus per-person traits used to find lookalikes.

use std::collections::BTreeSet;

use crate::manifest::{APPEARANCE_ATTRS, Appearance, Manifest, Person};

/// Stable traits of a person, derived from all of their photos by majority vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersonTraits {
    pub male: bool,
    pub young: bool,
    /// The look this person has on most photos.
    pub typical: Appearance,
}

/// People available for training.
#[derive(Debug, Clone)]
pub struct Catalog {
    people: Vec<Person>,
    traits: Vec<PersonTraits>,
}

impl Catalog {
    /// Builds a catalog, dropping every person whose id is in `excluded`.
    #[must_use]
    pub fn new(manifest: Manifest, excluded: &BTreeSet<String>) -> Self {
        let people: Vec<Person> = manifest
            .people
            .into_iter()
            .filter(|p| !excluded.contains(&p.id))
            .collect();
        let traits = people.iter().map(traits_of).collect();
        Self { people, traits }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.people.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.people.is_empty()
    }

    #[must_use]
    pub fn people(&self) -> &[Person] {
        &self.people
    }

    /// Traits of the person at `index` (same order as [`Catalog::people`]).
    #[must_use]
    pub fn traits(&self, index: usize) -> Option<PersonTraits> {
        self.traits.get(index).copied()
    }

    /// How unlike two people are; lower means easier to confuse.
    ///
    /// `None` when they differ in sex or age group: such a pair is never a
    /// lookalike. Hair colour weighs three times as much as any other attribute.
    #[must_use]
    pub fn lookalike_distance(&self, a: usize, b: usize) -> Option<u32> {
        let (ta, tb) = (self.traits.get(a)?, self.traits.get(b)?);
        if ta.male != tb.male || ta.young != tb.young {
            return None;
        }
        let hair = ta.typical.hair_color().distance(tb.typical.hair_color());
        Some(ta.typical.distance(tb.typical) + 2 * hair)
    }

    /// Candidates most similar to `target`, best first, at most `limit`.
    #[must_use]
    pub fn lookalikes(
        &self,
        target: usize,
        candidates: impl Iterator<Item = usize>,
        limit: usize,
    ) -> Vec<usize> {
        let mut scored: Vec<(u32, usize)> = candidates
            .filter(|c| *c != target)
            .filter_map(|c| self.lookalike_distance(target, c).map(|d| (d, c)))
            .collect();
        scored.sort_unstable();
        scored.into_iter().take(limit).map(|(_, c)| c).collect()
    }
}

fn traits_of(person: &Person) -> PersonTraits {
    let total = person.photos.len();
    let majority = |count: usize| count * 2 > total;
    let mut typical = 0u32;
    for bit in 0..APPEARANCE_ATTRS.len() {
        let count = person
            .photos
            .iter()
            .filter(|p| p.appearance.0 & (1 << bit) != 0)
            .count();
        if majority(count) {
            typical |= 1 << bit;
        }
    }
    PersonTraits {
        male: majority(person.photos.iter().filter(|p| p.male).count()),
        young: majority(person.photos.iter().filter(|p| p.young).count()),
        typical: Appearance(typical),
    }
}
