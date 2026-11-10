//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};

use familiar_core::manifest::APPEARANCE_ATTRS;
use familiar_core::{Appearance, Catalog, Manifest};

/// The small hand-written manifest.
pub const FIXTURE: &str = include_str!("../fixtures/manifest.json");

/// Tiny deterministic generator, so the synthetic data never changes.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
}

/// JSON of a synthetic manifest with `people` persons of `photos` photos each.
///
/// Photo 0 is the person's base look, photo 1 differs from it in one
/// attribute, photo 2 in two, the rest in many. Sex and age group cycle over
/// the people so every combination is well represented.
pub fn synthetic_json(people: usize, photos: usize) -> String {
    let mut rng = Lcg(7);
    let mut out = Vec::with_capacity(people);
    for p in 0..people {
        let base = rng.next() & 0x7_ffff;
        let mut list = Vec::with_capacity(photos);
        for k in 0..photos {
            let look = match k {
                0 => base,
                1 => base ^ (1 << (rng.next() % 19)),
                2 => base ^ (0b11 << (rng.next() % 18)),
                _ => base ^ (rng.next() & 0x7_ffff),
            };
            let mut attrs = serde_json::Map::new();
            for (bit, name) in APPEARANCE_ATTRS.iter().enumerate() {
                attrs.insert((*name).into(), ((look >> bit) & 1).into());
            }
            attrs.insert("Male".into(), (p % 2).into());
            attrs.insert("Young".into(), ((p / 2) % 2).into());
            attrs.insert("Pale_Skin".into(), 0.into());
            attrs.insert("Blurry".into(), u8::from(p % 5 == 0 && k == 0).into());
            list.push(serde_json::json!({ "file": format!("{p}/{k:06}.jpg"), "attrs": attrs }));
        }
        out.push(serde_json::json!({ "id": p.to_string(), "photos": list }));
    }
    serde_json::json!({ "source": "synthetic", "attrs": [], "people": out }).to_string()
}

pub fn synthetic_manifest(people: usize, photos: usize) -> Manifest {
    Manifest::from_json(&synthetic_json(people, photos)).expect("synthetic manifest is valid")
}

pub fn synthetic_catalog(people: usize, photos: usize) -> Catalog {
    Catalog::new(synthetic_manifest(people, photos), &BTreeSet::new())
}

/// Appearance of every photo of a catalog, by file name.
pub fn looks(catalog: &Catalog) -> HashMap<String, Appearance> {
    catalog
        .people()
        .iter()
        .flat_map(|p| {
            p.photos
                .iter()
                .map(|photo| (photo.file.clone(), photo.appearance))
        })
        .collect()
}
