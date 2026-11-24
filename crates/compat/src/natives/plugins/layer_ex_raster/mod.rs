//! The `layerExRaster` plugin: `Layer.copyRaster`.
//!
//! `Layer.copyRaster(src, maxh, lines, cycle, time)` copies the destination
//! layer's clip rectangle from `src`'s main image into the destination's main
//! image, shifting each row horizontally by a sine wave whose period is
//! `lines` rows, whose animation period is `cycle` milliseconds and whose
//! peak displacement is `maxh` pixels.
//!
//! Pixels are copied verbatim as 32-bit words with no blending, no alpha
//! arithmetic and no colour conversion; the province plane is not touched.
//! When the source and destination images differ in size, the call does
//! nothing and requests no redraw.
//!
//! Error handling for invalid arguments:
//!
//! * a `src` argument that is not an object or is an object that is not a
//!   `Layer` raises a typed script error;
//! * `lines <= 0` or `cycle <= 0` raises a typed script error.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_void};

use api::geometry::PixelRegion;
use tjs::{NativeInstanceMembers, NativeObjThisMethodDef, Tjs2Engine, VAL_OBJECT, Value};

use crate::engine_api::layer_ops::{self, RectI};
use crate::engine_api::{BitmapState, Scene};
use crate::natives::system::{resolve, try_value_as_i64};
use crate::{CompatCtx, CompatError};

/// Wave parameters for one `Layer.copyRaster` invocation.
#[derive(Clone, Copy)]
struct RasterParams {
    /// Peak horizontal displacement, in pixels (32-bit signed).
    maxh: i32,
    /// Number of image rows in one wave period (positive 32-bit signed).
    lines: i32,
    /// Animation period, in milliseconds (positive 32-bit signed).
    cycle: i32,
    /// Current time, in milliseconds (64-bit signed).
    time: i64,
}

/// Register the `Layer` members.
pub fn register(ctx: &mut CompatCtx) -> Result<(), CompatError> {
    ctx.register_instance_members(&NativeInstanceMembers {
        class_name: "Layer",
        methods: vec![NativeObjThisMethodDef {
            name: "copyRaster",
            f: layer_copy_raster,
        }],
        properties: Vec::new(),
    })?;
    Ok(())
}

/// The installed engine → context binding for a native callback.
fn bound(engine: *mut c_void) -> Option<(crate::natives::system::EngineBinding, CompatCtx)> {
    let resolved = resolve(engine)?;
    let ctx = resolved.ctx();
    Some((resolved, ctx))
}

/// The attached scene, mutably.
fn scene_mut<'a>(ctx: &CompatCtx) -> Result<&'a mut Scene, String> {
    // SAFETY: the host keeps the attached scene alive for the binding's
    // lifetime and the VM is single-threaded.
    let ptr = unsafe { ctx.services() }
        .scene()
        .ok_or_else(|| "Layer.copyRaster: no scene attached".to_string())?;
    // SAFETY: see above.
    Ok(unsafe { &mut *ptr })
}

/// The scene id of the layer the member was called on.
///
/// `Ok(None)` means the object is not a `Layer`.
fn layer_from_objthis(host: &Tjs2Engine, objthis: *mut c_void) -> Result<Option<u32>, String> {
    if objthis.is_null() {
        return Ok(None);
    }
    let object = host.retain_object_detached(objthis)?;
    crate::CompatCtx::native_instance_id(host, object.raw_id(), &["Layer"])
}

/// The scene id of the layer the member was called on.
fn receiver_id(host: &Tjs2Engine, objthis: *mut c_void) -> Result<u32, String> {
    match layer_from_objthis(host, objthis) {
        Ok(Some(id)) => Ok(id),
        Ok(None) => Err("Layer.copyRaster: the receiver is not a Layer".to_string()),
        Err(error) => Err(format!("Layer.copyRaster: {error}")),
    }
}

/// The scene id of the layer the source argument names.
///
/// Rejects non-object arguments and non-`Layer` objects with a typed error.
fn source_id(host: &Tjs2Engine, value: &Value) -> Result<u32, String> {
    if value.ty != VAL_OBJECT {
        return Err("Layer.copyRaster: the source cannot be converted to an object".to_string());
    }
    let object = host
        .retain_object_arg(value)
        .map_err(|error| format!("Layer.copyRaster: {error}"))?;
    match crate::CompatCtx::native_instance_id(host, object.raw_id(), &["Layer"])? {
        Some(id) => Ok(id),
        None => Err("Layer.copyRaster: the source is not a Layer".to_string()),
    }
}

