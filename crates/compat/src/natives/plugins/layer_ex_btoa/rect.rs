//! The two members that copy a rectangle from one layer to another:
//! `clipAlphaRect` and `overwrapRect`.
//!
//! Both take a rectangle in the destination's image coordinates, a source
//! layer and a rectangle in the source's image coordinates. The rectangle is
//! cut to the source's image and to the destination's clip rectangle, and
//! the two halves are moved together, so a source pixel keeps the partner the
//! script asked for.

use std::ffi::{c_char, c_int, c_void};

use tjs::{NativeObjThisMethodDef, Value};

use crate::engine_api::Scene;

use super::{
    bound, byte_option, callback_args, coordinate, done, fail, integer_value, is_add_alpha,
    layer_argument, layer_clip, layer_image, mark_modified, mark_region, optional_argument,
    pixel_offset, receiver_id, request_refresh, scale_channel, scale_pixel, scene_mut,
};

/// The message raised when the destination has no usable image.
const MUST_BE_DEST: &str = "dest must be Layer.";
/// The message raised when the source has no usable image.
const MUST_BE_SRC: &str = "src must be Layer.";

/// The `Layer` method definitions of this group.
pub(super) fn method_defs() -> Vec<NativeObjThisMethodDef> {
    vec![
        NativeObjThisMethodDef {
            name: "clipAlphaRect",
            f: layer_clip_alpha_rect,
        },
        NativeObjThisMethodDef {
            name: "overwrapRect",
            f: layer_overwrap_rect,
        },
    ]
}

/// The rectangle of a copy that survived both cuts.
struct Surviving {
    /// The destination rectangle, in the destination's image coordinates.
    dest: (i64, i64, i64, i64),
    /// The source rectangle, in the source's image coordinates.
    source: (i64, i64, i64, i64),
}

/// The rectangle a copy covers, after both cuts.
///
/// The source rectangle is cut to the source image first, and the destination
/// origin moves by the amount the source's left and top edges were cut by,
/// so the surviving source pixels keep the destination pixels the script
/// asked for. The destination rectangle is then cut to the destination's clip
/// rectangle, and the source origin moves by the same amount. `None` means
/// that nothing survived.
fn intersect_rects(
    dest_left: i64,
    dest_top: i64,
    source_left: i64,
    source_top: i64,
    width: i64,
    height: i64,
    source_size: (i64, i64),
    clip: (i64, i64, i64, i64),
) -> Option<Surviving> {
    let mut dest_x = dest_left;
    let mut dest_y = dest_top;
    let mut source_x = source_left;
    let mut source_y = source_top;
    let mut w = width;
    let mut h = height;

    if source_x < 0 {
        dest_x -= source_x;
        w += source_x;
        source_x = 0;
    }
    if source_x + w > source_size.0 {
        w = source_size.0 - source_x;
    }
    if source_y < 0 {
        dest_y -= source_y;
        h += source_y;
        source_y = 0;
    }
    if source_y + h > source_size.1 {
        h = source_size.1 - source_y;
    }

    if dest_x < clip.0 {
        source_x -= clip.0 - dest_x;
        w -= clip.0 - dest_x;
        dest_x = clip.0;
    }
    if dest_x + w > clip.2 {
        w = clip.2 - dest_x;
    }
    if dest_y < clip.1 {
        source_y -= clip.1 - dest_y;
        h -= clip.1 - dest_y;
        dest_y = clip.1;
    }
    if dest_y + h > clip.3 {
        h = clip.3 - dest_y;
    }

    if w <= 0 || h <= 0 {
        return None;
    }
    Some(Surviving {
        dest: (dest_x, dest_y, dest_x + w, dest_y + h),
        source: (source_x, source_y, source_x + w, source_y + h),
    })
}

