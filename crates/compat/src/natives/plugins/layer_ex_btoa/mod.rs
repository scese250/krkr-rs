//! The `layerExBTOA` plugin: the eight members a `Layer` class gains when a
//! script links `layerExBTOA.dll`.
//!
//! The members move alpha data between a layer's image and its province
//! plane:
//!
//! * `copyRightBlueToLeftAlpha` and `copyBottomBlueToTopAlpha` move the blue
//!   byte of one half of the image into the alpha byte of the other half —
//!   the shape a decoded video frame with its alpha in a colour channel
//!   arrives in.
//! * `fillAlpha` makes the alpha channel opaque over the clip rectangle.
//! * `copyAlphaToProvince` copies the alpha channel into the province plane,
//!   optionally thresholded.
//! * `clipAlphaRect` multiplies the destination's alpha by the source's
//!   alpha over a rectangle.
//! * `overwrapRect` lays the source over the destination, skipping the
//!   source pixels whose alpha is below a threshold.
//! * `fillByProvince` fills the pixels whose province byte matches an index.
//! * `fillToProvince` marks the pixels of a colour in the province plane and
//!   answers the bounding box of the marked pixels.
//!
//! The plugin defines no class of its own: the members hang on the engine's
//! own `Layer` class, and the plugin table entry is what publishes them.
//!
//! # The channel scaling
//!
//! The alpha members scale a channel by a factor with a fixed-point
//! approximation of a division by 255 — `(c * a + ((c * a) >> 7)) >> 8` — and
//! not the division itself. The formula is the identity for a factor of 255
//! and yields zero for a factor of zero, but it can land one step below or
//! one step above the rounded quotient; [`scale_channel`] is the one place
//! that arithmetic lives.

#![allow(unsafe_code)]

mod alpha;
mod province;
mod rect;

use std::ffi::{c_char, c_int, c_void};

use api::blend::BlendMode;
use api::geometry::PixelRegion;
use tjs::{
    NativeInstanceMembers, NativeObjThisMethodDef, Tjs2Engine, TjsValue, VAL_INTEGER, VAL_NULL,
    VAL_OBJECT, VAL_OCTET, VAL_REAL, VAL_STRING, VAL_VOID, Value,
};

use crate::engine_api::Scene;
use crate::native_util::{args, report_error, set_void, value_as_string};
use crate::natives::system::{resolve, value_as_i64};
use crate::{CompatCtx, CompatError};

/// Register the eight `Layer` members.
pub fn register(ctx: &mut CompatCtx) -> Result<(), CompatError> {
    let mut methods = alpha::method_defs();
    methods.extend(province::method_defs());
    methods.extend(rect::method_defs());
    ctx.register_instance_members(&NativeInstanceMembers {
        class_name: "Layer",
        methods,
        properties: Vec::new(),
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The installed engine → context binding for a native callback.
pub(super) fn bound(
    engine: *mut c_void,
) -> Option<(crate::natives::system::EngineBinding, CompatCtx)> {
    let resolved = resolve(engine)?;
    let ctx = resolved.ctx();
    Some((resolved, ctx))
}

/// The attached scene, mutably.
pub(super) fn scene_mut<'a>(ctx: &CompatCtx) -> Result<&'a mut Scene, String> {
    // SAFETY: the host keeps the attached scene alive for the binding's
    // lifetime and the VM is single-threaded.
    let ptr = unsafe { ctx.services() }
        .scene()
        .ok_or_else(|| "layerExBTOA: no scene attached".to_string())?;
    // SAFETY: see above.
    Ok(unsafe { &mut *ptr })
}

/// The value's readable form, the way the engine spells a value inside a
/// conversion error.
fn readable_value(value: &Value) -> String {
    match value.ty {
        VAL_VOID => "()".to_string(),
        VAL_INTEGER => format!("(int){}", value.integer),
        VAL_REAL => format!("(real){}", value.real),
        VAL_STRING => format!("(string)\"{}\"", value_as_string(value)),
        VAL_OCTET => "(octet)".to_string(),
        _ => "(object)".to_string(),
    }
}

/// The engine's conversion error for a value that cannot become `target`.
pub(super) fn convert_error(value: &Value, target: &str) -> String {
    format!(
        "Cannot convert the variable type ({} to {})",
        readable_value(value),
        target
    )
}

/// The value as an integer, the engine's integer conversion.
///
/// A value that is not a number and not a string cannot become one, so the
/// engine's conversion error is raised instead of a silent zero.
pub(super) fn integer_value(value: &Value) -> Result<i64, String> {
    match value.ty {
        VAL_INTEGER | VAL_REAL | VAL_STRING | VAL_VOID => Ok(value_as_i64(value)),
        _ => Err(convert_error(value, "int")),
    }
}

/// The argument at `index`, when the script passed it and it is not `void`.
///
/// A `void` argument counts as not passed: the member keeps its default.
pub(super) fn optional_argument<'a>(arguments: &'a [Value], index: usize) -> Option<&'a Value> {
    let value = arguments.get(index)?;
    if value.ty == VAL_VOID {
        None
    } else {
        Some(value)
    }
}

/// The scene id of the layer a member was called on.
///
/// The member is a `Layer` method, so the receiver names a layer; a receiver
/// that does not carries the member's own "must be a layer" message.
pub(super) fn receiver_id(
    host: &Tjs2Engine,
    objthis: *mut c_void,
    message: &str,
) -> Result<u32, String> {
    if objthis.is_null() {
        return Err(message.to_string());
    }
    let object = host.retain_object_detached(objthis)?;
    match crate::CompatCtx::native_instance_id(host, object.raw_id(), &["Layer"])? {
        Some(id) => Ok(id),
        None => Err(message.to_string()),
    }
}

/// The scene id of the layer an argument names.
///
/// A value that is not an object cannot become one, so the engine's
/// conversion error is raised; an object that names no layer carries the
/// member's own "must be a layer" message.
pub(super) fn layer_argument(
    host: &Tjs2Engine,
    value: &Value,
    message: &str,
) -> Result<u32, String> {
    match value.ty {
        VAL_OBJECT => {
            let object = host.retain_object_arg(value)?;
            match crate::CompatCtx::native_instance_id(host, object.raw_id(), &["Layer"])? {
                Some(id) => Ok(id),
                None => Err(message.to_string()),
            }
        }
        // A null is an object without a payload, so it names no layer.
        VAL_NULL => Err(message.to_string()),
        _ => Err(convert_error(value, "Object")),
    }
}

/// A script coordinate as the geometry's own number.
///
/// The coordinate is clamped to the 32-bit range the rectangle arithmetic
/// works in, so a script cannot overflow the arithmetic that pairs the two
/// rectangles.
pub(super) fn coordinate(value: i64) -> i64 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX))
}

