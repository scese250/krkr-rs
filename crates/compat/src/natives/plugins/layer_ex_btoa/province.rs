//! The three members that work on the layer's province plane:
//! `copyAlphaToProvince`, `fillByProvince` and `fillToProvince`.
//!
//! The province plane is a one-byte-per-pixel plane beside the image. Two of
//! the members write into it and one reads it: a plane that is written is
//! created on demand, while a plane that is read must already be there.

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeObjThisMethodDef, Value};

use crate::engine_api::Scene;

use super::{
    bound, callback_args, done, fail, integer_value, layer_clip, layer_image, low_byte,
    mark_modified, mark_region, optional_argument, pixel_offset, plane_offset, receiver_id,
    request_refresh, scene_mut, set_integer_array_out, word_to_rgba,
};

/// The message a member raises when the layer has no usable image.
const MUST_BE_SRC: &str = "src must be Layer.";
/// The message `fillByProvince` raises when the layer has no usable image.
const MUST_BE_LAYER: &str = "must be Layer.";
/// The message raised when the province plane cannot be created.
const NO_PROVINCE_IMAGE: &str = "dst has no province image.";
/// The message raised when the province plane has no row pitch.
const NO_PROVINCE_PITCH: &str = "dst has no province pitch.";
/// The message `fillByProvince` raises when the province plane is absent.
const NO_PLANE: &str = "no province image.";
/// The message `fillByProvince` raises when the province plane has no pitch.
const NO_PLANE_PITCH: &str = "no province pitch.";

/// The `Layer` method definitions of this group.
pub(super) fn method_defs() -> Vec<NativeObjThisMethodDef> {
    vec![
        NativeObjThisMethodDef {
            name: "copyAlphaToProvince",
            f: layer_copy_alpha_to_province,
        },
        NativeObjThisMethodDef {
            name: "fillByProvince",
            f: layer_fill_by_province,
        },
        NativeObjThisMethodDef {
            name: "fillToProvince",
            f: layer_fill_to_province,
        },
    ]
}

/// How the province byte of a pixel is decided.
#[derive(Clone, Copy)]
enum ProvinceMode {
    /// The alpha byte is copied into the province plane as it is.
    Verbatim,
    /// The alpha byte is compared with the threshold, and the plane takes the
    /// matched byte or the other one.
    Thresholded {
        /// The threshold the alpha byte is compared with.
        threshold: u8,
        /// The byte a pixel at or above the threshold takes.
        matched: Option<u8>,
        /// The byte a pixel below the threshold takes.
        otherwise: Option<u8>,
    },
    /// Every pixel takes the other byte.
    AllOtherwise(Option<u8>),
}

/// The value as a byte, when it names one.
fn byte_value(value: i64) -> Option<u8> {
    u8::try_from(value).ok()
}

