//! Applying a [`Distortion`] to a photo.
//!
//! Grayscale and brightness are baked into the pixels when the texture is
//! prepared (a tint cannot brighten). Mirror, zoom and rotation are applied
//! when drawing, through texture coordinates and a rotated quad clipped to the
//! photo frame.

use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use familiar_core::Distortion;

/// Width over height of the photos (CelebA aligned faces are 178 x 218).
pub const PHOTO_ASPECT: f32 = 178.0 / 218.0;

/// Bakes grayscale and brightness into RGB pixels.
pub fn prepare_pixels(rgb: &mut [u8], distortion: &Distortion) {
    let brightness = distortion.brightness;
    if !distortion.grayscale && (brightness - 1.0).abs() < f32::EPSILON {
        return;
    }
    let scale = |value: f32| (value * brightness).round().clamp(0.0, 255.0) as u8;
    for pixel in rgb.chunks_exact_mut(3) {
        let [r, g, b] = [
            f32::from(pixel[0]),
            f32::from(pixel[1]),
            f32::from(pixel[2]),
        ];
        if distortion.grayscale {
            let luma = 0.299 * r + 0.587 * g + 0.114 * b;
            pixel.fill(scale(luma));
        } else {
            pixel.copy_from_slice(&[scale(r), scale(g), scale(b)]);
        }
    }
}

/// The part of the texture that is shown: a crop of `1 / zoom` of each side,
/// placed by the offsets, and flipped horizontally when mirrored.
pub fn crop_uv(distortion: &Distortion) -> Rect {
    let zoom = distortion.zoom.max(1.0);
    let half = 0.5 / zoom;
    let slack = 0.5 - half;
    let center_u = 0.5 + distortion.offset_x.clamp(-1.0, 1.0) * slack;
    let center_v = 0.5 + distortion.offset_y.clamp(-1.0, 1.0) * slack;
    let (left, right) = (center_u - half, center_u + half);
    let (u_min, u_max) = if distortion.mirror {
        (right, left)
    } else {
        (left, right)
    };
    Rect::from_min_max(
        Pos2::new(u_min, center_v - half),
        Pos2::new(u_max, center_v + half),
    )
}

/// How much a `width` x `height` quad rotated by `degrees` must grow to still
/// cover its unrotated outline.
pub fn cover_scale(degrees: f32, width: f32, height: f32) -> f32 {
    let (sin, cos) = degrees.to_radians().abs().sin_cos();
    cos + sin * (width / height).max(height / width)
}

/// Draws the photo into `frame`, which it always fills completely.
pub fn paint_photo(
    painter: &egui::Painter,
    frame: Rect,
    texture: egui::TextureId,
    distortion: &Distortion,
) {
    let uv = crop_uv(distortion);
    let scale = cover_scale(distortion.rotation_deg, frame.width(), frame.height());
    let half = frame.size() * 0.5 * scale;
    let (sin, cos) = distortion.rotation_deg.to_radians().sin_cos();
    let corner = |x: f32, y: f32| {
        let offset = Vec2::new(x * half.x, y * half.y);
        frame.center()
            + Vec2::new(
                offset.x * cos - offset.y * sin,
                offset.x * sin + offset.y * cos,
            )
    };

    let mut mesh = egui::Mesh::with_texture(texture);
    for (x, y, u, v) in [
        (-1.0, -1.0, uv.min.x, uv.min.y),
        (1.0, -1.0, uv.max.x, uv.min.y),
        (-1.0, 1.0, uv.min.x, uv.max.y),
        (1.0, 1.0, uv.max.x, uv.max.y),
    ] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: corner(x, y),
            uv: Pos2::new(u, v),
            color: Color32::WHITE,
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(2, 1, 3);
    painter.with_clip_rect(frame).add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn no_distortion_shows_the_whole_photo() {
        let uv = crop_uv(&Distortion::NONE);
        assert_eq!(uv, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
        assert!(close(cover_scale(0.0, 178.0, 218.0), 1.0));
    }

    #[test]
    fn zoom_crops_and_offsets_stay_inside_the_photo() {
        for (offset_x, offset_y) in [(-1.0, -1.0), (1.0, 1.0), (0.3, -0.7), (5.0, -5.0)] {
            let uv = crop_uv(&Distortion {
                zoom: 1.5,
                offset_x,
                offset_y,
                ..Distortion::NONE
            });
            assert!(close(uv.width(), 1.0 / 1.5) && close(uv.height(), 1.0 / 1.5));
            assert!(uv.min.x >= -1e-6 && uv.min.y >= -1e-6, "{uv:?}");
            assert!(uv.max.x <= 1.0 + 1e-6 && uv.max.y <= 1.0 + 1e-6, "{uv:?}");
        }
        let left = crop_uv(&Distortion {
            zoom: 2.0,
            offset_x: -1.0,
            ..Distortion::NONE
        });
        assert!(close(left.min.x, 0.0) && close(left.max.x, 0.5));
    }

    #[test]
    fn mirror_swaps_the_horizontal_texture_coordinates() {
        let uv = crop_uv(&Distortion {
            mirror: true,
            ..Distortion::NONE
        });
        assert!(close(uv.min.x, 1.0) && close(uv.max.x, 0.0));
        assert!(close(uv.min.y, 0.0) && close(uv.max.y, 1.0));
    }

    #[test]
    fn a_rotated_quad_is_enlarged_enough_to_cover_the_frame() {
        let (w, h) = (178.0f32, 218.0f32);
        for degrees in [-8.0f32, -3.0, 2.0, 8.0] {
            let scale = cover_scale(degrees, w, h);
            assert!(scale > 1.0 && scale < 1.2, "{scale}");
            // Every frame corner, rotated back into the quad's own axes, must
            // fall inside the enlarged quad.
            let (sin, cos) = (-degrees).to_radians().sin_cos();
            for (x, y) in [(w / 2.0, h / 2.0), (-w / 2.0, h / 2.0)] {
                let (qx, qy) = (x * cos - y * sin, x * sin + y * cos);
                assert!(qx.abs() <= w / 2.0 * scale + 1e-3, "{degrees}: x {qx}");
                assert!(qy.abs() <= h / 2.0 * scale + 1e-3, "{degrees}: y {qy}");
            }
        }
    }

    #[test]
    fn pixels_are_untouched_without_grayscale_or_brightness() {
        let mut pixels = [10, 20, 30, 200, 100, 0];
        prepare_pixels(
            &mut pixels,
            &Distortion {
                zoom: 1.4,
                mirror: true,
                ..Distortion::NONE
            },
        );
        assert_eq!(pixels, [10, 20, 30, 200, 100, 0]);
    }

    #[test]
    fn brightness_scales_and_saturates() {
        let mut pixels = [100, 200, 250];
        prepare_pixels(
            &mut pixels,
            &Distortion {
                brightness: 1.2,
                ..Distortion::NONE
            },
        );
        assert_eq!(pixels, [120, 240, 255]);
        let mut pixels = [100, 200, 250];
        prepare_pixels(
            &mut pixels,
            &Distortion {
                brightness: 0.5,
                ..Distortion::NONE
            },
        );
        assert_eq!(pixels, [50, 100, 125]);
    }

    #[test]
    fn grayscale_makes_all_channels_equal() {
        let mut pixels = [255, 0, 0, 0, 255, 0, 80, 80, 80];
        prepare_pixels(
            &mut pixels,
            &Distortion {
                grayscale: true,
                ..Distortion::NONE
            },
        );
        assert_eq!(pixels, [76, 76, 76, 150, 150, 150, 80, 80, 80]);
    }
}
