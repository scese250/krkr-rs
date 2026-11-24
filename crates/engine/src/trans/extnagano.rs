//! `blurfade` and `rgbfade` transitions (`extNagano` provider family).
//!
//! * **`blurfade`** blurs the outgoing source progressively from `(0, 0)`
//!   toward `(blur1_x, blur1_y)` and un-blurs the incoming source from
//!   `(blur2_x, blur2_y)` toward `(0, 0)` at the eased progress `t`, then
//!   cross-fades the two images at `t`.
//! * **`rgbfade`** cross-fades each RGBA channel on its own delayed progress
//!   ramp across `[0, 1]`, so channels stagger according to their non-negative
//!   delays while converging cleanly to source 1 at `t = 0` and source 2 at
//!   `t = 1`.

use super::{PHASE_MAX, Trans, TransError, blend_channel, read_pixel, write_pixel};
use crate::layer_ops::{RectI, blur_bitmap_in_place, intersect};
use crate::scene::BitmapState;

/// Parameters for the `blurfade` transition.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlurFadeParams {
    /// Peak horizontal blur radius for source 1 (`blur1x` / `blur1`, default 0).
    pub blur1_x: i32,
    /// Peak vertical blur radius for source 1 (`blur1y` / `blur1`, default 0).
    pub blur1_y: i32,
    /// Peak horizontal blur radius for source 2 (`blur2x` / `blur2`, default 0).
    pub blur2_x: i32,
    /// Peak vertical blur radius for source 2 (`blur2y` / `blur2`, default 0).
    pub blur2_y: i32,
    /// Progress easing exponent (`exponent`, default 1.0; positive and finite).
    pub exponent: f64,
}

/// Parameters for the `rgbfade` transition.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RgbFadeParams {
    /// Delay for the red channel (`delayR` / `delayr`, default 0.0; non-negative).
    pub delay_r: f64,
    /// Delay for the green channel (`delayG` / `delayg`, default 0.0; non-negative).
    pub delay_g: f64,
    /// Delay for the blue channel (`delayB` / `delayb`, default 0.0; non-negative).
    pub delay_b: f64,
    /// Delay for the alpha channel (`delayA` / `delaya`, default 0.0; non-negative).
    pub delay_a: f64,
}

/// Render one `blurfade` frame into `dst`.
///
/// Endpoints (`phase <= 0` and `phase >= phase_max`) are already short-circuited
/// by [`Trans::apply`].
pub(crate) fn apply_blur_fade(
    trans: &Trans,
    dst: &mut BitmapState,
    src1: &BitmapState,
    src2: &BitmapState,
    region: RectI,
    params: BlurFadeParams,
) -> Result<(), TransError> {
    if intersect(dst, region).is_none() {
        return Ok(());
    }

    let phase_max = trans.phase_max().max(1);
    let raw_t = (f64::from(trans.phase()) / f64::from(phase_max)).clamp(0.0, 1.0);
    let t = if params.exponent != 1.0 {
        raw_t.powf(params.exponent).clamp(0.0, 1.0)
    } else {
        raw_t
    };

    let r1x = (f64::from(params.blur1_x) * t).round().max(0.0) as u32;
    let r1y = (f64::from(params.blur1_y) * t).round().max(0.0) as u32;
    let inv_t = 1.0 - t;
    let r2x = (f64::from(params.blur2_x) * inv_t).round().max(0.0) as u32;
    let r2y = (f64::from(params.blur2_y) * inv_t).round().max(0.0) as u32;

    let blurred1;
    let effective_src1 = if r1x > 0 || r1y > 0 {
        blurred1 = blurred_copy(src1, r1x, r1y);
        &blurred1
    } else {
        src1
    };

    let blurred2;
    let effective_src2 = if r2x > 0 || r2y > 0 {
        blurred2 = blurred_copy(src2, r2x, r2y);
        &blurred2
    } else {
        src2
    };

    let blend_phase = (t * f64::from(PHASE_MAX))
        .round()
        .clamp(0.0, f64::from(PHASE_MAX)) as i32;
    trans.blend_cross_fade(blend_phase, dst, effective_src1, effective_src2, region);
    Ok(())
}

