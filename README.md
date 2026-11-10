# Familiar

A face recognition trainer for macOS, written in Rust.

Faces go by quickly. If a person has already appeared in this session — even with a different look (glasses, beard, hairstyle, make-up, angle) — press the button at once. A false press or a missed repeat is punished with a flash and a harsh sound. The pace picks up for as long as you keep up.

## How a session works

- A session is 40, 80, 120 or 200 faces. About 35% of them are repeats of a person shown earlier in the same session.
- Each face is shown for the current exposure, then a fixation cross follows. You can answer until the cross disappears.
- Respond with **Space**, **J**, **Enter**, or a click on the photo or the button. **Esc** quits the session without saving it. If the window loses focus the session pauses.
- Every 10 faces the exposure adapts: it shortens when you are accurate and lengthens when you are not, within 250–3000 ms. The exposure you finish on is where the next session starts.

### Disguise levels

| Level | Repeats show |
|---|---|
| 1. Same look | the very same photo |
| 2. Similar look | another photo with nearly the same look |
| 3. Different look | another photo with a clearly different look |
| 4. Strong change | the most different photo, plus mirroring, rotation, zoom and brightness changes |
| 5. Full disguise | as level 4 with harsher distortions (grayscale, darkening), plus lookalikes: new people chosen to resemble those already shown |

With automatic level changes on, a session at 85% accuracy or better moves you up a level and one below 60% moves you down.

### After a session

The results screen shows hits, misses, false alarms, correct rejections, balanced accuracy, d′ and the mean reaction time. It also shows the faces you got wrong: if you recognise a celebrity among them, exclude them with **I know them**, since a familiar face makes the task too easy. The history screen plots your exposure over sessions, coloured by accuracy.

## Data

The photos live outside the repository, by default in `~/Documents/Familiar`:

```
~/Documents/Familiar/
  faces/
    manifest.json        attributes of every photo (glasses, beard, hair colour...)
    <person id>/
      000123.jpg
      ...
  settings.json          settings, level, exposure, excluded people
  history.json           finished sessions
```

Prepare the face set with:

```sh
python3 scripts/prepare_celeba.py --out ~/Documents/Familiar
```

The script downloads CelebA (about 1.4 GB) and sorts a selection of photos by person. If the app does not find the set it explains this and lets you pick the folder; the folder can also be changed in the settings.

CelebA may be used for non-commercial research only and must not be redistributed. That is why photos, settings and history are excluded from git in `.gitignore`.

If `~/Documents` is synced with iCloud, macOS may move photos to the cloud. The app skips people whose photos are not on disk and tells you so; to get them back, download the folder in Finder.

## Build and run

You need [rustup](https://rustup.rs) (or any Rust 1.85 or newer) and the Xcode Command Line Tools (`xcode-select --install`).

```sh
cargo run -p familiar-mac --release
```

To build `Familiar.app`:

```sh
cargo install cargo-bundle
cargo bundle --release --package familiar-mac
```

Run it from the repository root: the icon path is resolved against the current directory. The app is written to `target/release/bundle/osx/Familiar.app`; drag it to Applications. It is not signed with an Apple certificate, so the first time right-click it and choose **Open**. macOS will also ask for access to the Documents folder.

If `cargo install cargo-bundle` fails on an older Rust, install a version that still supports it, for example `cargo install cargo-bundle --version 0.8.0 --locked`.

## Repository layout

| Path | What it is |
|---|---|
| `crates/familiar-core` | Training logic with no UI: session planning, the session state machine, scoring, levels, storage. Platform-independent, to be reused on iOS. |
| `crates/familiar-mac` | The macOS app (`eframe`/`egui`). |
| `scripts/prepare_celeba.py` | Downloads CelebA and prepares the face set. |
| `scripts/make_icon.py` | Draws the app icon into `crates/familiar-mac/icons`. |

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Tests never use real photos: they run on a small synthetic manifest. A smoke test against the real face set is available with `cargo test -p familiar-core -- --ignored`.
