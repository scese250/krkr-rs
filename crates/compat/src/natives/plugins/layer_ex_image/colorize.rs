//! `Layer.colorize(hue, sat, blend)`: take a requested hue and saturation
//! for every pixel of the clip rectangle, keeping the lightness.
//!
//! The hue and the saturation are stored in a byte, so a value outside
//! 0..255 is taken modulo 256. The blend weight is clamped into 0..1: at 0
//! the pixel keeps its colour, at 1 the requested colour replaces it, and
//! in between the two are mixed.

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeObjThisMethodDef, Value};

use super::{for_each_pixel, integer_arg, real_arg, receiver, with_clip};
use crate::CompatCtx;
use crate::native_util::{callback_args, report_error, set_void};

/// Apply the member to the layer's image.
fn colorize(ctx: &CompatCtx, id: u32, hue: i64, sat: i64, blend: f64) -> Result<(), String> {
    // The hue and the saturation are stored in a byte, so a value outside
    // 0..255 is taken modulo 256.
    let hue = hue as u8;
    let sat = sat as u8;
    let blend = blend.clamp(0.0, 1.0);
    // The two weights add up to 256, so the mix divides by 256 and a
    // weight of 0 leaves the pixel exactly as it was.
    let a0 = (256.0 * blend) as i32;
    let a1 = 256 - a0;
    let full = blend > 0.999;
    with_clip(ctx, id, "colorize", |bitmap, region| {
        for_each_pixel(bitmap, region, "colorize", |pixel| {
            // Only the lightness of the pixel survives: the hue and the
            // saturation are the requested ones.
            let light = lightness(pixel[0], pixel[1], pixel[2]);
            let (red, green, blue) = hsl_to_rgb(hue, sat, light);
            if full {
                pixel[0] = red;
                pixel[1] = green;
                pixel[2] = blue;
            } else {
                pixel[0] = ((i32::from(red) * a0 + i32::from(pixel[0]) * a1) >> 8) as u8;
                pixel[1] = ((i32::from(green) * a0 + i32::from(pixel[1]) * a1) >> 8) as u8;
                pixel[2] = ((i32::from(blue) * a0 + i32::from(pixel[2]) * a1) >> 8) as u8;
            }
        })
    })
}

/// The lightness of a colour, in the byte scale the hue and the saturation
/// use.
///
/// The mean of the two extreme channels, rounded to the nearest step.
fn lightness(red: u8, green: u8, blue: u8) -> u8 {
    let (red, green, blue) = (u32::from(red), u32::from(green), u32::from(blue));
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    (((max + min) * 255 + 255) / 510) as u8
}

/// The colour of a hue, a saturation and a lightness.
///
/// The step is single precision and the result is truncated into a byte, so
/// the round trip through it moves a channel by up to one step.
fn hsl_to_rgb(hue: u8, saturation: u8, lightness: u8) -> (u8, u8, u8) {
    let hue = f32::from(hue) * 360.0 / 255.0;
    let saturation = f32::from(saturation) / 255.0;
    let lightness = f32::from(lightness) / 255.0;
    let m2 = if lightness <= 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let m1 = 2.0 * lightness - m2;
    if saturation == 0.0 {
        let value = (lightness * 255.0) as u8;
        (value, value, value)
    } else {
        (
            (hue_to_rgb(m1, m2, hue + 120.0) * 255.0) as u8,
            (hue_to_rgb(m1, m2, hue) * 255.0) as u8,
            (hue_to_rgb(m1, m2, hue - 120.0) * 255.0) as u8,
        )
    }
}

/// The colour of a hue between two bounds.
///
/// The hue is folded into 0..360 once, and the six sectors of the wheel
/// answer the mix of the two bounds.
fn hue_to_rgb(n1: f32, n2: f32, mut hue: f32) -> f32 {
    if hue > 360.0 {
        hue -= 360.0;
    } else if hue < 0.0 {
        hue += 360.0;
    }
    if hue < 60.0 {
        n1 + (n2 - n1) * hue / 60.0
    } else if hue < 180.0 {
        n2
    } else if hue < 240.0 {
        n1 + (n2 - n1) * (240.0 - hue) / 60.0
    } else {
        n1
    }
}

extern "C" fn layer_colorize(
    engine: *mut c_void,
    objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let (ctx, id) = match receiver(engine, objthis, "colorize") {
        Ok(pair) => pair,
        Err(error) => return report_error(out_error, &error),
    };
    let a = callback_args(argv, argc);
    if a.len() < 3 {
        return report_error(out_error, "Layer.colorize requires 3 arguments");
    }
    let hue = match integer_arg(&a[0], "colorize", "hue") {
        Ok(value) => value,
        Err(error) => return report_error(out_error, &error),
    };
    let sat = match integer_arg(&a[1], "colorize", "sat") {
        Ok(value) => value,
        Err(error) => return report_error(out_error, &error),
    };
    let blend = match real_arg(&a[2], "colorize", "blend") {
        Ok(value) => value,
        Err(error) => return report_error(out_error, &error),
    };
    if let Err(error) = colorize(&ctx, id, hue, sat, blend) {
        return report_error(out_error, &error);
    }
    set_void(out);
    0
}

/// The `Layer`-instance method definitions.
pub(super) fn layer_method_defs() -> Vec<NativeObjThisMethodDef> {
    vec![NativeObjThisMethodDef {
        name: "colorize",
        f: layer_colorize,
    }]
}