/// Render one `rgbfade` frame into `dst`.
///
/// Endpoints (`phase <= 0` and `phase >= phase_max`) are already short-circuited
/// by [`Trans::apply`].
pub(crate) fn apply_rgb_fade(
    trans: &Trans,
    dst: &mut BitmapState,
    src1: &BitmapState,
    src2: &BitmapState,
    region: RectI,
    params: RgbFadeParams,
) -> Result<(), TransError> {
    let Some((x0, y0, x1, y1)) = intersect(dst, region) else {
        return Ok(());
    };

    let phase_max = trans.phase_max().max(1);
    let t = (f64::from(trans.phase()) / f64::from(phase_max)).clamp(0.0, 1.0);
    let max_delay = params
        .delay_r
        .max(params.delay_g)
        .max(params.delay_b)
        .max(params.delay_a);

    let opa_r = channel_weight(t, params.delay_r, max_delay);
    let opa_g = channel_weight(t, params.delay_g, max_delay);
    let opa_b = channel_weight(t, params.delay_b, max_delay);
    let opa_a = channel_weight(t, params.delay_a, max_delay);

    for y in y0..y1 {
        for x in x0..x1 {
            let s1 = read_pixel(src1, x, y);
            let s2 = read_pixel(src2, x, y);
            write_pixel(
                dst,
                x,
                y,
                [
                    blend_channel(s1[0], s2[0], opa_r),
                    blend_channel(s1[1], s2[1], opa_g),
                    blend_channel(s1[2], s2[2], opa_b),
                    blend_channel(s1[3], s2[3], opa_a),
                ],
            );
        }
    }

    dst.mark_dirty();
    Ok(())
}

/// Clone `src` and apply an in-place box blur with radius `(rx, ry)`.
fn blurred_copy(src: &BitmapState, rx: u32, ry: u32) -> BitmapState {
    let mut copy = src.clone();
    blur_bitmap_in_place(&mut copy, rx, ry);
    copy
}

/// Convert normalized transition progress `t` and a channel's non-negative
/// `delay` into a `0..=256` blend weight.
///
/// With `scale = 1.0 + max_delay`, each channel ramps linearly across a
/// window of width `1.0 / scale` starting at `delay / scale`:
/// * at `t = 0.0`, every channel is `0`;
/// * at `t = 1.0`, every channel is `256` (exact source 2);
/// * when all delays are `0.0`, every channel follows `t` identically.
fn channel_weight(t: f64, delay: f64, max_delay: f64) -> i32 {
    let scale = 1.0 + max_delay;
    let local_t = if scale.is_finite() {
        (t * scale - delay).clamp(0.0, 1.0)
    } else if delay < max_delay {
        1.0
    } else {
        t
    };
    (local_t * 256.0).round().clamp(0.0, 256.0) as i32
}

#[cfg(test)]
mod tests {
    use super::super::{DestAlpha, Trans, TransError, TransKind};
    use crate::scene::BitmapState;

