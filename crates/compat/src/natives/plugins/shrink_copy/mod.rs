//! The `shrinkCopy` plugin: `Layer.shrinkCopy` and `Layer.shrinkCopyFast`.
//!
//! `shrinkCopy` copies a rectangle of a source layer into a smaller
//! rectangle of the destination layer, averaging the area every destination
//! pixel covers. The average is taken at a 1/256-pixel resolution, so the
//! result is an approximation of the exact area average.
//!
//! `shrinkCopyFast` averages whole boxes of an integer size and resizes the
//! destination layer to the size of the result. Its alpha is not read: every
//! destination pixel comes out opaque.
//!
//! Both members write the destination layer's MainImage directly: the draw
//! face and the hold-alpha flag are not consulted, and neither member
//! blends.
//!
//! The arithmetic follows the plugin's own sources. The licence notice of
//! those sources is kept beside this module in `LICENSE.krkr2`, and the
//! provenance — which sources, what was taken from them and what was not —
//! is recorded in the module's page under
//! `docs/code/compat/plugins/shrink_copy.md`.
#![allow(unsafe_code)]

mod area;
mod fast;

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeInstanceMembers, NativeObjThisMethodDef, Tjs2Engine, VAL_OBJECT, Value};

use crate::natives::system::{resolve, value_as_i64};
use crate::{CompatCtx, CompatError};

/// Register the `Layer` members.
pub fn register(ctx: &mut CompatCtx) -> Result<(), CompatError> {
    ctx.register_instance_members(&NativeInstanceMembers {
        class_name: "Layer",
        methods: vec![
            NativeObjThisMethodDef {
                name: "shrinkCopy",
                f: layer_shrink_copy,
            },
            NativeObjThisMethodDef {
                name: "shrinkCopyFast",
                f: layer_shrink_copy_fast,
            },
        ],
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
        .ok_or_else(|| "shrinkCopy: no scene attached".to_string())?;
    // SAFETY: see above.
    Ok(unsafe { &mut *ptr })
}

/// The scene id of an objthis-only `Layer` member.
///
/// `Ok(None)` means the object is not a `Layer`.
fn layer_from_objthis(host: &Tjs2Engine, objthis: *mut c_void) -> Result<Option<u32>, String> {
    if objthis.is_null() {
        return Ok(None);
    }
    let object = host.retain_object_detached(objthis)?;
    crate::CompatCtx::native_instance_id(host, object.raw_id(), &["Layer"])
}

/// The scene id of the `Layer` an argument names.
fn layer_source_id(host: &Tjs2Engine, value: &Value) -> Result<u32, String> {
    if value.ty != VAL_OBJECT {
        return Err("shrinkCopy: the source is not a Layer".to_string());
    }
    let object = host
        .retain_object_arg(value)
        .map_err(|error| format!("shrinkCopy: {error}"))?;
    match crate::CompatCtx::native_instance_id(host, object.raw_id(), &["Layer"])? {
        Some(id) => Ok(id),
        None => Err("shrinkCopy: the source is not a Layer".to_string()),
    }
}

/// The TJS `AsReal` coercion: integers become reals, strings are parsed and
/// every other value is zero.
fn value_as_f64(value: &Value) -> f64 {
    match value.ty {
        tjs::VAL_INTEGER => value.integer as f64,
        tjs::VAL_REAL => value.real,
        tjs::VAL_STRING => {
            crate::natives::system::tjs_string_to_real(&crate::native_util::value_as_string(value))
        }
        _ => 0.0,
    }
}

/// The scene id of the layer a member was called on, or an error message.
fn receiver_id(host: &Tjs2Engine, objthis: *mut c_void, method: &str) -> Result<u32, String> {
    match layer_from_objthis(host, objthis) {
        Ok(Some(id)) => Ok(id),
        Ok(None) => Err(format!("Layer.{method}: the receiver is not a Layer")),
        Err(error) => Err(format!("Layer.{method}: {error}")),
    }
}

/// The bitmap of a layer's main image.
pub(super) fn layer_bitmap(
    scene: &crate::engine_api::Scene,
    layer_id: u32,
    role: &str,
) -> Result<u32, String> {
    let layer = scene
        .layer(layer_id)
        .ok_or_else(|| format!("shrinkCopy: the {role} layer no longer exists"))?;
    layer
        .bitmap
        .ok_or_else(|| format!("shrinkCopy: the {role} layer has no main image"))
}

/// The size of a bitmap, in pixels.
pub(super) fn bitmap_size(
    scene: &crate::engine_api::Scene,
    bitmap: u32,
) -> Result<(i64, i64), String> {
    let state = scene
        .bitmap(bitmap)
        .ok_or_else(|| "shrinkCopy: the image does not exist".to_string())?;
    Ok((i64::from(state.width), i64::from(state.height)))
}

/// `Layer.shrinkCopy(dleft, dtop, dwidth, dheight, src, sleft, stop, swidth,
/// sheight)`.
extern "C" fn layer_shrink_copy(
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
            "Layer.shrinkCopy: compat context is not bound",
        );
    };
    let id = match receiver_id(resolved.engine(), objthis, "shrinkCopy") {
        Ok(id) => id,
        Err(error) => return crate::native_util::report_error(out_error, &error),
    };
    // SAFETY: the trampoline passes `argc` valid entries for this call.
    let a = unsafe { crate::native_util::args(argv, argc) };
    if a.len() < 9 {
        return crate::native_util::report_error(
            out_error,
            "Layer.shrinkCopy requires 9 arguments",
        );
    }
    let source = match layer_source_id(resolved.engine(), &a[4]) {
        Ok(id) => id,
        Err(error) => return crate::native_util::report_error(out_error, &error),
    };
    if let Err(error) = area::shrink_copy(&ctx, id, source, a) {
        return crate::native_util::report_error(out_error, &error);
    }
    crate::native_util::set_void(out);
    0
}

/// `Layer.shrinkCopyFast(src, shrink_x, shrink_y)`.
extern "C" fn layer_shrink_copy_fast(
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
            "Layer.shrinkCopyFast: compat context is not bound",
        );
    };
    let id = match receiver_id(resolved.engine(), objthis, "shrinkCopyFast") {
        Ok(id) => id,
        Err(error) => return crate::native_util::report_error(out_error, &error),
    };
    // SAFETY: the trampoline passes `argc` valid entries for this call.
    let a = unsafe { crate::native_util::args(argv, argc) };
    if a.len() < 2 {
        return crate::native_util::report_error(
            out_error,
            "Layer.shrinkCopyFast requires 2 arguments",
        );
    }
    let source = match layer_source_id(resolved.engine(), &a[0]) {
        Ok(id) => id,
        Err(error) => return crate::native_util::report_error(out_error, &error),
    };
    let step_x = value_as_i64(&a[1]);
    // The vertical step is optional: a missing or zero value repeats the
    // horizontal one.
    let step_y = if a.len() > 2 { value_as_i64(&a[2]) } else { 0 };
    if let Err(error) = fast::shrink_copy_fast(&ctx, id, source, step_x, step_y) {
        return crate::native_util::report_error(out_error, &error);
    }
    crate::native_util::set_void(out);
    0
}
