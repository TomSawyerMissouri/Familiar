//! Smoke test against the real face set. Ignored by default: the data is not
//! in the repository. Run with `cargo test -p familiar-core -- --ignored`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use familiar_core::{Catalog, Level, Manifest, SessionConfig, generate};

#[test]
#[ignore = "needs ~/Documents/Familiar/faces/manifest.json"]
fn sessions_can_be_generated_from_the_real_manifest() {
    let home = std::env::var_os("HOME").expect("HOME is set");
    let path = PathBuf::from(home).join("Documents/Familiar/faces/manifest.json");
    let manifest = Manifest::load(&path).expect("the real manifest loads");
    let photos: usize = manifest.people.iter().map(|p| p.photos.len()).sum();
    println!("{} people, {photos} photos", manifest.people.len());

    let catalog = Catalog::new(manifest, &BTreeSet::new());
    for level in 1..=5 {
        for trials in [40, 80, 120, 200] {
            let config = SessionConfig {
                trials,
                level: Level::new(level),
                seed: 1,
            };
            let plan = generate(&catalog, &config).expect("a session is generated");
            assert_eq!(plan.trials.len(), trials);
        }
    }
}
