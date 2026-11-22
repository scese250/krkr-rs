//! The members that build the accumulation: `initExposure`, `snapExposure`
//! and `termExposure`.
//!
//! The accumulation belongs to one layer: it is created for the layer's
//! image size, it grows with every snap, and it is released when the layer
//! goes away or when the script asks for it.

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeObjThisMethodDef, Value};

use crate::CompatCtx;
use crate::native_util::{report_error, set_void};

use super::{
    Accumulation, NOT_A_LAYER, bound, exposure_state, image_size, receiver_id, release_gone,
    scene_mut, scene_ref,
};

/// The message the initialisation raises when the layer has no image.
const INIT_INVALID_IMAGE: &str = "LongExposure.init: invalid layer image";
/// The message a snap raises when the layer carries no accumulation.
const SNAP_NOT_INITIALIZED: &str = "LongExposure.snap: not initialized";
/// The message a snap raises when the layer has no image.
const SNAP_INVALID_IMAGE: &str = "LongExposure.snap: invalid layer image";
/// The message a snap raises when the image size changed.
const SNAP_INVALID_SIZE: &str = "LongExposure.snap: invalid layer size";

/// Start a fresh accumulation for the layer.
///
/// The accumulation the layer carried is released first, so a failed
/// initialisation leaves the layer with no accumulation at all.
fn init_exposure(ctx: &CompatCtx, id: u32) -> Result<(), String> {
    release_gone(ctx, id);
    {
        let mut state = exposure_state(ctx);
        state.layers.remove(&id);
    }
    let size = {
        let scene = scene_ref(ctx)?;
        image_size(scene, id)
    };
    let Some((width, height)) = size else {
        return Err(INIT_INVALID_IMAGE.to_string());
    };
    let accumulation =
        Accumulation::new(width, height).map_err(|error| format!("LongExposure.init: {error}"))?;
    exposure_state(ctx).layers.insert(id, accumulation);
    Ok(())
}

/// Add the layer's current image to the accumulation.
///
/// The accumulation is refused before the image is looked at, and the size
/// check precedes the addition, so an accumulation that does not fit the
/// image is left untouched.
fn snap_exposure(ctx: &CompatCtx, id: u32) -> Result<(), String> {
    release_gone(ctx, id);
    {
        let state = exposure_state(ctx);
        if !state.layers.contains_key(&id) {
            return Err(SNAP_NOT_INITIALIZED.to_string());
        }
    }
    let scene = scene_mut(ctx)?;
    let Some((width, height)) = image_size(scene, id) else {
        return Err(SNAP_INVALID_IMAGE.to_string());
    };
    let mut state = exposure_state(ctx);
    let Some(accumulation) = state.layers.get_mut(&id) else {
        return Err(SNAP_NOT_INITIALIZED.to_string());
    };
    if accumulation.width != width || accumulation.height != height {
        return Err(SNAP_INVALID_SIZE.to_string());
    }
    // The image may still be decoding: the pixels are not there yet, and the
    // snap waits for them.
    let Some(bitmap_id) = scene.layer(id).and_then(|layer| layer.bitmap) else {
        return Err(SNAP_INVALID_IMAGE.to_string());
    };
    scene.resolve_bitmap(bitmap_id);
    let needed = accumulation.bytes();
    let added = scene.with_main_image_pixels(id, |pixels| {
        if pixels.len() < needed {
            return false;
        }
        accumulation.add(pixels);
        true
    });
    match added {
        Some(true) => Ok(()),
        Some(false) => {
            log::error!(
                "compat.layer: Layer.snapExposure: the image of the layer {id} declares \
                 {width}x{height} pixels but does not hold them"
            );
            Err(SNAP_INVALID_IMAGE.to_string())
        }
        None => Err(SNAP_INVALID_IMAGE.to_string()),
    }
}

/// Release the layer's accumulation.
///
/// Releasing an accumulation that is not there is not an error.
fn term_exposure(ctx: &CompatCtx, id: u32) -> Result<(), String> {
    release_gone(ctx, id);
    exposure_state(ctx).layers.remove(&id);
    Ok(())
}

// ---------------------------------------------------------------------------
// The `Layer` members
// ---------------------------------------------------------------------------

extern "C" fn layer_init_exposure(
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
    match init_exposure(&ctx, id) {
        Ok(()) => {
            set_void(out);
            0
        }
        Err(error) => report_error(out_error, &error),
    }
}

extern "C" fn layer_snap_exposure(
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
    match snap_exposure(&ctx, id) {
        Ok(()) => {
            set_void(out);
            0
        }
        Err(error) => report_error(out_error, &error),
    }
}

extern "C" fn layer_term_exposure(
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
    match term_exposure(&ctx, id) {
        Ok(()) => {
            set_void(out);
            0
        }
        Err(error) => report_error(out_error, &error),
    }
}

/// The `Layer`-instance method definitions.
pub(super) fn method_defs() -> Vec<NativeObjThisMethodDef> {
    vec![
        NativeObjThisMethodDef {
            name: "initExposure",
            f: layer_init_exposure,
        },
        NativeObjThisMethodDef {
            name: "snapExposure",
            f: layer_snap_exposure,
        },
        NativeObjThisMethodDef {
            name: "termExposure",
            f: layer_term_exposure,
        },
    ]
}