/// The source rectangle's pixels, copied out of the source's image.
///
/// The copy is what lets the destination be written while the source is
/// read: the two images are separate objects, and they may even be the same
/// one.
fn source_pixels(
    scene: &Scene,
    source_bitmap: u32,
    source_width: u32,
    rect: (i64, i64, i64, i64),
) -> Result<Vec<u8>, String> {
    let region_width = usize::try_from(rect.2 - rect.0)
        .map_err(|_| "the source rectangle is too wide".to_string())?;
    let region_height = usize::try_from(rect.3 - rect.1)
        .map_err(|_| "the source rectangle is too tall".to_string())?;
    let bytes = region_width
        .checked_mul(region_height)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| "the source rectangle is too large".to_string())?;
    let mut pixels = vec![0u8; bytes];
    let bitmap = scene
        .bitmap(source_bitmap)
        .ok_or_else(|| MUST_BE_SRC.to_string())?;
    for row in 0..region_height {
        for column in 0..region_width {
            let x = rect.0 + column as i64;
            let y = rect.1 + row as i64;
            let Some(offset) = pixel_offset(source_width, x, y) else {
                continue;
            };
            let Some(pixel) = bitmap.rgba.get(offset..offset + 4) else {
                continue;
            };
            let start = (row * region_width + column) * 4;
            pixels[start..start + 4].copy_from_slice(pixel);
        }
    }
    Ok(pixels)
}

/// Multiply the destination's alpha by the source's alpha over a rectangle.
///
/// A destination that stores an additive (premultiplied) alpha has all four
/// of its channels scaled instead, the alpha included. The clear value, when
/// one was given, writes over the part of the destination's clip rectangle
/// the rectangle does not cover.
fn clip_alpha_rect(
    scene: &mut Scene,
    dest_id: u32,
    source_id: u32,
    dest_left: i64,
    dest_top: i64,
    source_left: i64,
    source_top: i64,
    width: i64,
    height: i64,
    clear: Option<u8>,
) -> Result<(), String> {
    // The destination is inspected before the source.
    let (dest_bitmap, dest_width, _dest_height) =
        layer_image(scene, dest_id).map_err(|_| MUST_BE_DEST.to_string())?;
    let (source_bitmap, source_width, source_height) =
        layer_image(scene, source_id).map_err(|_| MUST_BE_SRC.to_string())?;
    let clip = layer_clip(scene, dest_id);
    let add_alpha = is_add_alpha(scene, dest_id);

    let surviving = intersect_rects(
        dest_left,
        dest_top,
        source_left,
        source_top,
        width,
        height,
        (i64::from(source_width), i64::from(source_height)),
        clip,
    );
    let Some(rect) = surviving else {
        // Nothing to draw: the clear value, when one was given, reaches every
        // pixel of the clip rectangle, and the colour bytes are untouched.
        if let Some(clear) = clear {
            {
                let bitmap = scene
                    .bitmap_mut(dest_bitmap)
                    .ok_or_else(|| MUST_BE_DEST.to_string())?;
                for y in clip.1..clip.3 {
                    for x in clip.0..clip.2 {
                        let Some(offset) = pixel_offset(dest_width, x, y) else {
                            continue;
                        };
                        let Some(pixel) = bitmap.rgba.get_mut(offset..offset + 4) else {
                            continue;
                        };
                        pixel[3] = clear;
                    }
                }
            }
            mark_region(
                scene,
                dest_bitmap,
                super::region(clip.0, clip.1, clip.2, clip.3),
            );
            mark_modified(scene, dest_id);
        }
        return Ok(());
    };

    let pixels = source_pixels(scene, source_bitmap, source_width, rect.source)?;
    let region_width = usize::try_from(rect.dest.2 - rect.dest.0)
        .map_err(|_| "the rectangle is too wide".to_string())?;
    let region_height = usize::try_from(rect.dest.3 - rect.dest.1)
        .map_err(|_| "the rectangle is too tall".to_string())?;
    {
        let bitmap = scene
            .bitmap_mut(dest_bitmap)
            .ok_or_else(|| MUST_BE_DEST.to_string())?;
        for row in 0..region_height {
            for column in 0..region_width {
                let x = rect.dest.0 + column as i64;
                let y = rect.dest.1 + row as i64;
                let Some(offset) = pixel_offset(dest_width, x, y) else {
                    continue;
                };
                let Some(pixel) = bitmap.rgba.get_mut(offset..offset + 4) else {
                    continue;
                };
                let start = (row * region_width + column) * 4;
                let source_alpha = pixels[start + 3];
                if add_alpha {
                    let current = [pixel[0], pixel[1], pixel[2], pixel[3]];
                    pixel.copy_from_slice(&scale_pixel(current, source_alpha));
                } else {
                    pixel[3] = scale_channel(pixel[3], source_alpha);
                }
            }
        }
        if let Some(clear) = clear {
            for y in clip.1..clip.3 {
                for x in clip.0..clip.2 {
                    if x >= rect.dest.0 && x < rect.dest.2 && y >= rect.dest.1 && y < rect.dest.3 {
                        continue;
                    }
                    let Some(offset) = pixel_offset(dest_width, x, y) else {
                        continue;
                    };
                    let Some(pixel) = bitmap.rgba.get_mut(offset..offset + 4) else {
                        continue;
                    };
                    if add_alpha {
                        match clear {
                            // A clear of zero empties the pixel, and a clear
                            // of 255 leaves it alone.
                            0 => pixel.fill(0),
                            255 => {}
                            value => {
                                let current = [pixel[0], pixel[1], pixel[2], pixel[3]];
                                pixel.copy_from_slice(&scale_pixel(current, value));
                            }
                        }
                    } else {
                        pixel[3] = clear;
                    }
                }
            }
        }
    }
    let changed = match clear {
        Some(_) => super::region(clip.0, clip.1, clip.2, clip.3),
        None => super::region(rect.dest.0, rect.dest.1, rect.dest.2, rect.dest.3),
    };
    mark_region(scene, dest_bitmap, changed);
    mark_modified(scene, dest_id);
    request_refresh(scene, dest_id);
    Ok(())
}

