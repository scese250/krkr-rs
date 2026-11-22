//! The `layerExAreaAverage` plugin: `Layer.stretchCopyAA`.
//!
//! `stretchCopyAA` copies a rectangle of a source layer into a rectangle of
//! the destination layer that is no larger than the source rectangle, and
//! averages the area every destination pixel covers. The average is taken in
//! a fixed-point form with twelve fractional bits, and the colour of a
//! source pixel is weighted by the area it covers times its alpha.
//!
//! Two quirks of the plugin's own arithmetic are kept, because a copy that
//! repairs them no longer matches it:
//!
//! * the source image's last column and last row are never sampled, so a
//!   copy of the same size leaves the destination's last column and last row
//!   untouched;
//! * the covered region of a destination pixel is anchored at the source
//!   rectangle's origin, so a source rectangle that does not start at the
//!   origin is still drawn where it was asked for.
//!
//! The copy writes the destination layer's main image directly: the draw
//! face, the hold-alpha flag and the province are not consulted, and the
//! member does not blend. A destination pixel that covers no source pixel
//! keeps the content it had.
//!
//! The arithmetic and its two quirks are worked out in the `area` module.
#![allow(unsafe_code)]

mod area;

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeInstanceMembers, NativeObjThisMethodDef, Tjs2Engine, VAL_OBJECT, Value};

use crate::natives::system::{resolve, try_value_as_i64};
use crate::{CompatCtx, CompatError};