/// The integer value of an argument under the VM's integer coercion.
fn number(value: &Value, name: &str) -> Result<i64, String> {
    try_value_as_i64(value)
        .map_err(|error| format!("Layer.copyRaster: the {name} argument {error}"))
}

/// Narrow a 64-bit script integer to the low 32 bits, reinterpreted as signed.
fn narrow_i32(value: i64) -> i32 {
    value as i32
}

/// The size of an optional layer bitmap, in pixels.
///
/// A layer without a main image reports `0 x 0`.
fn bitmap_size(scene: &Scene, bitmap: Option<u32>) -> Result<(u32, u32), String> {
    let Some(id) = bitmap else {
        return Ok((0, 0));
    };
    let state = scene
        .bitmap(id)
        .ok_or_else(|| "Layer.copyRaster: the image does not exist".to_string())?;
    Ok((state.width, state.height))
}

/// Convert a half-open image-clipped rectangle into an unsigned [`PixelRegion`].
fn unsigned_region(rect: RectI) -> Result<PixelRegion, String> {
    let edge = |value: i32| {
        u32::try_from(value)
            .map_err(|_| "Layer.copyRaster: the clip rectangle is outside the image".to_string())
    };
    Ok(PixelRegion::from_min_max(
        edge(rect.0)?,
        edge(rect.1)?,
        edge(rect.2)?,
        edge(rect.3)?,
    ))
}

/// `Layer.copyRaster(src, maxh, lines, cycle, time)`.
extern "C" fn layer_copy_raster(
    engine: *mut c_void,
    objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let Some((resolved, ctx)) = bound(engine) else {
        return crate::native_util::report_error(
            out_error,
            "Layer.copyRaster: compat context is not bound",
        );
    };
    // SAFETY: the trampoline passes `argc` valid entries for this call.
    let a = unsafe { crate::native_util::args(argv, argc) };
    if a.len() < 5 {
        return crate::native_util::report_error(
            out_error,
            "Layer.copyRaster requires 5 arguments",
        );
    }
    match copy_raster(&ctx, resolved.engine(), objthis, a) {
        Ok(()) => {
            crate::native_util::set_void(out);
            0
        }
        Err(error) => crate::native_util::report_error(out_error, &error),
    }
}

/// Execute `Layer.copyRaster(src, maxh, lines, cycle, time)`.
fn copy_raster(
    ctx: &CompatCtx,
    host: &Tjs2Engine,
    objthis: *mut c_void,
    a: &[Value],
) -> Result<(), String> {
    let dest_id = receiver_id(host, objthis)?;
    let src_id = source_id(host, &a[0])?;
    let maxh = narrow_i32(number(&a[1], "maxh")?);
    let lines = narrow_i32(number(&a[2], "lines")?);
    let cycle = narrow_i32(number(&a[3], "cycle")?);
    let time = number(&a[4], "time")?;

    if lines <= 0 {
        return Err("Layer.copyRaster: lines must be positive".to_string());
    }
    if cycle <= 0 {
        return Err("Layer.copyRaster: cycle must be positive".to_string());
    }

    let scene = scene_mut(ctx)?;
    let (dest_bitmap, dest_clip) = {
        let layer = scene.layer(dest_id).ok_or_else(|| {
            "Layer.copyRaster: the destination layer no longer exists".to_string()
        })?;
        (layer.bitmap, layer_ops::clip_rect(layer.clip))
    };
    let src_bitmap = scene
        .layer(src_id)
        .ok_or_else(|| "Layer.copyRaster: the source layer no longer exists".to_string())?
        .bitmap;

    if let Some(id) = dest_bitmap {
        scene.resolve_bitmap(id);
    }
    if let Some(id) = src_bitmap {
        scene.resolve_bitmap(id);
    }

    let dest_size = bitmap_size(scene, dest_bitmap)?;
    let src_size = bitmap_size(scene, src_bitmap)?;
    if dest_size != src_size {
        return Ok(());
    }

    if let (Some(dest_bmp), Some(src_bmp)) = (dest_bitmap, src_bitmap) {
        let region = {
            let bitmap = scene.bitmap(dest_bmp).ok_or_else(|| {
                "Layer.copyRaster: the destination image does not exist".to_string()
            })?;
            match layer_ops::intersect(bitmap, dest_clip) {
                Some(rect) => unsigned_region(rect)?,
                None => PixelRegion::ZERO,
            }
        };
        if !region.is_empty() {
            let params = RasterParams {
                maxh,
                lines,
                cycle,
                time,
            };
            let copied = scene.with_two_bitmaps(dest_bmp, src_bmp, |dest, source| {
                copy_raster_into(dest, source, region, params)
            });
            match copied {
                Some(Ok(())) => {}
                Some(Err(error)) => return Err(error),
                None => {
                    return Err("Layer.copyRaster: the image does not exist".to_string());
                }
            }
        }
    }

    // Whenever the size guard passed, the destination's image is marked
    // modified and a repaint is requested, even if the clip rectangle is empty.
    scene.mark_layer_image_modified(dest_id);
    scene.set_layer_pending_paint(dest_id, true);
    Ok(())
}