/// The value as a byte, when it names one.
pub(super) fn byte_option(value: i64) -> Option<u8> {
    if (0..=255).contains(&value) {
        // The value is in the byte range, so the cast keeps every bit of it.
        Some(value as u8)
    } else {
        None
    }
}

/// The channel scaling the alpha members use.
///
/// The formula is a fixed-point approximation of a division by 255 and not the
/// division itself, so it can land one step below or one step above the
/// rounded quotient. It is the identity for a factor of 255 and yields zero
/// for a factor of zero.
pub(super) fn scale_channel(value: u8, factor: u8) -> u8 {
    let product = u32::from(value) * u32::from(factor);
    // The product and its correction fit in 32 bits, and the sum stays
    // below 256 for every pair of bytes.
    ((product + (product >> 7)) >> 8) as u8
}

/// The whole pixel scaled by `factor`: every channel, the alpha included.
pub(super) fn scale_pixel(pixel: [u8; 4], factor: u8) -> [u8; 4] {
    [
        scale_channel(pixel[0], factor),
        scale_channel(pixel[1], factor),
        scale_channel(pixel[2], factor),
        scale_channel(pixel[3], factor),
    ]
}

/// The packed colour word's channels: the red, green and blue bytes and the
/// alpha byte.
pub(super) fn word_to_rgba(word: u32) -> [u8; 4] {
    [
        ((word >> 16) & 0xff) as u8,
        ((word >> 8) & 0xff) as u8,
        (word & 0xff) as u8,
        ((word >> 24) & 0xff) as u8,
    ]
}

/// The low byte of a value.
///
/// The value is truncated to 32 bits and then to its low byte, so a negative
/// value wraps around instead of naming no byte.
pub(super) fn low_byte(value: i64) -> u8 {
    let word = value as u32;
    // The masked word is below 256, so the cast keeps every bit of it.
    (word & 0xff) as u8
}

/// The byte offset of the pixel at `(x, y)` in an image `width` pixels wide.
pub(super) fn pixel_offset(width: u32, x: i64, y: i64) -> Option<usize> {
    let column = u32::try_from(x).ok()?;
    let row = u32::try_from(y).ok()?;
    let index = usize::try_from(row)
        .ok()?
        .checked_mul(usize::try_from(width).ok()?)?;
    index
        .checked_add(usize::try_from(column).ok()?)?
        .checked_mul(4)
}

/// The byte offset of the province byte at `(x, y)` in a plane `width` bytes
/// wide.
pub(super) fn plane_offset(width: u32, x: i64, y: i64) -> Option<usize> {
    let column = u32::try_from(x).ok()?;
    let row = u32::try_from(y).ok()?;
    let index = usize::try_from(row)
        .ok()?
        .checked_mul(usize::try_from(width).ok()?)?;
    index.checked_add(usize::try_from(column).ok()?)
}

/// The number of bytes an image of `width` x `height` RGBA pixels holds.
fn image_bytes(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)
        .and_then(|count| count.checked_mul(4))
}

