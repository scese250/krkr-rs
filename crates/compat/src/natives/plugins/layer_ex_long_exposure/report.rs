//! The members that report the accumulation: `statExposure` and
//! `copyExposure`.
//!
//! The statistics are the accumulated sums, and the copy writes the
//! accumulation back into the layer's own image, stretched by the ranges the
//! argument carries.

use std::ffi::{c_char, c_int, c_void};

use tjs::{
    DetachedValue, MemberRead, NativeObjThisMethodDef, Tjs2Engine, Tjs2ValueId, TjsValue,
    VAL_OBJECT, VAL_VOID, Value,
};

use crate::CompatCtx;
use crate::native_util::{callback_args, report_error, set_void};
use crate::natives::construct;
use crate::natives::system::tjs_string_to_integer;

use super::{
    NOT_A_LAYER, Ranges, bound, exposure_state, image_size, receiver_id, release_gone, scene_mut,
    scene_ref,
};

/// The message the statistics raise when the layer carries no accumulation.
const STAT_NOT_INITIALIZED: &str = "LongExposure.stat: not initialized";
/// The message the copy raises when the layer carries no accumulation.
const COPY_NOT_INITIALIZED: &str = "LongExposure.copy: not initialized";
/// The message the copy raises when the layer has no image to write into.
const COPY_INVALID_IMAGE: &str = "LongExposure.copy: invalid layer image";
/// The message the copy raises when the image size changed.
const COPY_INVALID_SIZE: &str = "LongExposure.copy: invalid layer size";

/// The members the argument may carry: the name, the channel it names, and
/// whether it is the lower or the upper bound of that channel.
const MEMBERS: [(&str, usize, bool); 8] = [
    ("min_r", 0, true),
    ("min_g", 1, true),
    ("min_b", 2, true),
    ("min_a", 3, true),
    ("max_r", 0, false),
    ("max_g", 1, false),
    ("max_b", 2, false),
    ("max_a", 3, false),
];

// ---------------------------------------------------------------------------
// Reading the argument
// ---------------------------------------------------------------------------

/// The engine's conversion error for a value that cannot become an integer.
fn convert_error(readable: &str) -> String {
    format!("Cannot convert the variable type ({readable} to int)")
}

/// A member's value as an integer.
///
/// A member the object does not carry reads as zero, and a member that holds
/// something without an integer conversion raises the engine's conversion
/// error.
fn member_integer(host: &Tjs2Engine, id: Tjs2ValueId, name: &str) -> Result<i64, String> {
    match host.member_read(id, name)? {
        MemberRead::Void => Ok(0),
        MemberRead::Scalar(TjsValue::Integer(value)) => Ok(value),
        // A real is truncated towards zero, the way the engine's own integer
        // conversion truncates it.
        MemberRead::Scalar(TjsValue::Real(value)) => Ok(value as i64),
        MemberRead::Scalar(TjsValue::String(value)) => Ok(tjs_string_to_integer(&value)),
        MemberRead::Object(_) => Err(convert_error("(object)")),
        MemberRead::Octet(_) => Err(convert_error("(octet)")),
        // A member read never produces another shape; one that did cannot
        // become an integer either.
        MemberRead::Scalar(_) => Err(convert_error("(value)")),
    }
}

/// The ranges an object carries.
///
/// Every one of the eight members is read as an integer; a member the object
/// does not carry reads as zero.
fn ranges_from_object(host: &Tjs2Engine, object: &DetachedValue) -> Result<Ranges, String> {
    let mut ranges = Ranges {
        lo: [0; 4],
        hi: [0; 4],
    };
    for (name, channel, is_lower) in MEMBERS {
        let value = member_integer(host, object.raw_id(), name)?;
        // The value is used as an unsigned 32-bit number: a negative value
        // wraps to its two's complement and a larger one keeps its low 32
        // bits.
        let value = value as u32;
        if is_lower {
            ranges.lo[channel] = value;
        } else {
            ranges.hi[channel] = value;
        }
    }
    Ok(ranges)
}

// ---------------------------------------------------------------------------
// The members
// ---------------------------------------------------------------------------

/// The accumulated extremes of the layer's accumulation.
fn stat_exposure(ctx: &CompatCtx, id: u32) -> Result<Ranges, String> {
    release_gone(ctx, id);
    let state = exposure_state(ctx);
    let Some(accumulation) = state.layers.get(&id) else {
        return Err(STAT_NOT_INITIALIZED.to_string());
    };
    Ok(accumulation.extremes())
}

