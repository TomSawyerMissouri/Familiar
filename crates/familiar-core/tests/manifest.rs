//! Manifest parsing, validation and the catalog built from it.

mod common;

use std::collections::BTreeSet;

use familiar_core::{Catalog, Error, Manifest};

fn fixture() -> Manifest {
    Manifest::from_json(common::FIXTURE).unwrap()
}

#[test]
fn fixture_is_parsed() {
    let manifest = fixture();
    assert_eq!(manifest.people.len(), 4);
    let anna = &manifest.people[0];
    assert_eq!(anna.id, "anna");
    assert_eq!(anna.photos.len(), 3);
    assert_eq!(anna.photos[0].file, "anna/1.jpg");
    assert!(anna.photos[0].young && !anna.photos[0].male && !anna.photos[0].blurry);
    assert!(anna.photos[2].blurry);
    assert!(anna.photos[0].appearance.has("Blond_Hair"));
    assert!(anna.photos[0].appearance.has("Smiling"));
    assert!(!anna.photos[0].appearance.has("Eyeglasses"));
    assert!(!anna.photos[0].appearance.has("No_Such_Attribute"));
}

#[test]
fn appearance_distance_ignores_person_traits_and_quality() {
    let manifest = fixture();
    let anna = &manifest.people[0].photos;
    // Smiling vs Eyeglasses.
    assert_eq!(anna[0].appearance.distance(anna[1].appearance), 2);
    // Blond + Smiling vs Brown + Hat; Blurry does not count.
    assert_eq!(anna[0].appearance.distance(anna[2].appearance), 4);
    // Pale_Skin is a person trait, Smiling is the only appearance difference.
    let bella = &manifest.people[1].photos;
    assert_eq!(bella[0].appearance.distance(bella[1].appearance), 1);
}

#[test]
fn load_reads_a_file_and_reports_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    std::fs::write(&path, common::FIXTURE).unwrap();
    assert_eq!(Manifest::load(&path).unwrap(), fixture());

    let missing = Manifest::load(&dir.path().join("nope.json")).unwrap_err();
    assert!(matches!(missing, Error::Io { .. }), "{missing}");

    std::fs::write(&path, "{ broken").unwrap();
    let broken = Manifest::load(&path).unwrap_err();
    assert!(matches!(broken, Error::Json { .. }), "{broken}");
}

#[test]
fn invalid_manifests_are_rejected() {
    let cases = [
        ("no people", r#"{"people": []}"#),
        (
            "empty id",
            r#"{"people": [{"id": "", "photos": [{"file": "a/1.jpg"}]}]}"#,
        ),
        ("no photos", r#"{"people": [{"id": "a", "photos": []}]}"#),
        (
            "duplicate id",
            r#"{"people": [{"id": "a", "photos": [{"file": "a/1.jpg"}]},
                           {"id": "a", "photos": [{"file": "a/2.jpg"}]}]}"#,
        ),
        (
            "attribute out of range",
            r#"{"people": [{"id": "a", "photos": [{"file": "a/1.jpg", "attrs": {"Bald": -1}}]}]}"#,
        ),
        (
            "absolute path",
            r#"{"people": [{"id": "a", "photos": [{"file": "/etc/passwd"}]}]}"#,
        ),
        (
            "path escape",
            r#"{"people": [{"id": "a", "photos": [{"file": "../a/1.jpg"}]}]}"#,
        ),
        ("not a manifest", r#"[1, 2, 3]"#),
    ];
    for (name, json) in cases {
        let err = Manifest::from_json(json).expect_err(name);
        assert!(matches!(err, Error::InvalidManifest(_)), "{name}: {err}");
    }
}

#[test]
fn catalog_drops_excluded_people() {
    let excluded: BTreeSet<String> = ["bella".to_string(), "nobody".to_string()].into();
    let catalog = Catalog::new(fixture(), &excluded);
    let ids: Vec<&str> = catalog.people().iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["anna", "carl", "dora"]);
    assert_eq!(catalog.len(), 3);
    assert!(!catalog.is_empty());
}

#[test]
fn person_traits_follow_the_majority_of_photos() {
    let catalog = Catalog::new(fixture(), &BTreeSet::new());
    let anna = catalog.traits(0).unwrap();
    assert!(!anna.male && anna.young);
    // Blond on two photos of three, a hat on one only.
    assert!(anna.typical.has("Blond_Hair") && !anna.typical.has("Wearing_Hat"));
    let carl = catalog.traits(2).unwrap();
    assert!(carl.male && !carl.young);
    assert!(carl.typical.has("Black_Hair") && carl.typical.has("Mustache"));
    assert!(catalog.traits(99).is_none());
}

#[test]
fn lookalikes_share_sex_and_age_and_prefer_the_same_hair() {
    let catalog = Catalog::new(fixture(), &BTreeSet::new());
    let (anna, bella, carl, dora) = (0, 1, 2, 3);
    // A man is never a lookalike of a young woman.
    assert_eq!(catalog.lookalike_distance(anna, carl), None);
    let to_bella = catalog.lookalike_distance(anna, bella).unwrap();
    let to_dora = catalog.lookalike_distance(anna, dora).unwrap();
    assert!(
        to_bella < to_dora,
        "blond bella ({to_bella}) vs dark dora ({to_dora})"
    );
    assert_eq!(catalog.lookalikes(anna, 0..catalog.len(), 5), [bella, dora]);
    assert_eq!(catalog.lookalikes(anna, 0..catalog.len(), 1), [bella]);
    assert!(catalog.lookalikes(carl, 0..catalog.len(), 5).is_empty());
}