/// Copy `region` row by row from `source` into `dest` with a sinusoidal
/// horizontal displacement.
fn copy_raster_into(
    dest: &mut BitmapState,
    source: &BitmapState,
    region: PixelRegion,
    params: RasterParams,
) -> Result<(), String> {
    let angular_step = std::f64::consts::TAU / f64::from(params.lines);
    let half_height = dest.height / 2;
    let mut phase =
        ((-angular_step * (params.time as f64)) / f64::from(params.cycle)) * f64::from(half_height);
    phase += angular_step * f64::from(region.y);

    let clip_width = i64::from(region.width);
    let amplitude = f64::from(params.maxh);

    for row_index in 0..region.height {
        let row = region.y + row_index;
        let displacement = (phase.sin() * amplitude).trunc() as i64;
        if displacement >= 0 {
            let count = clip_width - displacement;
            if count > 0 {
                let shift = u32::try_from(displacement).map_err(|_| {
                    "Layer.copyRaster: the destination pixel is outside the destination image"
                        .to_string()
                })?;
                let pixels = usize::try_from(count).map_err(|_| {
                    "Layer.copyRaster: the destination pixel is outside the destination image"
                        .to_string()
                })?;
                let dst_x = region.x.checked_add(shift).ok_or_else(|| {
                    "Layer.copyRaster: the destination pixel is outside the destination image"
                        .to_string()
                })?;
                copy_row_span(dest, source, row, region.x, dst_x, pixels)?;
            }
        } else {
            let shift_i64 = -displacement;
            let count = clip_width - shift_i64;
            if count > 0 {
                let shift = u32::try_from(shift_i64).map_err(|_| {
                    "Layer.copyRaster: the source pixel is outside the source image".to_string()
                })?;
                let pixels = usize::try_from(count).map_err(|_| {
                    "Layer.copyRaster: the destination pixel is outside the destination image"
                        .to_string()
                })?;
                let src_x = region.x.checked_add(shift).ok_or_else(|| {
                    "Layer.copyRaster: the source pixel is outside the source image".to_string()
                })?;
                copy_row_span(dest, source, row, src_x, region.x, pixels)?;
            }
        }
        phase += angular_step;
    }

    dest.mark_dirty_rect(region);
    Ok(())
}

/// Copy `pixels` contiguous 32-bit pixels on `row` from `src_x` in `source`
/// to `dst_x` in `dest`, verbatim.
fn copy_row_span(
    dest: &mut BitmapState,
    source: &BitmapState,
    row: u32,
    src_x: u32,
    dst_x: u32,
    pixels: usize,
) -> Result<(), String> {
    let src_outside =
        || "Layer.copyRaster: the source pixel is outside the source image".to_string();
    let dst_outside =
        || "Layer.copyRaster: the destination pixel is outside the destination image".to_string();
    let byte_len = pixels.checked_mul(4).ok_or_else(dst_outside)?;
    let src_offset = source.pixel_offset(src_x, row).ok_or_else(src_outside)?;
    let src_end = src_offset.checked_add(byte_len).ok_or_else(src_outside)?;
    let dst_offset = dest.pixel_offset(dst_x, row).ok_or_else(dst_outside)?;
    let dst_end = dst_offset.checked_add(byte_len).ok_or_else(dst_outside)?;
    let src_bytes = source
        .rgba
        .get(src_offset..src_end)
        .ok_or_else(src_outside)?;
    let dst_bytes = dest
        .rgba
        .get_mut(dst_offset..dst_end)
        .ok_or_else(dst_outside)?;
    dst_bytes.copy_from_slice(src_bytes);
    Ok(())
}