/// Copy the layer's own alpha channel into its own province plane.
///
/// The threshold decides what the plane receives: without one the alpha byte
/// is copied as it is, with one the pixels at or above the threshold take the
/// matched byte and the pixels below it take the other one, and with a
/// threshold above the byte range every pixel takes the other byte.
fn copy_alpha_to_province(
    scene: &mut Scene,
    id: u32,
    threshold: Option<i64>,
    matched: i64,
    otherwise: i64,
) -> Result<(), String> {
    let (bitmap_id, width, _height) =
        layer_image(scene, id).map_err(|_| MUST_BE_SRC.to_string())?;
    let (left, top, right, bottom) = layer_clip(scene, id);

    // The plane is created on demand: the member writes into it.
    scene.allocate_province_image(id);
    let plane_width = scene.layer(id).map_or(0, |layer| layer.province_width);
    let has_plane = scene
        .layer(id)
        .is_some_and(|layer| layer.province.is_some());
    if !has_plane {
        return Err(NO_PROVINCE_IMAGE.to_string());
    }
    if plane_width == 0 {
        return Err(NO_PROVINCE_PITCH.to_string());
    }

    // A threshold below zero names no threshold at all, so the alpha byte is
    // copied as it is; a threshold above the byte range names no pixel, so
    // every pixel takes the other byte.
    let mode = match threshold {
        None => ProvinceMode::Verbatim,
        Some(value) if value < 0 => ProvinceMode::Verbatim,
        Some(value) if value <= 255 => ProvinceMode::Thresholded {
            threshold: value as u8,
            matched: byte_value(matched),
            otherwise: byte_value(otherwise),
        },
        Some(_) => ProvinceMode::AllOtherwise(byte_value(otherwise)),
    };
    let writes = match mode {
        ProvinceMode::Verbatim => true,
        ProvinceMode::Thresholded {
            matched, otherwise, ..
        } => matched.is_some() || otherwise.is_some(),
        ProvinceMode::AllOtherwise(otherwise) => otherwise.is_some(),
    };
    if !writes {
        // Nothing is written, and no refresh is asked for.
        return Ok(());
    }

    // The image and the plane are separate objects, so the bytes the plane
    // receives are decided while the image is read and written afterwards.
    let mut writes_at: Vec<(i64, i64, u8)> = Vec::new();
    {
        let bitmap = scene
            .bitmap(bitmap_id)
            .ok_or_else(|| "the layer's image no longer exists".to_string())?;
        for y in top..bottom {
            for x in left..right {
                let Some(offset) = pixel_offset(width, x, y) else {
                    continue;
                };
                let Some(alpha) = bitmap.rgba.get(offset + 3).copied() else {
                    continue;
                };
                let value = match mode {
                    ProvinceMode::Verbatim => Some(alpha),
                    ProvinceMode::Thresholded {
                        threshold,
                        matched,
                        otherwise,
                    } => {
                        if alpha >= threshold {
                            matched
                        } else {
                            otherwise
                        }
                    }
                    ProvinceMode::AllOtherwise(otherwise) => otherwise,
                };
                if let Some(value) = value {
                    writes_at.push((x, y, value));
                }
            }
        }
    }
    {
        let Some(layer) = scene.layer_mut(id) else {
            return Err(NO_PROVINCE_IMAGE.to_string());
        };
        let Some(plane) = layer.province.as_deref_mut() else {
            return Err(NO_PROVINCE_IMAGE.to_string());
        };
        for (x, y, value) in &writes_at {
            let Some(offset) = plane_offset(plane_width, *x, *y) else {
                continue;
            };
            let Some(slot) = plane.get_mut(offset) else {
                continue;
            };
            *slot = *value;
        }
        layer.image_modified = true;
    }
    let changed = super::region(left, top, right, bottom);
    mark_region(scene, bitmap_id, changed);
    mark_modified(scene, id);
    request_refresh(scene, id);
    Ok(())
}

/// Fill the pixels whose province byte carries the given index.
fn fill_by_province(scene: &mut Scene, id: u32, index: u8, color: [u8; 4]) -> Result<(), String> {
    let (bitmap_id, width, _height) =
        layer_image(scene, id).map_err(|_| MUST_BE_LAYER.to_string())?;
    let (left, top, right, bottom) = layer_clip(scene, id);

    // The plane is read here and not created: a layer without one has
    // nothing to match.
    let matched = {
        let Some(layer) = scene.layer(id) else {
            return Err(NO_PLANE.to_string());
        };
        let Some(plane) = layer.province.as_deref() else {
            return Err(NO_PLANE.to_string());
        };
        let plane_width = layer.province_width;
        if plane_width == 0 {
            return Err(NO_PLANE_PITCH.to_string());
        }
        let mut matched = Vec::new();
        for y in top..bottom {
            for x in left..right {
                let Some(offset) = plane_offset(plane_width, x, y) else {
                    continue;
                };
                if plane.get(offset) == Some(&index) {
                    matched.push((x, y));
                }
            }
        }
        matched
    };
    {
        let bitmap = scene
            .bitmap_mut(bitmap_id)
            .ok_or_else(|| "the layer's image no longer exists".to_string())?;
        for (x, y) in &matched {
            let Some(offset) = pixel_offset(width, *x, *y) else {
                continue;
            };
            let Some(slot) = bitmap.rgba.get_mut(offset..offset + 4) else {
                continue;
            };
            slot.copy_from_slice(&color);
        }
    }
    let changed = super::region(left, top, right, bottom);
    mark_region(scene, bitmap_id, changed);
    mark_modified(scene, id);
    request_refresh(scene, id);
    Ok(())
}

