//! The `layerExImage` plugin: the `Layer.colorize`, `Layer.modulate`,
//! `Layer.noise` and `Layer.generateWhiteNoise` members.
//!
//! The plugin attaches its members to the host's `Layer` class, so a script
//! calls them on a layer instance. Every member works on the layer's own
//! MainImage: it walks the clip rectangle, transforms the three colour
//! channels of each pixel and leaves the alpha byte alone.
//!
//! The members keep the plugin's own arithmetic, including its rounding:
//!
//! * `colorize` converts a colour to a byte-packed HSL, takes the requested
//!   hue and saturation and keeps the lightness, then converts back in
//!   single precision. The round trip moves a channel by up to one step,
//!   and the member keeps that loss.
//! * `modulate` is a different transform: a double-precision HSL whose hue
//!   is a fraction of a turn. The two members are not two spellings of the
//!   same operation.
//! * `noise` adds an independent random offset to each colour channel, and
//!   `generateWhiteNoise` gives every pixel the same uniform grey.
//!
//! The random source is this module's own: a generator whose state lives in
//! the binding's state, so a run of the plugin is reproducible — the same
//! calls in the same order produce the same pixels. The members come from a
//! plugin that draws from a process-wide random generator whose sequence is
//! neither seeded nor reproducible, so the distribution, the truncation and
//! the clamping are kept and the sequence is not.
//!
//! The pixel work lives in this module rather than in the engine's own
//! layer operations: the engine's `colorize` and `noise` are the tree's
//! operations with the tree's own semantics, and the plugin's members
//! differ from them (a negative noise level darkens the pixels here, while
//! the engine's operation does nothing).
#![allow(unsafe_code)]

mod colorize;
mod modulate;
mod noise;

use std::ffi::c_void;

use crate::engine_api::BitmapState;
use crate::engine_api::layer_ops::{self, RectI};
use api::geometry::PixelRegion;
use tjs::{NativeInstanceMembers, VAL_INTEGER, VAL_REAL, VAL_STRING, VAL_VOID, Value};

use super::layer_ex::{bound, layer_from_objthis, scene_mut};
use crate::CompatCtx;
use crate::CompatError;

/// Register the plugin's members on the host's `Layer` class.
pub fn register(ctx: &mut CompatCtx) -> Result<(), CompatError> {
    let mut methods = colorize::layer_method_defs();
    methods.extend(modulate::layer_method_defs());
    methods.extend(noise::layer_method_defs());
    ctx.register_instance_members(&NativeInstanceMembers {
        class_name: "Layer",
        methods,
        properties: Vec::new(),
    })?;
    Ok(())
}

/// The scene id of the layer a member was called on, and the context to work
/// in.
///
/// # Errors
///
/// Returns a message when the binding is not installed or when the receiver
/// is not a layer.
pub(super) fn receiver(
    engine: *mut c_void,
    objthis: *mut c_void,
    method: &str,
) -> Result<(CompatCtx, u32), String> {
    let (resolved, ctx) =
        bound(engine).map_err(|_| format!("Layer.{method}: compat context is not bound"))?;
    let id = match layer_from_objthis(resolved.engine(), objthis) {
        Ok(Some(id)) => id,
        Ok(None) => return Err(format!("Layer.{method}: the receiver is not a Layer")),
        Err(error) => return Err(format!("Layer.{method}: {error}")),
    };
    Ok((ctx, id))
}

/// Run `f` over the layer's main image, inside the clip rectangle.
///
/// The image is marked modified and the layer is asked to repaint, whether
/// or not the rectangle names a pixel: the write buffer is asked for before
/// anything else, and the repaint is asked for after the write.
///
/// The clip rectangle is intersected with the image, so a rectangle that
/// reaches past the image writes the pixels that are there.
///
/// # Errors
///
/// Returns an error when the layer has no main image, when its image does
/// not hold the pixels its size names, or when a pixel of the rectangle is
/// missing.
pub(super) fn with_clip<F>(ctx: &CompatCtx, id: u32, method: &str, f: F) -> Result<(), String>
where
    F: FnOnce(&mut BitmapState, PixelRegion) -> Result<(), String>,
{
    let scene = scene_mut(ctx).map_err(|_| format!("Layer.{method}: no scene attached"))?;
    let (bitmap_id, clip) = {
        let layer = scene
            .layer(id)
            .ok_or_else(|| format!("Layer.{method}: this layer no longer exists"))?;
        (layer.bitmap, layer_ops::clip_rect(layer.clip))
    };
    let Some(bitmap_id) = bitmap_id else {
        // A layer without a main image has no pixels to write; the write
        // would go through an address that names nothing.
        return Err(format!("Layer.{method}: this layer has no main image"));
    };
    // The image may still be decoding: the pixels are not there yet, and
    // every path that reads them waits for the decode.
    scene.resolve_bitmap(bitmap_id);
    let region = {
        let bitmap = scene
            .bitmap(bitmap_id)
            .ok_or_else(|| format!("Layer.{method}: this layer's image no longer exists"))?;
        if !holds_pixels(bitmap) {
            return Err(format!(
                "Layer.{method}: this layer's image does not hold its pixels"
            ));
        }
        match layer_ops::intersect(bitmap, clip) {
            Some(rect) => unsigned_region(method, rect)?,
            // The clip names no pixel of the image: nothing is written.
            None => PixelRegion::ZERO,
        }
    };
    {
        let bitmap = scene
            .bitmap_mut(bitmap_id)
            .ok_or_else(|| format!("Layer.{method}: this layer's image no longer exists"))?;
        f(bitmap, region)?;
        // The write touched the rectangle alone, so the upload needs that
        // rectangle and not the whole image.
        bitmap.mark_dirty_rect(region);
    }
    // The write marks the image modified, and the layer is asked to repaint
    // the rectangle that was written.
    scene.mark_layer_image_modified(id);
    scene.set_layer_pending_paint(id, true);
    Ok(())
}

