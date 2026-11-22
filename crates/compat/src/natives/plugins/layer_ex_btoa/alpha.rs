//! The three members that move an alpha channel inside the layer's own
//! image: `copyRightBlueToLeftAlpha`, `copyBottomBlueToTopAlpha` and
//! `fillAlpha`.
//!
//! The two copies move the blue byte of one half of the image into the alpha
//! byte of the other half, and neither of them looks at the clip rectangle:
//! the whole image takes part. `fillAlpha` works inside the clip rectangle
//! and only touches the alpha byte.

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeObjThisMethodDef, Value};

use crate::engine_api::Scene;

use super::{
    bound, done, fail, layer_clip, layer_image, mark_modified, mark_region, pixel_offset,
    receiver_id, region, request_refresh, scene_mut,
};

/// The message a member raises when the layer has no usable image.
const MUST_BE_LAYER: &str = "dest must be Layer.";

/// The `Layer` method definitions of this group.
pub(super) fn method_defs() -> Vec<NativeObjThisMethodDef> {
    vec![
        NativeObjThisMethodDef {
            name: "copyRightBlueToLeftAlpha",
            f: layer_copy_right_blue_to_left_alpha,
        },
        NativeObjThisMethodDef {
            name: "copyBottomBlueToTopAlpha",
            f: layer_copy_bottom_blue_to_top_alpha,
        },
        NativeObjThisMethodDef {
            name: "fillAlpha",
            f: layer_fill_alpha,
        },
    ]
}

/// Move the blue byte of the image's right half into the alpha byte of its
/// left half.
///
/// The halves meet in the middle: the left half holds `width / 2` columns and
/// the right half starts right after it. An odd width leaves the last column
/// of the image alone, because the halves do not meet there.
fn copy_right_blue_to_left_alpha(scene: &mut Scene, id: u32) -> Result<(), String> {
    let (bitmap_id, width, height) =
        layer_image(scene, id).map_err(|_| MUST_BE_LAYER.to_string())?;
    let half = width / 2;
    {
        let bitmap = scene
            .bitmap_mut(bitmap_id)
            .ok_or_else(|| MUST_BE_LAYER.to_string())?;
        for y in 0..i64::from(height) {
            for x in 0..i64::from(half) {
                let Some(source) = pixel_offset(width, x + i64::from(half), y) else {
                    continue;
                };
                let Some(dest) = pixel_offset(width, x, y) else {
                    continue;
                };
                let Some(blue) = bitmap.rgba.get(source + 2).copied() else {
                    continue;
                };
                let Some(alpha) = bitmap.rgba.get_mut(dest + 3) else {
                    continue;
                };
                *alpha = blue;
            }
        }
    }
    mark_region(
        scene,
        bitmap_id,
        region(0, 0, i64::from(half), i64::from(height)),
    );
    mark_modified(scene, id);
    request_refresh(scene, id);
    Ok(())
}

/// Move the blue byte of the image's bottom half into the alpha byte of its
/// top half.
///
/// The halves meet in the middle: the top half holds `height / 2` rows and
/// the bottom half starts right after it. An odd height leaves the last row
/// of the image alone.
fn copy_bottom_blue_to_top_alpha(scene: &mut Scene, id: u32) -> Result<(), String> {
    let (bitmap_id, width, height) =
        layer_image(scene, id).map_err(|_| MUST_BE_LAYER.to_string())?;
    let half = height / 2;
    {
        let bitmap = scene
            .bitmap_mut(bitmap_id)
            .ok_or_else(|| MUST_BE_LAYER.to_string())?;
        for y in 0..i64::from(half) {
            for x in 0..i64::from(width) {
                let Some(source) = pixel_offset(width, x, y + i64::from(half)) else {
                    continue;
                };
                let Some(dest) = pixel_offset(width, x, y) else {
                    continue;
                };
                let Some(blue) = bitmap.rgba.get(source + 2).copied() else {
                    continue;
                };
                let Some(alpha) = bitmap.rgba.get_mut(dest + 3) else {
                    continue;
                };
                *alpha = blue;
            }
        }
    }
    mark_region(
        scene,
        bitmap_id,
        region(0, 0, i64::from(width), i64::from(half)),
    );
    mark_modified(scene, id);
    request_refresh(scene, id);
    Ok(())
}

/// Make the alpha channel opaque over the clip rectangle.
///
/// Only the alpha byte is written: the colour bytes keep their values.
fn fill_alpha(scene: &mut Scene, id: u32) -> Result<(), String> {
    let (bitmap_id, width, _height) =
        layer_image(scene, id).map_err(|_| MUST_BE_LAYER.to_string())?;
    let (left, top, right, bottom) = layer_clip(scene, id);
    {
        let bitmap = scene
            .bitmap_mut(bitmap_id)
            .ok_or_else(|| MUST_BE_LAYER.to_string())?;
        for y in top..bottom {
            for x in left..right {
                let Some(offset) = pixel_offset(width, x, y) else {
                    continue;
                };
                let Some(alpha) = bitmap.rgba.get_mut(offset + 3) else {
                    continue;
                };
                *alpha = 0xff;
            }
        }
    }
    mark_region(scene, bitmap_id, region(left, top, right, bottom));
    mark_modified(scene, id);
    request_refresh(scene, id);
    Ok(())
}

/// `Layer.copyRightBlueToLeftAlpha()`.
extern "C" fn layer_copy_right_blue_to_left_alpha(
    engine: *mut c_void,
    objthis: *mut c_void,
    _argc: c_int,
    _argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let Some((resolved, ctx)) = bound(engine) else {
        return fail(out_error, "layerExBTOA: compat context is not bound");
    };
    let id = match receiver_id(resolved.engine(), objthis, MUST_BE_LAYER) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    let result = scene_mut(&ctx).and_then(|scene| copy_right_blue_to_left_alpha(scene, id));
    match result {
        Ok(()) => done(out),
        Err(error) => fail(out_error, &error),
    }
}

/// `Layer.copyBottomBlueToTopAlpha()`.
extern "C" fn layer_copy_bottom_blue_to_top_alpha(
    engine: *mut c_void,
    objthis: *mut c_void,
    _argc: c_int,
    _argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let Some((resolved, ctx)) = bound(engine) else {
        return fail(out_error, "layerExBTOA: compat context is not bound");
    };
    let id = match receiver_id(resolved.engine(), objthis, MUST_BE_LAYER) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    let result = scene_mut(&ctx).and_then(|scene| copy_bottom_blue_to_top_alpha(scene, id));
    match result {
        Ok(()) => done(out),
        Err(error) => fail(out_error, &error),
    }
}

/// `Layer.fillAlpha()`.
extern "C" fn layer_fill_alpha(
    engine: *mut c_void,
    objthis: *mut c_void,
    _argc: c_int,
    _argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let Some((resolved, ctx)) = bound(engine) else {
        return fail(out_error, "layerExBTOA: compat context is not bound");
    };
    let id = match receiver_id(resolved.engine(), objthis, MUST_BE_LAYER) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    let result = scene_mut(&ctx).and_then(|scene| fill_alpha(scene, id));
    match result {
        Ok(()) => done(out),
        Err(error) => fail(out_error, &error),
    }
}