/// The layer's main image: its bitmap id and its size in pixels.
///
/// The image is resolved first: an image that is still decoding holds no
/// pixels yet, and every member that writes waits for it.
pub(super) fn layer_image(scene: &mut Scene, id: u32) -> Result<(u32, u32, u32), String> {
    let bitmap_id = scene
        .layer(id)
        .and_then(|layer| layer.bitmap)
        .ok_or_else(|| "the layer has no main image".to_string())?;
    scene.resolve_bitmap(bitmap_id);
    let bitmap = scene
        .bitmap(bitmap_id)
        .ok_or_else(|| "the layer's image no longer exists".to_string())?;
    let (width, height) = (bitmap.width, bitmap.height);
    let Some(bytes) = image_bytes(width, height) else {
        return Err("the layer's image size is not representable".to_string());
    };
    if bytes > bitmap.rgba.len() {
        return Err("the layer's image does not hold its pixels".to_string());
    }
    Ok((bitmap_id, width, height))
}

/// The layer's clip rectangle in image coordinates.
///
/// A layer without a clip rectangle is clipped to its whole image, so the
/// rectangle is never empty for a layer that has an image.
pub(super) fn layer_clip(scene: &Scene, id: u32) -> (i64, i64, i64, i64) {
    let Some(layer) = scene.layer(id) else {
        return (0, 0, 0, 0);
    };
    match layer.clip {
        Some(rect) => (
            i64::from(rect.x),
            i64::from(rect.y),
            i64::from(rect.x) + i64::from(rect.w),
            i64::from(rect.y) + i64::from(rect.h),
        ),
        None => (
            0,
            0,
            i64::from(layer.image_width),
            i64::from(layer.image_height),
        ),
    }
}

/// Whether the layer's image stores an additive (premultiplied) alpha.
///
/// The scaling the alpha members use runs over the stored words, and a
/// premultiplied word is scaled as it stands: the colour and the alpha fall
/// by the same factor, which leaves the colour's own value alone.
pub(super) fn is_add_alpha(scene: &Scene, id: u32) -> bool {
    scene
        .layer(id)
        .is_some_and(|layer| layer.blend == BlendMode::AddAlpha)
}

/// Mark the layer's image modified after a pixel write.
pub(super) fn mark_modified(scene: &mut Scene, id: u32) {
    if let Some(layer) = scene.layer_mut(id) {
        layer.image_modified = true;
    }
}

/// Ask the layer for a repaint.
///
/// The request is the one the engine's own `update` member makes: the layer
/// tree is redrawn every frame, and the request reaches the script as the
/// layer's `onPaint` dispatch.
pub(super) fn request_refresh(scene: &mut Scene, id: u32) {
    if let Some(layer) = scene.layer_mut(id) {
        layer.pending_paint = true;
    }
}

/// The region a rectangle covers, as the bitmap marks it.
///
/// The rectangle's edges are clamped into the image's own range, so a
/// rectangle that reaches outside it marks the part that is inside.
pub(super) fn region(left: i64, top: i64, right: i64, bottom: i64) -> PixelRegion {
    let edge = |value: i64| {
        let clamped = value.clamp(0, i64::from(u32::MAX));
        // The clamped value is in the 32-bit range, so the cast keeps every
        // bit of it.
        clamped as u32
    };
    PixelRegion::new(
        edge(left),
        edge(top),
        edge(right.saturating_sub(left)),
        edge(bottom.saturating_sub(top)),
    )
}

/// Mark the rectangle the member wrote as changed.
pub(super) fn mark_region(scene: &mut Scene, bitmap_id: u32, region: PixelRegion) {
    if let Some(bitmap) = scene.bitmap_mut(bitmap_id) {
        bitmap.mark_dirty_rect(region);
    }
}

/// Write an array of integers into a native callback's out slot.
pub(super) fn set_integer_array_out(
    host: &Tjs2Engine,
    out: *mut Value,
    values: &[i64],
) -> Result<(), String> {
    let array = host.new_array()?;
    for value in values {
        host.array_add(array.raw_id(), &TjsValue::Integer(*value))?;
    }
    if out.is_null() {
        return Ok(());
    }
    // SAFETY: the trampoline passes a valid out slot for the call.
    unsafe {
        (*out).ty = tjs::VAL_RETAINED;
        (*out).integer = 0;
        (*out).real = 0.0;
        (*out).string = std::ptr::null();
        (*out).array = std::ptr::null();
        (*out).array_count = 0;
        (*out).retained = array.raw_id().as_ptr() as usize;
    }
    // The out slot owns the array now: the conversion copies the value and
    // releases the retention, so the handle must not release it as well.
    std::mem::forget(array);
    Ok(())
}

/// The arguments of a native callback, as a slice.
pub(super) fn callback_args<'a>(argv: *const Value, argc: c_int) -> &'a [Value] {
    // SAFETY: the trampoline passes `argc` valid values in `argv` for the
    // duration of the callback.
    unsafe { args(argv, argc) }
}

/// Report a failure through the callback's error slot.
pub(super) fn fail(out_error: *mut *mut c_char, message: &str) -> c_int {
    report_error(out_error, message)
}

/// Finish a member that returns nothing.
pub(super) fn done(out: *mut Value) -> c_int {
    set_void(out);
    0
}
