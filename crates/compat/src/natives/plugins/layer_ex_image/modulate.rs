//! `Layer.modulate(hue, saturation, luminance)`: rotate the hue of every
//! pixel of the clip rectangle and push its saturation and its lightness.
//!
//! The three parameters are a rotation of the hue in degrees, a change of
//! the saturation in percent and a change of the lightness in percent. None
//! of them is clamped: a saturation or a lightness outside the documented
//! range pushes the colour past its end, and the channel values that come
//! out of it wrap into a byte.
//!
//! The colour model is a double-precision HSL whose hue is a fraction of a
//! turn, so this member is not another spelling of `colorize`.

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeObjThisMethodDef, Value};

use super::{for_each_pixel, integer_arg, receiver, with_clip};
use crate::CompatCtx;
use crate::native_util::{callback_args, report_error, set_void};

/// Apply the member to the layer's image.
fn modulate(
    ctx: &CompatCtx,
    id: u32,
    hue: i64,
    saturation: i64,
    luminance: i64,
) -> Result<(), String> {
    // The three parameters are fractions: a turn of the hue, and a share of
    // the saturation and of the lightness.
    let hue = hue as f64 / 360.0;
    let saturation = saturation as f64 / 100.0;
    let luminance = luminance as f64 / 100.0;
    with_clip(ctx, id, "modulate", |bitmap, region| {
        for_each_pixel(bitmap, region, "modulate", |pixel| {
            let (red, green, blue) =
                modulate_pixel(pixel[0], pixel[1], pixel[2], hue, saturation, luminance);
            pixel[0] = red;
            pixel[1] = green;
            pixel[2] = blue;
        })
    })
}

/// The colour of a pixel after the rotation and the two pushes.
fn modulate_pixel(
    red: u8,
    green: u8,
    blue: u8,
    hue: f64,
    saturation: f64,
    luminance: f64,
) -> (u8, u8, u8) {
    let red = f64::from(red) / 255.0;
    let green = f64::from(green) / 255.0;
    let blue = f64::from(blue) / 255.0;
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    let sum = max + min;
    let mut lightness = sum / 2.0;
    let (mut hue_fraction, mut saturation_value) = if delta == 0.0 {
        // A grey pixel has no hue and no saturation.
        (0.0, 0.0)
    } else {
        let saturation_value = if lightness < 0.5 {
            delta / sum
        } else {
            delta / (2.0 - sum)
        };
        let hue_fraction = (if red == max {
            (green - blue) / delta
        } else if green == max {
            2.0 + (blue - red) / delta
        } else {
            4.0 + (red - green) / delta
        }) / 6.0;
        (hue_fraction, saturation_value)
    };
    // The hue is rotated and folded back into a turn.
    hue_fraction = fold_turn(hue_fraction + hue);
    // The saturation is pushed towards the end of its range: a positive
    // share moves it towards one, a negative share towards zero.
    saturation_value = if saturation > 0.0 {
        saturation_value + (1.0 - saturation_value) * saturation
    } else {
        saturation_value + saturation_value * saturation
    };
    // The lightness is pushed the same way.
    lightness = if luminance > 0.0 {
        lightness + (1.0 - lightness) * luminance
    } else {
        lightness + lightness * luminance
    };
    if saturation_value == 0.0 {
        let value = to_byte(lightness * 255.0);
        (value, value, value)
    } else {
        let m2 = if lightness <= 0.5 {
            lightness * (1.0 + saturation_value)
        } else {
            lightness + saturation_value - lightness * saturation_value
        };
        let m1 = 2.0 * lightness - m2;
        (
            to_byte(hue_to_rgb(m1, m2, hue_fraction + 1.0 / 3.0) * 255.0),
            to_byte(hue_to_rgb(m1, m2, hue_fraction) * 255.0),
            to_byte(hue_to_rgb(m1, m2, hue_fraction - 1.0 / 3.0) * 255.0),
        )
    }
}

/// Fold a value into a turn.
///
/// The value is folded one whole turn at a time, so a rotation of many
/// turns lands where the rotation of the remainder lands. The fold is
/// exact, so the fractional part is the value the repeated addition
/// reaches; a whole number of turns folds onto the end of the turn.
fn fold_turn(value: f64) -> f64 {
    let folded = value - value.floor();
    if folded == 0.0 && value > 0.0 {
        1.0
    } else {
        folded
    }
}

/// The colour of a hue between two bounds.
///
/// The hue is a fraction of a turn and is folded into 0..1 once.
fn hue_to_rgb(n1: f64, n2: f64, mut hue: f64) -> f64 {
    if hue < 0.0 {
        hue += 1.0;
    }
    if hue > 1.0 {
        hue -= 1.0;
    }
    if hue < 1.0 / 6.0 {
        n1 + (n2 - n1) * hue * 6.0
    } else if hue < 0.5 {
        n2
    } else if hue < 2.0 / 3.0 {
        n1 + (n2 - n1) * (2.0 / 3.0 - hue) * 6.0
    } else {
        n1
    }
}

/// A value as the byte the plugin stores it in.
///
/// The value is truncated toward zero and keeps its low eight bits, so a
/// value outside 0..255 wraps.
fn to_byte(value: f64) -> u8 {
    value.trunc().rem_euclid(256.0) as u8
}

extern "C" fn layer_modulate(
    engine: *mut c_void,
    objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let (ctx, id) = match receiver(engine, objthis, "modulate") {
        Ok(pair) => pair,
        Err(error) => return report_error(out_error, &error),
    };
    let a = callback_args(argv, argc);
    if a.len() < 3 {
        return report_error(out_error, "Layer.modulate requires 3 arguments");
    }
    let hue = match integer_arg(&a[0], "modulate", "hue") {
        Ok(value) => value,
        Err(error) => return report_error(out_error, &error),
    };
    let saturation = match integer_arg(&a[1], "modulate", "saturation") {
        Ok(value) => value,
        Err(error) => return report_error(out_error, &error),
    };
    let luminance = match integer_arg(&a[2], "modulate", "luminance") {
        Ok(value) => value,
        Err(error) => return report_error(out_error, &error),
    };
    if let Err(error) = modulate(&ctx, id, hue, saturation, luminance) {
        return report_error(out_error, &error);
    }
    set_void(out);
    0
}

/// The `Layer`-instance method definitions.
pub(super) fn layer_method_defs() -> Vec<NativeObjThisMethodDef> {
    vec![NativeObjThisMethodDef {
        name: "modulate",
        f: layer_modulate,
    }]
}