    fn solid_bitmap(width: u32, height: u32, rgba: [u8; 4]) -> BitmapState {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            pixels.extend_from_slice(&rgba);
        }
        BitmapState::new(width, height, pixels)
    }

    fn impulse_bitmap(width: u32, height: u32, bg: [u8; 4], fg: [u8; 4]) -> BitmapState {
        let mut bmp = solid_bitmap(width, height, bg);
        let cx = width / 2;
        let cy = height / 2;
        if let Some(i) = bmp.pixel_offset(cx, cy) {
            bmp.rgba[i..i + 4].copy_from_slice(&fg);
        }
        bmp
    }

    #[test]
    fn trans_kind_names_match_script_providers() {
        let blur = TransKind::BlurFade {
            blur1_x: 0,
            blur1_y: 0,
            blur2_x: 0,
            blur2_y: 0,
            exponent: 1.0,
        };
        let rgb = TransKind::RgbFade {
            delay_r: 0.0,
            delay_g: 0.0,
            delay_b: 0.0,
            delay_a: 0.0,
        };
        assert_eq!(blur.name(), "blurfade");
        assert_eq!(rgb.name(), "rgbfade");
    }

    #[test]
    fn blurfade_endpoints_copy_sources_verbatim() {
        let src1 = impulse_bitmap(8, 8, [10, 20, 30, 255], [250, 240, 230, 255]);
        let src2 = impulse_bitmap(8, 8, [200, 150, 100, 255], [5, 15, 25, 255]);
        let region = Trans::full_region(&src1);

        let mut trans = Trans::new(
            TransKind::BlurFade {
                blur1_x: 4,
                blur1_y: 4,
                blur2_x: 4,
                blur2_y: 4,
                exponent: 2.0,
            },
            100.0,
        )
        .expect("valid blurfade");

        let mut dst = solid_bitmap(8, 8, [0, 0, 0, 0]);
        trans
            .apply(&mut dst, &src1, &src2, region)
            .expect("phase 0");
        assert_eq!(dst.rgba, src1.rgba);

        trans.complete();
        trans
            .apply(&mut dst, &src1, &src2, region)
            .expect("phase max");
        assert_eq!(dst.rgba, src2.rgba);
    }

    #[test]
    fn blurfade_zero_blur_matches_crossfade() {
        let src1 = impulse_bitmap(8, 8, [20, 40, 60, 255], [240, 200, 160, 255]);
        let src2 = impulse_bitmap(8, 8, [180, 120, 80, 255], [10, 30, 50, 255]);
        let region = Trans::full_region(&src1);

        let mut blur = Trans::new(
            TransKind::BlurFade {
                blur1_x: 0,
                blur1_y: 0,
                blur2_x: 0,
                blur2_y: 0,
                exponent: 1.0,
            },
            100.0,
        )
        .expect("valid blurfade");
        blur.dest_alpha = DestAlpha::Opaque;
        blur.step(50.0);

        let mut cross = Trans::cross_fade(100.0).expect("valid crossfade");
        cross.dest_alpha = DestAlpha::Opaque;
        cross.step(50.0);

        let mut dst_blur = solid_bitmap(8, 8, [0, 0, 0, 0]);
        let mut dst_cross = solid_bitmap(8, 8, [0, 0, 0, 0]);
        blur.apply(&mut dst_blur, &src1, &src2, region)
            .expect("apply blurfade");
        cross
            .apply(&mut dst_cross, &src1, &src2, region)
            .expect("apply crossfade");

        assert_eq!(dst_blur.rgba, dst_cross.rgba);
    }

    #[test]
    fn blurfade_mid_transition_blurs_sources_and_blends() {
        let src1 = impulse_bitmap(9, 9, [0, 0, 0, 255], [255, 0, 0, 255]);
        let src2 = impulse_bitmap(9, 9, [0, 0, 0, 255], [0, 0, 255, 255]);
        let region = Trans::full_region(&src1);

        let mut blur = Trans::new(
            TransKind::BlurFade {
                blur1_x: 4,
                blur1_y: 4,
                blur2_x: 4,
                blur2_y: 4,
                exponent: 1.0,
            },
            100.0,
        )
        .expect("valid blurfade");
        blur.dest_alpha = DestAlpha::Opaque;
        blur.step(50.0);

        let mut dst = solid_bitmap(9, 9, [0, 0, 0, 0]);
        blur.apply(&mut dst, &src1, &src2, region)
            .expect("apply blurfade");

        // The impulse at (4, 4) should spread into its immediate neighbour (3, 4).
        let neighbor = super::read_pixel(&dst, 3, 4);
        assert!(
            neighbor[0] > 0 && neighbor[2] > 0,
            "expected blurred energy in neighbor pixel, got {neighbor:?}"
        );
    }

    #[test]
    fn blurfade_exponent_eases_progress() {
        let src1 = solid_bitmap(4, 4, [0, 0, 0, 255]);
        let src2 = solid_bitmap(4, 4, [240, 240, 240, 255]);
        let region = Trans::full_region(&src1);

        let render_at_half = |exponent| {
            let mut trans = Trans::new(
                TransKind::BlurFade {
                    blur1_x: 0,
                    blur1_y: 0,
                    blur2_x: 0,
                    blur2_y: 0,
                    exponent,
                },
                100.0,
            )
            .expect("valid blurfade");
            trans.dest_alpha = DestAlpha::Opaque;
            trans.step(50.0);
            let mut dst = solid_bitmap(4, 4, [0, 0, 0, 0]);
            trans.apply(&mut dst, &src1, &src2, region).expect("apply");
            dst.rgba[0]
        };

        let linear = render_at_half(1.0);
        let ease_in = render_at_half(2.0);
        let ease_out = render_at_half(0.5);

        assert!(
            ease_in < linear && linear < ease_out,
            "expected ease_in ({ease_in}) < linear ({linear}) < ease_out ({ease_out})"
        );
    }

    #[test]
    fn blurfade_rejects_invalid_options() {
        let cases = [
            TransKind::BlurFade {
                blur1_x: -1,
                blur1_y: 0,
                blur2_x: 0,
                blur2_y: 0,
                exponent: 1.0,
            },
            TransKind::BlurFade {
                blur1_x: 0,
                blur1_y: -1,
                blur2_x: 0,
                blur2_y: 0,
                exponent: 1.0,
            },
            TransKind::BlurFade {
                blur1_x: 0,
                blur1_y: 0,
                blur2_x: -1,
                blur2_y: 0,
                exponent: 1.0,
            },
            TransKind::BlurFade {
                blur1_x: 0,
                blur1_y: 0,
                blur2_x: 0,
                blur2_y: -1,
                exponent: 1.0,
            },
            TransKind::BlurFade {
                blur1_x: 0,
                blur1_y: 0,
                blur2_x: 0,
                blur2_y: 0,
                exponent: 0.0,
            },
            TransKind::BlurFade {
                blur1_x: 0,
                blur1_y: 0,
                blur2_x: 0,
                blur2_y: 0,
                exponent: -2.0,
            },
            TransKind::BlurFade {
                blur1_x: 0,
                blur1_y: 0,
                blur2_x: 0,
                blur2_y: 0,
                exponent: f64::NAN,
            },
            TransKind::BlurFade {
                blur1_x: 0,
                blur1_y: 0,
                blur2_x: 0,
                blur2_y: 0,
                exponent: f64::INFINITY,
            },
        ];

        for kind in cases {
            assert!(
                matches!(
                    Trans::new(kind, 100.0),
                    Err(TransError::InvalidOption { .. })
                ),
                "expected InvalidOption for {kind:?}"
            );
        }
    }

    #[test]
    fn rgbfade_endpoints_copy_sources_verbatim() {
        let src1 = solid_bitmap(4, 4, [10, 20, 30, 40]);
        let src2 = solid_bitmap(4, 4, [210, 220, 230, 240]);
        let region = Trans::full_region(&src1);

        let mut trans = Trans::new(
            TransKind::RgbFade {
                delay_r: 0.0,
                delay_g: 0.5,
                delay_b: 1.0,
                delay_a: 0.25,
            },
            100.0,
        )
        .expect("valid rgbfade");

        let mut dst = solid_bitmap(4, 4, [0, 0, 0, 0]);
        trans
            .apply(&mut dst, &src1, &src2, region)
            .expect("phase 0");
        assert_eq!(dst.rgba, src1.rgba);

        trans.complete();
        trans
            .apply(&mut dst, &src1, &src2, region)
            .expect("phase max");
        assert_eq!(dst.rgba, src2.rgba);
    }

    #[test]
    fn rgbfade_zero_delays_blends_all_channels_equally() {
        let src1 = solid_bitmap(4, 4, [0, 0, 0, 0]);
        let src2 = solid_bitmap(4, 4, [200, 200, 200, 200]);
        let region = Trans::full_region(&src1);

        let mut trans = Trans::new(
            TransKind::RgbFade {
                delay_r: 0.0,
                delay_g: 0.0,
                delay_b: 0.0,
                delay_a: 0.0,
            },
            100.0,
        )
        .expect("valid rgbfade");
        trans.step(50.0);

        let mut dst = solid_bitmap(4, 4, [0, 0, 0, 0]);
        trans.apply(&mut dst, &src1, &src2, region).expect("apply");

        let px = super::read_pixel(&dst, 0, 0);
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);
        assert_eq!(px[2], px[3]);
        assert!((95..=105).contains(&px[0]), "expected ~100, got {}", px[0]);
    }

    #[test]
    fn rgbfade_staggers_channels_by_delay() {
        let src1 = solid_bitmap(4, 4, [0, 0, 0, 0]);
        let src2 = solid_bitmap(4, 4, [255, 255, 255, 255]);
        let region = Trans::full_region(&src1);

        let mut trans = Trans::new(
            TransKind::RgbFade {
                delay_r: 0.0,
                delay_g: 0.5,
                delay_b: 1.0,
                delay_a: 0.0,
            },
            100.0,
        )
        .expect("valid rgbfade");
        trans.step(50.0);

        let mut dst = solid_bitmap(4, 4, [0, 0, 0, 0]);
        trans.apply(&mut dst, &src1, &src2, region).expect("apply");

        let px = super::read_pixel(&dst, 0, 0);
        assert!(
            px[0] > px[1] && px[1] > px[2],
            "expected R > G > B at midpoint, got {px:?}"
        );
        assert_eq!(px[0], px[3], "R and A share delay 0.0");
    }

    #[test]
    fn rgbfade_rejects_invalid_options() {
        let cases = [
            TransKind::RgbFade {
                delay_r: -0.1,
                delay_g: 0.0,
                delay_b: 0.0,
                delay_a: 0.0,
            },
            TransKind::RgbFade {
                delay_r: 0.0,
                delay_g: -1.0,
                delay_b: 0.0,
                delay_a: 0.0,
            },
            TransKind::RgbFade {
                delay_r: 0.0,
                delay_g: 0.0,
                delay_b: f64::NAN,
                delay_a: 0.0,
            },
            TransKind::RgbFade {
                delay_r: 0.0,
                delay_g: 0.0,
                delay_b: 0.0,
                delay_a: f64::INFINITY,
            },
        ];

        for kind in cases {
            assert!(
                matches!(
                    Trans::new(kind, 100.0),
                    Err(TransError::InvalidOption { .. })
                ),
                "expected InvalidOption for {kind:?}"
            );
        }
    }
}