/// Write the normalised accumulation into the layer's own image.
fn copy_exposure(
    ctx: &CompatCtx,
    host: &Tjs2Engine,
    id: u32,
    argument: &Value,
) -> Result<(), String> {
    release_gone(ctx, id);
    {
        let state = exposure_state(ctx);
        if !state.layers.contains_key(&id) {
            return Err(COPY_NOT_INITIALIZED.to_string());
        }
    }
    let size = {
        let scene = scene_ref(ctx)?;
        image_size(scene, id)
    };
    let Some((width, height)) = size else {
        return Err(COPY_INVALID_IMAGE.to_string());
    };
    {
        let state = exposure_state(ctx);
        let Some(accumulation) = state.layers.get(&id) else {
            return Err(COPY_NOT_INITIALIZED.to_string());
        };
        if accumulation.width != width || accumulation.height != height {
            return Err(COPY_INVALID_SIZE.to_string());
        }
    }
    let needed = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| COPY_INVALID_IMAGE.to_string())?;
    // The writable image is asked for before the argument is read, so a layer
    // without one is refused before the argument is looked at, and the image
    // is marked modified even when the argument turns out to be unusable.
    {
        let scene = scene_mut(ctx)?;
        let Some(bitmap_id) = scene.layer(id).and_then(|layer| layer.bitmap) else {
            return Err(COPY_INVALID_IMAGE.to_string());
        };
        scene.resolve_bitmap(bitmap_id);
        let Some(bitmap) = scene.bitmap(bitmap_id) else {
            return Err(COPY_INVALID_IMAGE.to_string());
        };
        if bitmap.rgba.len() < needed {
            log::error!(
                "compat.layer: Layer.copyExposure: the image of the layer {id} declares \
                 {width}x{height} pixels but does not hold them"
            );
            return Err(COPY_INVALID_IMAGE.to_string());
        }
        scene.mark_layer_image_modified(id);
    }
    // The argument carries the ranges the accumulation is stretched by: a
    // void argument asks for the accumulation's own extremes, an object
    // carries the eight members, and anything else leaves the built-in pair
    // — a lower bound above the upper one, so every channel is written as
    // 255.
    let ranges = match argument.ty {
        VAL_VOID => {
            let state = exposure_state(ctx);
            let Some(accumulation) = state.layers.get(&id) else {
                return Err(COPY_NOT_INITIALIZED.to_string());
            };
            accumulation.extremes()
        }
        VAL_OBJECT => {
            let object = host
                .retain_object_arg(argument)
                .map_err(|error| format!("LongExposure.copy: {error}"))?;
            ranges_from_object(host, &object)?
        }
        _ => Ranges::default(),
    };
    // The write itself: every pixel of the main image takes the normalised
    // value of its four counters.
    let scene = scene_mut(ctx)?;
    let state = exposure_state(ctx);
    let Some(accumulation) = state.layers.get(&id) else {
        return Err(COPY_NOT_INITIALIZED.to_string());
    };
    let written = scene.with_main_image_pixels_mut(id, |pixels| {
        if pixels.len() < needed {
            return false;
        }
        accumulation.write(&ranges, pixels);
        true
    });
    match written {
        Some(true) => Ok(()),
        Some(false) => {
            log::error!(
                "compat.layer: Layer.copyExposure: the image of the layer {id} declares \
                 {width}x{height} pixels but does not hold them"
            );
            Err(COPY_INVALID_IMAGE.to_string())
        }
        None => Err(COPY_INVALID_IMAGE.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The `Layer` members
// ---------------------------------------------------------------------------

extern "C" fn layer_stat_exposure(
    engine: *mut c_void,
    objthis: *mut c_void,
    _argc: c_int,
    _argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let (resolved, ctx) = match bound(engine) {
        Ok(binding) => binding,
        Err(error) => return report_error(out_error, &error),
    };
    let id = match receiver_id(resolved.engine(), objthis, NOT_A_LAYER) {
        Ok(id) => id,
        Err(error) => return report_error(out_error, &error),
    };
    let ranges = match stat_exposure(&ctx, id) {
        Ok(ranges) => ranges,
        Err(error) => return report_error(out_error, &error),
    };
    let dictionary = match construct::dictionary_of(
        resolved.engine(),
        &[
            ("min_r", TjsValue::Integer(i64::from(ranges.lo[0]))),
            ("min_g", TjsValue::Integer(i64::from(ranges.lo[1]))),
            ("min_b", TjsValue::Integer(i64::from(ranges.lo[2]))),
            ("min_a", TjsValue::Integer(i64::from(ranges.lo[3]))),
            ("max_r", TjsValue::Integer(i64::from(ranges.hi[0]))),
            ("max_g", TjsValue::Integer(i64::from(ranges.hi[1]))),
            ("max_b", TjsValue::Integer(i64::from(ranges.hi[2]))),
            ("max_a", TjsValue::Integer(i64::from(ranges.hi[3]))),
        ],
    ) {
        Ok(dictionary) => dictionary,
        Err(error) => return report_error(out_error, &format!("LongExposure.stat: {error}")),
    };
    set_object_out(out, dictionary);
    0
}

extern "C" fn layer_copy_exposure(
    engine: *mut c_void,
    objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let arguments = callback_args(argv, argc);
    if arguments.is_empty() {
        // The member takes one argument, and the call is refused before the
        // member runs.
        return report_error(out_error, "Invalid argument count");
    }
    let (resolved, ctx) = match bound(engine) {
        Ok(binding) => binding,
        Err(error) => return report_error(out_error, &error),
    };
    let id = match receiver_id(resolved.engine(), objthis, NOT_A_LAYER) {
        Ok(id) => id,
        Err(error) => return report_error(out_error, &error),
    };
    match copy_exposure(&ctx, resolved.engine(), id, &arguments[0]) {
        Ok(()) => {
            set_void(out);
            0
        }
        Err(error) => report_error(out_error, &error),
    }
}

/// Write a retained object into the out slot; the conversion consumes the
/// retention.
fn set_object_out(out: *mut Value, object: DetachedValue) {
    if out.is_null() {
        return;
    }
    // SAFETY: the trampoline passes a valid out slot for the call.
    unsafe {
        (*out).ty = tjs::VAL_RETAINED;
        (*out).integer = 0;
        (*out).real = 0.0;
        (*out).string = std::ptr::null();
        (*out).array = std::ptr::null();
        (*out).array_count = 0;
        (*out).retained = object.raw_id().as_ptr() as usize;
    }
    // The out slot owns the object now: the conversion copies the value and
    // releases the retention, so the handle must not release it as well.
    std::mem::forget(object);
}

/// The `Layer`-instance method definitions.
pub(super) fn method_defs() -> Vec<NativeObjThisMethodDef> {
    vec![
        NativeObjThisMethodDef {
            name: "statExposure",
            f: layer_stat_exposure,
        },
        NativeObjThisMethodDef {
            name: "copyExposure",
            f: layer_copy_exposure,
        },
    ]
}