/// Lay the source over the destination, skipping the source pixels whose
/// alpha is below the threshold.
///
/// The source pixel is copied in full, not blended, and the member asks for
/// no refresh: the layer's pixels are written and the member returns.
fn overwrap_rect(
    scene: &mut Scene,
    dest_id: u32,
    source_id: u32,
    dest_left: i64,
    dest_top: i64,
    source_left: i64,
    source_top: i64,
    width: i64,
    height: i64,
    threshold: u8,
) -> Result<(), String> {
    // The destination is inspected before the source.
    let (dest_bitmap, dest_width, _dest_height) =
        layer_image(scene, dest_id).map_err(|_| MUST_BE_DEST.to_string())?;
    let (source_bitmap, source_width, source_height) =
        layer_image(scene, source_id).map_err(|_| MUST_BE_SRC.to_string())?;
    let clip = layer_clip(scene, dest_id);

    let Some(rect) = intersect_rects(
        dest_left,
        dest_top,
        source_left,
        source_top,
        width,
        height,
        (i64::from(source_width), i64::from(source_height)),
        clip,
    ) else {
        // Nothing to draw: the member returns without touching anything.
        return Ok(());
    };

    let pixels = source_pixels(scene, source_bitmap, source_width, rect.source)?;
    let region_width = usize::try_from(rect.dest.2 - rect.dest.0)
        .map_err(|_| "the rectangle is too wide".to_string())?;
    let region_height = usize::try_from(rect.dest.3 - rect.dest.1)
        .map_err(|_| "the rectangle is too tall".to_string())?;
    {
        let bitmap = scene
            .bitmap_mut(dest_bitmap)
            .ok_or_else(|| MUST_BE_DEST.to_string())?;
        for row in 0..region_height {
            for column in 0..region_width {
                let start = (row * region_width + column) * 4;
                if pixels[start + 3] < threshold {
                    continue;
                }
                let x = rect.dest.0 + column as i64;
                let y = rect.dest.1 + row as i64;
                let Some(offset) = pixel_offset(dest_width, x, y) else {
                    continue;
                };
                let Some(pixel) = bitmap.rgba.get_mut(offset..offset + 4) else {
                    continue;
                };
                pixel.copy_from_slice(&pixels[start..start + 4]);
            }
        }
    }
    let changed = super::region(rect.dest.0, rect.dest.1, rect.dest.2, rect.dest.3);
    mark_region(scene, dest_bitmap, changed);
    mark_modified(scene, dest_id);
    Ok(())
}

