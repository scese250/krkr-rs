//! `Layer.noise(level)` and `Layer.generateWhiteNoise()`: the two members
//! that draw from the plugin's random source.
//!
//! `noise` adds an independent random offset to each colour channel of
//! every pixel of the clip rectangle; the alpha byte is left alone. The
//! offset is the truncation toward zero of the draw's distance from the
//! middle of the range, so a draw below the middle darkens the channel and
//! a draw above it brightens it, and the level is not clamped: a negative
//! level mirrors the offsets of a positive one, and a level above 255 widens
//! the range of the offsets.
//!
//! `generateWhiteNoise` gives every pixel of the clip rectangle the same
//! uniform grey; the colour the pixel had is ignored and the alpha byte is
//! left alone.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeObjThisMethodDef, Value};

use super::{for_each_pixel, integer_arg, receiver, with_clip};
use crate::CompatCtx;
use crate::native_util::{callback_args, report_error, set_void};

/// The plugin's random source.
///
/// The generator is a xorshift whose state lives in the binding's state, so
/// a run of the plugin is reproducible: the same calls in the same order
/// produce the same pixels. The members come from a plugin that draws from
/// a process-wide random generator whose sequence is neither seeded nor
/// reproducible, so the distribution, the truncation and the clamping are
/// kept and the sequence is not.
pub(super) struct Random {
    /// The generator's state; it is never zero.
    state: u64,
}

impl Default for Random {
    fn default() -> Self {
        // A fixed seed: the sequence of the draws is the same in every run.
        Self {
            state: 0x9e37_79b9_7f4a_7c15,
        }
    }
}

impl Random {
    /// A draw in the range the plugin's random source answers.
    fn draw(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as u32
    }

    /// A uniform value between zero and one.
    fn unit(&mut self) -> f64 {
        f64::from(self.draw()) / f64::from(i32::MAX)
    }

    /// A uniform byte.
    ///
    /// The draw is divided by the integer constant that the largest draw
    /// divided by 255 gives, so the value is uniform over 0..=255.
    fn byte(&mut self) -> u8 {
        (self.draw() / 8_421_504) as u8
    }
}

/// Apply the noise to the layer's image.
fn noise(ctx: &CompatCtx, id: u32, level: i64) -> Result<(), String> {
    let level = level as i32;
    // SAFETY: the caller resolved `ctx` from the installed binding.
    let services = unsafe { ctx.services() };
    let mut random = services.state::<Random>();
    with_clip(ctx, id, "noise", |bitmap, region| {
        for_each_pixel(bitmap, region, "noise", |pixel| {
            // The three colour channels draw on their own, in the buffer's
            // own order: the blue, the green and the red. The alpha byte is
            // left alone.
            for index in [2usize, 1, 0] {
                let offset = ((random.unit() - 0.5) * f64::from(level)) as i32;
                pixel[index] = (i32::from(pixel[index]) + offset).clamp(0, 255) as u8;
            }
        })
    })
}

/// Apply the white noise to the layer's image.
fn generate_white_noise(ctx: &CompatCtx, id: u32) -> Result<(), String> {
    // SAFETY: the caller resolved `ctx` from the installed binding.
    let services = unsafe { ctx.services() };
    let mut random = services.state::<Random>();
    with_clip(ctx, id, "generateWhiteNoise", |bitmap, region| {
        for_each_pixel(bitmap, region, "generateWhiteNoise", |pixel| {
            let value = random.byte();
            pixel[0] = value;
            pixel[1] = value;
            pixel[2] = value;
        })
    })
}

extern "C" fn layer_noise(
    engine: *mut c_void,
    objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let (ctx, id) = match receiver(engine, objthis, "noise") {
        Ok(pair) => pair,
        Err(error) => return report_error(out_error, &error),
    };
    let a = callback_args(argv, argc);
    if a.is_empty() {
        return report_error(out_error, "Layer.noise requires 1 argument");
    }
    let level = match integer_arg(&a[0], "noise", "level") {
        Ok(value) => value,
        Err(error) => return report_error(out_error, &error),
    };
    if let Err(error) = noise(&ctx, id, level) {
        return report_error(out_error, &error);
    }
    set_void(out);
    0
}

extern "C" fn layer_generate_white_noise(
    engine: *mut c_void,
    objthis: *mut c_void,
    _argc: c_int,
    _argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let (ctx, id) = match receiver(engine, objthis, "generateWhiteNoise") {
        Ok(pair) => pair,
        Err(error) => return report_error(out_error, &error),
    };
    if let Err(error) = generate_white_noise(&ctx, id) {
        return report_error(out_error, &error);
    }
    set_void(out);
    0
}

/// The `Layer`-instance method definitions.
pub(super) fn layer_method_defs() -> Vec<NativeObjThisMethodDef> {
    vec![
        NativeObjThisMethodDef {
            name: "noise",
            f: layer_noise,
        },
        NativeObjThisMethodDef {
            name: "generateWhiteNoise",
            f: layer_generate_white_noise,
        },
    ]
}