/// Mark the pixels of a colour in the province plane.
///
/// The colour is compared on its red, green and blue bytes; the pixel's own
/// alpha has to reach the threshold for the pixel to match. The bounding box
/// of the matched pixels is answered when at least one pixel matched.
fn fill_to_province(
    scene: &mut Scene,
    id: u32,
    color: u32,
    index: u8,
    threshold: i64,
) -> Result<Option<[i64; 4]>, String> {
    let (bitmap_id, width, _height) =
        layer_image(scene, id).map_err(|_| MUST_BE_SRC.to_string())?;
    let (left, top, right, bottom) = layer_clip(scene, id);

    // The plane is created on demand: the member writes into it.
    scene.allocate_province_image(id);
    let plane_width = scene.layer(id).map_or(0, |layer| layer.province_width);
    let has_plane = scene
        .layer(id)
        .is_some_and(|layer| layer.province.is_some());
    if !has_plane {
        return Err(NO_PROVINCE_IMAGE.to_string());
    }
    if plane_width == 0 {
        return Err(NO_PROVINCE_PITCH.to_string());
    }

    let wanted = word_to_rgba(color);
    // A threshold outside the byte range names no pixel, so every pixel of
    // the colour matches.
    let threshold_in_range = (0..=255).contains(&threshold);
    // The clamped threshold is in the byte range, so the cast keeps every bit
    // of it.
    let threshold_byte = threshold.clamp(0, 255) as u8;

    // The image and the plane are separate objects, so the matched pixels are
    // found while the image is read and marked afterwards.
    let mut matched: Vec<(i64, i64)> = Vec::new();
    let mut bounds: Option<(i64, i64, i64, i64)> = None;
    {
        let bitmap = scene
            .bitmap(bitmap_id)
            .ok_or_else(|| "the layer's image no longer exists".to_string())?;
        for y in top..bottom {
            for x in left..right {
                let Some(offset) = pixel_offset(width, x, y) else {
                    continue;
                };
                let Some(pixel) = bitmap.rgba.get(offset..offset + 4) else {
                    continue;
                };
                if pixel[0] != wanted[0] || pixel[1] != wanted[1] || pixel[2] != wanted[2] {
                    continue;
                }
                if threshold_in_range && pixel[3] < threshold_byte {
                    continue;
                }
                matched.push((x, y));
                bounds = Some(match bounds {
                    None => (x, y, x, y),
                    Some((min_x, min_y, max_x, max_y)) => {
                        (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                    }
                });
            }
        }
    }
    {
        let Some(layer) = scene.layer_mut(id) else {
            return Err(NO_PROVINCE_IMAGE.to_string());
        };
        let Some(plane) = layer.province.as_deref_mut() else {
            return Err(NO_PROVINCE_IMAGE.to_string());
        };
        for (x, y) in &matched {
            let Some(offset) = plane_offset(plane_width, *x, *y) else {
                continue;
            };
            let Some(slot) = plane.get_mut(offset) else {
                continue;
            };
            *slot = index;
        }
        layer.image_modified = true;
    }
    let changed = super::region(left, top, right, bottom);
    mark_region(scene, bitmap_id, changed);
    mark_modified(scene, id);
    request_refresh(scene, id);
    Ok(bounds
        .map(|(min_x, min_y, max_x, max_y)| [min_x, min_y, max_x - min_x + 1, max_y - min_y + 1]))
}

/// `Layer.copyAlphaToProvince(threshold, matched, otherwise)`.
extern "C" fn layer_copy_alpha_to_province(
    engine: *mut c_void,
    objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let Some((resolved, ctx)) = bound(engine) else {
        return fail(out_error, "layerExBTOA: compat context is not bound");
    };
    let id = match receiver_id(resolved.engine(), objthis, MUST_BE_SRC) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    let arguments = callback_args(argv, argc);
    let threshold = match optional_argument(arguments, 0) {
        Some(value) => match integer_value(value) {
            Ok(value) => Some(value),
            Err(error) => return fail(out_error, &error),
        },
        None => None,
    };
    let matched = match optional_argument(arguments, 1) {
        Some(value) => match integer_value(value) {
            Ok(value) => value,
            Err(error) => return fail(out_error, &error),
        },
        None => 1,
    };
    let otherwise = match optional_argument(arguments, 2) {
        Some(value) => match integer_value(value) {
            Ok(value) => value,
            Err(error) => return fail(out_error, &error),
        },
        None => 0,
    };
    let result = scene_mut(&ctx)
        .and_then(|scene| copy_alpha_to_province(scene, id, threshold, matched, otherwise));
    match result {
        Ok(()) => done(out),
        Err(error) => fail(out_error, &error),
    }
}

/// `Layer.fillByProvince(index, color)`.
extern "C" fn layer_fill_by_province(
    engine: *mut c_void,
    objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
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
    let arguments = callback_args(argv, argc);
    if arguments.len() < 2 {
        return fail(out_error, "Invalid argument count");
    }
    let index = match integer_value(&arguments[0]) {
        Ok(value) => value,
        Err(error) => return fail(out_error, &error),
    };
    let color = match integer_value(&arguments[1]) {
        Ok(value) => value,
        Err(error) => return fail(out_error, &error),
    };
    // The index names one plane byte, and the colour is a packed word.
    let index = low_byte(index);
    let color = word_to_rgba(color as u32);
    let result = scene_mut(&ctx).and_then(|scene| fill_by_province(scene, id, index, color));
    match result {
        Ok(()) => done(out),
        Err(error) => fail(out_error, &error),
    }
}

/// `Layer.fillToProvince(color, index, threshold)`.
extern "C" fn layer_fill_to_province(
    engine: *mut c_void,
    objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    let Some((resolved, ctx)) = bound(engine) else {
        return fail(out_error, "layerExBTOA: compat context is not bound");
    };
    let id = match receiver_id(resolved.engine(), objthis, MUST_BE_SRC) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    let arguments = callback_args(argv, argc);
    if arguments.len() < 2 {
        return fail(out_error, "Invalid argument count");
    }
    let color = match integer_value(&arguments[0]) {
        Ok(value) => value,
        Err(error) => return fail(out_error, &error),
    };
    let index = match integer_value(&arguments[1]) {
        Ok(value) => value,
        Err(error) => return fail(out_error, &error),
    };
    let threshold = match optional_argument(arguments, 2) {
        Some(value) => match integer_value(value) {
            Ok(value) => value,
            Err(error) => return fail(out_error, &error),
        },
        None => 64,
    };
    // The colour is a packed word and the index names one plane byte.
    let color = color as u32;
    let index = low_byte(index);
    let result =
        scene_mut(&ctx).and_then(|scene| fill_to_province(scene, id, color, index, threshold));
    match result {
        Ok(Some(bounds)) => match set_integer_array_out(resolved.engine(), out, &bounds) {
            Ok(()) => 0,
            Err(error) => fail(out_error, &error),
        },
        Ok(None) => done(out),
        Err(error) => fail(out_error, &error),
    }
}