/// Register the `Layer` members.
pub fn register(ctx: &mut CompatCtx) -> Result<(), CompatError> {
    ctx.register_instance_members(&NativeInstanceMembers {
        class_name: "Layer",
        methods: vec![NativeObjThisMethodDef {
            name: "stretchCopyAA",
            f: layer_stretch_copy_aa,
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
fn scene_mut<'a>(ctx: &CompatCtx) -> Result<&'a mut crate::engine_api::Scene, String> {
    // SAFETY: the host keeps the attached scene alive for the binding's
    // lifetime and the VM is single-threaded.
    let ptr = unsafe { ctx.services() }
        .scene()
        .ok_or_else(|| "Layer.stretchCopyAA: no scene attached".to_string())?;
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
        Ok(None) => Err("Layer.stretchCopyAA: the receiver is not a Layer".to_string()),
        Err(error) => Err(format!("Layer.stretchCopyAA: {error}")),
    }
}

/// The scene id of the layer the source argument names.
///
/// The plugin reads the source's image through the layer the argument names;
/// an argument that is not an object, or an object that is not a layer, has
/// no image to read.
fn source_id(host: &Tjs2Engine, value: &Value) -> Result<u32, String> {
    if value.ty != VAL_OBJECT {
        return Err("Layer.stretchCopyAA: the source cannot be converted to an object".to_string());
    }
    let object = host
        .retain_object_arg(value)
        .map_err(|error| format!("Layer.stretchCopyAA: {error}"))?;
    match crate::CompatCtx::native_instance_id(host, object.raw_id(), &["Layer"])? {
        Some(id) => Ok(id),
        None => Err("Layer.stretchCopyAA: the source is not a Layer".to_string()),
    }
}

/// The integer value of an argument.
///
/// The numbers are read with the VM's integer conversion: a real is
/// truncated toward zero, a string is parsed, a void is zero, and an
/// object, a null or an octet raise the conversion error.
fn number(value: &Value, name: &str) -> Result<i64, String> {
    try_value_as_i64(value)
        .map_err(|error| format!("Layer.stretchCopyAA: the {name} argument {error}"))
}

/// The bitmap of a layer's main image.
fn layer_bitmap(
    scene: &crate::engine_api::Scene,
    layer_id: u32,
    role: &str,
) -> Result<u32, String> {
    let layer = scene
        .layer(layer_id)
        .ok_or_else(|| format!("Layer.stretchCopyAA: the {role} layer no longer exists"))?;
    layer
        .bitmap
        .ok_or_else(|| format!("Layer.stretchCopyAA: the {role} layer has no main image"))
}

/// The size of a bitmap, in pixels.
fn bitmap_size(scene: &crate::engine_api::Scene, bitmap: u32) -> Result<(i64, i64), String> {
    let state = scene
        .bitmap(bitmap)
        .ok_or_else(|| "Layer.stretchCopyAA: the image does not exist".to_string())?;
    Ok((i64::from(state.width), i64::from(state.height)))
}

/// `Layer.stretchCopyAA(dleft, dtop, dwidth, dheight, src, sleft, stop,
/// swidth, sheight)`.
extern "C" fn layer_stretch_copy_aa(
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
            "Layer.stretchCopyAA: compat context is not bound",
        );
    };
    // SAFETY: the trampoline passes `argc` valid entries for this call.
    let a = unsafe { crate::native_util::args(argv, argc) };
    if a.len() < 9 {
        return crate::native_util::report_error(
            out_error,
            "Layer.stretchCopyAA requires 9 arguments",
        );
    }
    match stretch_copy_aa(&ctx, resolved.engine(), objthis, &a) {
        Ok(()) => {
            crate::native_util::set_void(out);
            0
        }
        Err(error) => crate::native_util::report_error(out_error, &error),
    }
}

/// The copy itself.
fn stretch_copy_aa(
    ctx: &CompatCtx,
    host: &Tjs2Engine,
    objthis: *mut c_void,
    a: &[Value],
) -> Result<(), String> {
    let dest_id = receiver_id(host, objthis)?;
    let dest_left = number(&a[0], "dleft")?;
    let dest_top = number(&a[1], "dtop")?;
    let dest_width = number(&a[2], "dwidth")?;
    let dest_height = number(&a[3], "dheight")?;
    let src_id = source_id(host, &a[4])?;
    let src_left = number(&a[5], "sleft")?;
    let src_top = number(&a[6], "stop")?;
    let src_width = number(&a[7], "swidth")?;
    let src_height = number(&a[8], "sheight")?;

    // The copy only shrinks: a destination rectangle wider or taller than
    // the source rectangle is refused, and the check runs before the
    // clipping, so a destination that the clipping alone would shrink to
    // the source's size is refused as well.
    if dest_width > src_width || dest_height > src_height {
        return Err(
            "Layer.stretchCopyAA: cannot magnify; the destination rectangle must not be wider \
             or taller than the source rectangle"
                .to_string(),
        );
    }
    // A negative origin addresses memory outside the image, which the
    // plugin's own arithmetic leaves undefined. The copy refuses it instead
    // of writing outside the buffer.
    if dest_left < 0 || dest_top < 0 {
        return Err("Layer.stretchCopyAA: the destination origin is negative".to_string());
    }
    if src_left < 0 || src_top < 0 {
        return Err("Layer.stretchCopyAA: the source origin is negative".to_string());
    }

    let request = area::Request {
        dest_left,
        dest_top,
        dest_width,
        dest_height,
        src_left,
        src_top,
        src_width,
        src_height,
    };

    let scene = scene_mut(ctx)?;
    let dest_bitmap = layer_bitmap(scene, dest_id, "destination")?;
    let src_bitmap = layer_bitmap(scene, src_id, "source")?;
    // The images may still be decoding: the copy reads and writes their
    // pixels.
    scene.resolve_bitmap(dest_bitmap);
    scene.resolve_bitmap(src_bitmap);
    let dest_size = bitmap_size(scene, dest_bitmap)?;
    let src_size = bitmap_size(scene, src_bitmap)?;

    let clipped = area::clip(&request, dest_size, src_size);
    let copied = scene.with_two_bitmaps(dest_bitmap, src_bitmap, |dest, source| {
        area::copy_into(dest, source, &request, &clipped, src_size)
    });
    match copied {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error),
        None => {
            return Err("Layer.stretchCopyAA: the destination image does not exist".to_string());
        }
    }
    // The write goes through the destination's write address, which marks
    // the image modified, and the copy asks the destination for a repaint of
    // the rectangle it wrote.
    scene.mark_layer_image_modified(dest_id);
    scene.set_layer_pending_paint(dest_id, true);
    Ok(())
}
