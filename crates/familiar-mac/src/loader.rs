//! Decoding photos off the UI thread.
//!
//! The session view asks for the photos of the next few trials ahead of time;
//! a worker thread reads and decodes them, and the view turns the results into
//! textures before the faces are due. Showing a face then never waits for disk.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use eframe::egui::ColorImage;
use familiar_core::Distortion;
use image::imageops::FilterType;

use crate::render::prepare_pixels;

/// Photos are enlarged this many times with a good filter before they become
/// textures. The source is only 178 x 218 pixels; leaving all of the
/// enlargement to the GPU's bilinear filter makes faces look smeared.
const UPSCALE: u32 = 2;
/// Blur radius of the unsharp mask applied after enlarging.
const SHARPEN_SIGMA: f32 = 1.2;
/// Differences below this are left alone, so JPEG noise is not amplified.
const SHARPEN_THRESHOLD: i32 = 2;

struct Request {
    index: usize,
    path: PathBuf,
    distortion: Distortion,
}

/// A decoded photo for trial `index`, or why it could not be decoded.
pub struct Loaded {
    pub index: usize,
    pub image: Result<ColorImage, String>,
}

/// Handle to the decoding thread. Dropping it stops the thread.
pub struct Loader {
    requests: Sender<Request>,
    results: Receiver<Loaded>,
}

impl Loader {
    pub fn new() -> Self {
        let (requests, incoming) = mpsc::channel::<Request>();
        let (outgoing, results) = mpsc::channel::<Loaded>();
        thread::spawn(move || {
            for request in incoming {
                let image = decode(&request.path, &request.distortion);
                if outgoing
                    .send(Loaded {
                        index: request.index,
                        image,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self { requests, results }
    }

    /// Queues the photo at `path` for trial `index`.
    pub fn request(&self, index: usize, path: PathBuf, distortion: Distortion) {
        // The worker only stops when this handle is dropped, so a failed send
        // cannot happen while it is in use.
        let _ = self.requests.send(Request {
            index,
            path,
            distortion,
        });
    }

    /// Photos decoded since the last call.
    pub fn poll(&self) -> impl Iterator<Item = Loaded> + '_ {
        self.results.try_iter()
    }
}

/// Reads and decodes one photo, with grayscale and brightness baked in,
/// enlarged and lightly sharpened.
pub fn decode(path: &Path, distortion: &Distortion) -> Result<ColorImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decoded =
        image::load_from_memory(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rgb = decoded.into_rgb8();
    prepare_pixels(&mut rgb, distortion);
    Ok(enlarge(&rgb))
}

/// Enlarges a photo with a Lanczos filter and sharpens the result a little.
fn enlarge(photo: &image::RgbImage) -> ColorImage {
    let (width, height) = (photo.width() * UPSCALE, photo.height() * UPSCALE);
    let enlarged = image::imageops::resize(photo, width, height, FilterType::Lanczos3);
    let sharpened = image::imageops::unsharpen(&enlarged, SHARPEN_SIGMA, SHARPEN_THRESHOLD);
    ColorImage::from_rgb([width as usize, height as usize], &sharpened)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photos_are_enlarged_twice_and_keep_flat_areas_flat() {
        let photo = image::RgbImage::from_pixel(178, 218, image::Rgb([120, 90, 60]));
        let texture = enlarge(&photo);
        assert_eq!(texture.size, [356, 436]);
        let expected = eframe::egui::Color32::from_rgb(120, 90, 60);
        assert!(texture.pixels.iter().all(|pixel| *pixel == expected));
    }

    #[test]
    fn edges_come_out_sharper_than_plain_enlarging() {
        // A hard vertical edge: black on the left, white on the right.
        let photo = image::RgbImage::from_fn(40, 8, |x, _| {
            if x < 20 {
                image::Rgb([0, 0, 0])
            } else {
                image::Rgb([255, 255, 255])
            }
        });
        let texture = enlarge(&photo);
        let row = &texture.pixels[3 * texture.size[0]..4 * texture.size[0]];
        // Few pixels are in between black and white: the edge stays narrow.
        let grey = row.iter().filter(|p| (30..226).contains(&p.r())).count();
        assert!(grey <= 3, "{grey} grey pixels across the edge");
    }

    #[test]
    fn unreadable_files_are_reported_with_their_path() {
        let error = decode(Path::new("/nonexistent/1.jpg"), &Distortion::NONE).unwrap_err();
        assert!(error.contains("/nonexistent/1.jpg"), "{error}");
    }
}
