//! Loading and validating `faces/manifest.json`.
//!
//! The manifest lists people and their photos. Every photo carries binary
//! attributes; the first 19 describe the *appearance* on that photo (glasses,
//! beard, hair colour...) and are packed into an [`Appearance`] bitmask so the
//! distance between two looks is a single XOR and popcount.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::Deserialize;

use crate::error::{Error, Result};

/// Attributes that describe how a person looks on one particular photo.
///
/// The position in this array is the bit index inside [`Appearance`].
pub const APPEARANCE_ATTRS: [&str; 19] = [
    "Eyeglasses",
    "Wearing_Hat",
    "No_Beard",
    "Mustache",
    "Goatee",
    "Sideburns",
    "Bald",
    "Bangs",
    "Black_Hair",
    "Blond_Hair",
    "Brown_Hair",
    "Gray_Hair",
    "Heavy_Makeup",
    "Wearing_Lipstick",
    "Smiling",
    "Mouth_Slightly_Open",
    "Straight_Hair",
    "Wavy_Hair",
    "Receding_Hairline",
];

/// Bits of [`Appearance`] that encode the hair colour.
pub const HAIR_COLOR_MASK: u32 = 0b1111 << 8;

/// Appearance attributes of one photo packed into a bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Appearance(pub u32);

impl Appearance {
    /// Number of appearance attributes that differ between two photos.
    #[must_use]
    pub fn distance(self, other: Self) -> u32 {
        (self.0 ^ other.0).count_ones()
    }

    /// Whether the named appearance attribute is set. Unknown names are `false`.
    #[must_use]
    pub fn has(self, attr: &str) -> bool {
        APPEARANCE_ATTRS
            .iter()
            .position(|a| *a == attr)
            .is_some_and(|bit| self.0 & (1 << bit) != 0)
    }

    /// Only the hair colour bits.
    #[must_use]
    pub fn hair_color(self) -> Self {
        Self(self.0 & HAIR_COLOR_MASK)
    }
}

/// One photo of a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo {
    /// Path relative to the `faces` directory, e.g. `2880/000123.jpg`.
    pub file: String,
    pub appearance: Appearance,
    pub male: bool,
    pub young: bool,
    pub pale_skin: bool,
    pub blurry: bool,
}

/// One person with all of their photos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub photos: Vec<Photo>,
}

/// A validated manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub source: String,
    pub people: Vec<Person>,
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    source: String,
    people: Vec<RawPerson>,
}

#[derive(Deserialize)]
struct RawPerson {
    id: String,
    photos: Vec<RawPhoto>,
}

#[derive(Deserialize)]
struct RawPhoto {
    file: String,
    #[serde(default)]
    attrs: BTreeMap<String, i64>,
}

impl Manifest {
    /// Reads and validates a manifest file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let raw: RawManifest = serde_json::from_str(&text).map_err(|source| Error::Json {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_raw(raw)
    }

    /// Parses and validates a manifest from a JSON string.
    pub fn from_json(json: &str) -> Result<Self> {
        let raw: RawManifest = serde_json::from_str(json)
            .map_err(|e| Error::InvalidManifest(format!("not a manifest: {e}")))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawManifest) -> Result<Self> {
        if raw.people.is_empty() {
            return Err(Error::InvalidManifest("no people".into()));
        }
        let mut seen = BTreeSet::new();
        let mut people = Vec::with_capacity(raw.people.len());
        for person in raw.people {
            if person.id.is_empty() {
                return Err(Error::InvalidManifest("person with an empty id".into()));
            }
            if !seen.insert(person.id.clone()) {
                return Err(Error::InvalidManifest(format!(
                    "duplicate person id {}",
                    person.id
                )));
            }
            if person.photos.is_empty() {
                return Err(Error::InvalidManifest(format!(
                    "person {} has no photos",
                    person.id
                )));
            }
            let photos = person
                .photos
                .into_iter()
                .map(photo_from_raw)
                .collect::<Result<_>>()?;
            people.push(Person {
                id: person.id,
                photos,
            });
        }
        Ok(Self {
            source: raw.source,
            people,
        })
    }
}

fn photo_from_raw(raw: RawPhoto) -> Result<Photo> {
    let path = Path::new(&raw.file);
    let is_plain = path.components().all(|c| matches!(c, Component::Normal(_)));
    if raw.file.is_empty() || !is_plain {
        return Err(Error::InvalidManifest(format!(
            "photo path {:?} must be relative and stay inside the faces directory",
            raw.file
        )));
    }
    let flag = |name: &str| -> Result<bool> {
        match raw.attrs.get(name) {
            None | Some(0) => Ok(false),
            Some(1) => Ok(true),
            Some(other) => Err(Error::InvalidManifest(format!(
                "attribute {name} of {} is {other}, expected 0 or 1",
                raw.file
            ))),
        }
    };
    let mut bits = 0u32;
    for (bit, name) in APPEARANCE_ATTRS.iter().enumerate() {
        if flag(name)? {
            bits |= 1 << bit;
        }
    }
    Ok(Photo {
        appearance: Appearance(bits),
        male: flag("Male")?,
        young: flag("Young")?,
        pale_skin: flag("Pale_Skin")?,
        blurry: flag("Blurry")?,
        file: raw.file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_differing_attributes() {
        let a = Appearance(0b1011);
        let b = Appearance(0b0110);
        assert_eq!(a.distance(b), 3);
        assert_eq!(a.distance(a), 0);
    }

    #[test]
    fn hair_mask_covers_the_four_hair_colours() {
        for name in ["Black_Hair", "Blond_Hair", "Brown_Hair", "Gray_Hair"] {
            let bit = APPEARANCE_ATTRS.iter().position(|a| *a == name).unwrap();
            assert_ne!(HAIR_COLOR_MASK & (1 << bit), 0, "{name}");
        }
        assert_eq!(HAIR_COLOR_MASK.count_ones(), 4);
    }
}