/// `Layer.clipAlphaRect(dleft, dtop, src, sleft, stop, swidth, sheight,
/// clear)`.
extern "C" fn layer_clip_alpha_rect(
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
    let id = match receiver_id(resolved.engine(), objthis, MUST_BE_DEST) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    let arguments = callback_args(argv, argc);
    if arguments.len() < 7 {
        return fail(out_error, "Invalid argument count");
    }
    let source = match layer_argument(resolved.engine(), &arguments[2], MUST_BE_SRC) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    let clear = match optional_argument(arguments, 7) {
        Some(value) => match integer_value(value) {
            Ok(value) => byte_option(value),
            Err(error) => return fail(out_error, &error),
        },
        None => None,
    };
    let mut numbers = [0i64; 6];
    for (slot, value) in numbers.iter_mut().zip([
        &arguments[0],
        &arguments[1],
        &arguments[3],
        &arguments[4],
        &arguments[5],
        &arguments[6],
    ]) {
        match integer_value(value) {
            Ok(value) => *slot = coordinate(value),
            Err(error) => return fail(out_error, &error),
        }
    }
    let (dest_left, dest_top, source_left, source_top, width, height) = numbers;
    if width <= 0 || height <= 0 {
        return fail(out_error, "Invalid argument");
    }
    let result = scene_mut(&ctx).and_then(|scene| {
        clip_alpha_rect(
            scene,
            id,
            source,
            dest_left,
            dest_top,
            source_left,
            source_top,
            width,
            height,
            clear,
        )
    });
    match result {
        Ok(()) => done(out),
        Err(error) => fail(out_error, &error),
    }
}

/// `Layer.overwrapRect(dleft, dtop, src, sleft, stop, swidth, sheight,
/// threshold)`.
extern "C" fn layer_overwrap_rect(
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
    let id = match receiver_id(resolved.engine(), objthis, MUST_BE_DEST) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    let arguments = callback_args(argv, argc);
    if arguments.len() < 7 {
        return fail(out_error, "Invalid argument count");
    }
    let source = match layer_argument(resolved.engine(), &arguments[2], MUST_BE_SRC) {
        Ok(id) => id,
        Err(error) => return fail(out_error, &error),
    };
    // A threshold outside the byte range leaves the default in place; the
    // feature is not switched off by it.
    let threshold = match optional_argument(arguments, 7) {
        Some(value) => match integer_value(value) {
            Ok(value) => match byte_option(value) {
                Some(value) => value,
                None => 1,
            },
            Err(error) => return fail(out_error, &error),
        },
        None => 1,
    };
    let mut numbers = [0i64; 6];
    for (slot, value) in numbers.iter_mut().zip([
        &arguments[0],
        &arguments[1],
        &arguments[3],
        &arguments[4],
        &arguments[5],
        &arguments[6],
    ]) {
        match integer_value(value) {
            Ok(value) => *slot = coordinate(value),
            Err(error) => return fail(out_error, &error),
        }
    }
    let (dest_left, dest_top, source_left, source_top, width, height) = numbers;
    if width <= 0 || height <= 0 {
        return fail(out_error, "Invalid argument");
    }
    let result = scene_mut(&ctx).and_then(|scene| {
        overwrap_rect(
            scene,
            id,
            source,
            dest_left,
            dest_top,
            source_left,
            source_top,
            width,
            height,
            threshold,
        )
    });
    match result {
        Ok(()) => done(out),
        Err(error) => fail(out_error, &error),
    }
}