/// Apply `f` to every pixel of the region.
///
/// The four bytes of a pixel are the image's own straight-alpha RGBA: a
/// member writes the three colour bytes and leaves the alpha byte alone.
///
/// # Errors
///
/// Returns an error when the image holds no pixel at a position of the
/// region.
pub(super) fn for_each_pixel(
    bitmap: &mut BitmapState,
    region: PixelRegion,
    method: &str,
    mut f: impl FnMut(&mut [u8; 4]),
) -> Result<(), String> {
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            let Some(offset) = bitmap.pixel_offset(x, y) else {
                return Err(format!(
                    "Layer.{method}: the image holds no pixel at ({x}, {y})"
                ));
            };
            let Some(bytes) = bitmap.rgba.get_mut(offset..offset + 4) else {
                return Err(format!(
                    "Layer.{method}: the image holds no pixel at ({x}, {y})"
                ));
            };
            let mut pixel = [bytes[0], bytes[1], bytes[2], bytes[3]];
            f(&mut pixel);
            bytes.copy_from_slice(&pixel);
        }
    }
    Ok(())
}

/// An integer argument.
///
/// A real is truncated toward zero and a string that reads as a number
/// becomes that number; a value with no numeric conversion is an error.
pub(super) fn integer_arg(value: &Value, method: &str, name: &str) -> Result<i64, String> {
    match value.ty {
        VAL_INTEGER => Ok(value.integer),
        VAL_REAL => Ok(value.real as i64),
        VAL_STRING => Ok(crate::natives::system::tjs_string_to_integer(
            &crate::native_util::value_as_string(value),
        )),
        VAL_VOID => Ok(0),
        _ => Err(format!(
            "Layer.{method}: the {name} argument is not a number"
        )),
    }
}

/// A real argument.
///
/// A string that reads as a number becomes that number; a value with no
/// numeric conversion is an error.
pub(super) fn real_arg(value: &Value, method: &str, name: &str) -> Result<f64, String> {
    match value.ty {
        VAL_INTEGER => Ok(value.integer as f64),
        VAL_REAL => Ok(value.real),
        VAL_STRING => Ok(crate::natives::system::tjs_string_to_real(
            &crate::native_util::value_as_string(value),
        )),
        VAL_VOID => Ok(0.0),
        _ => Err(format!(
            "Layer.{method}: the {name} argument is not a number"
        )),
    }
}

/// The rectangle as an unsigned region.
///
/// The rectangle comes from the image's own bounds, so its edges are never
/// negative; the conversion is checked anyway, and a rectangle that does not
/// convert is reported instead of being clamped.
fn unsigned_region(method: &str, rect: RectI) -> Result<PixelRegion, String> {
    let edge = |value: i32| {
        u32::try_from(value)
            .map_err(|_| format!("Layer.{method}: the clip rectangle is outside the image"))
    };
    Ok(PixelRegion::from_min_max(
        edge(rect.0)?,
        edge(rect.1)?,
        edge(rect.2)?,
        edge(rect.3)?,
    ))
}

/// Whether a bitmap's buffer holds every pixel its declared size names.
fn holds_pixels(bitmap: &BitmapState) -> bool {
    usize::try_from(bitmap.width)
        .ok()
        .zip(usize::try_from(bitmap.height).ok())
        .and_then(|(width, height)| width.checked_mul(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .is_some_and(|bytes| bytes <= bitmap.rgba.len())
}
