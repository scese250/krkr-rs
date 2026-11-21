//! `Layer.shrinkCopyFast`: the box average.
//!
//! The source is averaged over boxes of `shrink_x` by `shrink_y` pixels and
//! the destination layer is resized to the size of the result: the width
//! becomes the source's width divided by `shrink_x` (rounded up) and the
//! height its height divided by `shrink_y` (rounded up). The pixels that do
//! not fill a whole box form one more box of their own.
//!
//! The alpha is not read: every destination pixel comes out opaque.

use crate::CompatCtx;
use crate::engine_api::BitmapState;

use super::scene_mut;

/// Copy the source into the destination, averaging every box of
/// `step_x` by `step_y` source pixels.
///
/// The destination is resized to the size of the result first.
///
/// # Errors
///
/// Returns an error when a shrink factor is not positive, when a layer has
/// no main image, when the destination cannot be resized, or when the copy
/// cannot read the source it was given.
pub(super) fn shrink_copy_fast(
    ctx: &CompatCtx,
    dst_id: u32,
    src_id: u32,
    step_x: i64,
    step_y: i64,
) -> Result<(), String> {
    // A missing or zero vertical factor repeats the horizontal one.
    let step_y = if step_y == 0 { step_x } else { step_y };
    if step_x <= 0 || step_y <= 0 {
        return Err("Layer.shrinkCopyFast: the shrink factors must be at least 1".to_string());
    }
    let scene = scene_mut(ctx)?;
    let src_bitmap = super::layer_bitmap(scene, src_id, "source")?;
    let dst_bitmap = super::layer_bitmap(scene, dst_id, "destination")?;
    // The image may still be decoding: the copy reads its pixels.
    scene.resolve_bitmap(src_bitmap);
    let source_size = scene
        .bitmap(src_bitmap)
        .map(|bitmap| (i64::from(bitmap.width), i64::from(bitmap.height)))
        .ok_or_else(|| "Layer.shrinkCopyFast: the source image does not exist".to_string())?;
    let (src_w, src_h) = source_size;
    // The destination takes the size of the result: the source's size
    // divided by the shrink factor, rounded up.
    let (Ok(src_w), Ok(src_h)) = (u64::try_from(src_w), u64::try_from(src_h)) else {
        return Err("Layer.shrinkCopyFast: the source size is out of range".to_string());
    };
    let (Ok(step_x), Ok(step_y)) = (u64::try_from(step_x), u64::try_from(step_y)) else {
        return Err("Layer.shrinkCopyFast: the shrink factor is out of range".to_string());
    };
    let width = src_w.div_ceil(step_x);
    let height = src_h.div_ceil(step_y);
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err("Layer.shrinkCopyFast: the result does not fit an image".to_string());
    };
    // A copy of a layer onto itself reads the source before the resize
    // replaces it.
    let snapshot =
        if src_bitmap == dst_bitmap {
            Some(scene.bitmap(src_bitmap).cloned().ok_or_else(|| {
                "Layer.shrinkCopyFast: the source image does not exist".to_string()
            })?)
        } else {
            None
        };
    if !scene.internal_set_image_size(dst_id, width, height) {
        return Err("Layer.shrinkCopyFast: could not resize the destination image".to_string());
    }
    // The number of whole boxes in a row and the pixels that are left over.
    let boxes = src_w / step_x;
    let rest = src_w - boxes * step_x;
    let copied = match snapshot {
        Some(source) => {
            let Some(dest) = scene.bitmap_mut(dst_bitmap) else {
                return Err(
                    "Layer.shrinkCopyFast: the destination image does not exist".to_string()
                );
            };
            let result = copy_into(dest, &source, boxes, rest, step_x, step_y);
            dest.mark_dirty();
            Some(result)
        }
        None => scene.with_two_bitmaps(dst_bitmap, src_bitmap, |dest, source| {
            copy_into(dest, source, boxes, rest, step_x, step_y)
        }),
    };
    match copied {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(error),
        None => Err("Layer.shrinkCopyFast: the destination image does not exist".to_string()),
    }
}

/// Write the averaged boxes into the destination.
fn copy_into(
    dest: &mut BitmapState,
    source: &BitmapState,
    boxes: u64,
    rest: u64,
    step_x: u64,
    step_y: u64,
) -> Result<(), String> {
    let src_h = u64::from(source.height);
    let dst_h = u64::from(dest.height);
    for row in 0..dst_h {
        // The source rows this destination row covers.
        let first = row * step_y;
        let last = ((row + 1) * step_y).min(src_h);
        let mut shrunk = Vec::with_capacity((last - first) as usize);
        for source_row in first..last {
            shrunk.push(shrink_row(source, source_row, boxes, rest, step_x)?);
        }
        for (column, pixel) in average_rows(&shrunk)?.into_iter().enumerate() {
            write_pixel(dest, column as u64, row, pixel)?;
        }
    }
    // The copy rewrites the whole image, so the frame uploads all of it.
    dest.mark_dirty();
    Ok(())
}

/// The horizontally shrunk row: every box of `step_x` pixels becomes one
/// pixel, and the pixels that are left over become one more.
fn shrink_row(
    source: &BitmapState,
    row: u64,
    boxes: u64,
    rest: u64,
    step_x: u64,
) -> Result<Vec<[u8; 3]>, String> {
    let mut out = Vec::with_capacity((boxes + u64::from(rest > 0)) as usize);
    for box_index in 0..boxes {
        out.push(box_average(source, row, box_index * step_x, step_x)?);
    }
    if rest > 0 {
        out.push(box_average(source, row, boxes * step_x, rest)?);
    }
    Ok(out)
}

/// The average colour of a box of source pixels.
fn box_average(source: &BitmapState, row: u64, column: u64, count: u64) -> Result<[u8; 3], String> {
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for offset in 0..count {
        let pixel = pixel(source, column + offset, row).ok_or_else(|| {
            format!(
                "Layer.shrinkCopyFast: the source pixel ({}, {row}) is outside the source image",
                column + offset
            )
        })?;
        r += u64::from(pixel[0]);
        g += u64::from(pixel[1]);
        b += u64::from(pixel[2]);
    }
    let channel = |value: u64| {
        u8::try_from(value / count).map_err(|_| {
            "Layer.shrinkCopyFast: the averaged colour is outside a channel".to_string()
        })
    };
    Ok([channel(r)?, channel(g)?, channel(b)?])
}

/// The average of the shrunk rows: every column's colour is the average of
/// the rows' colours, and the alpha is opaque.
fn average_rows(rows: &[Vec<[u8; 3]>]) -> Result<Vec<[u8; 4]>, String> {
    let count = i64::try_from(rows.len().max(1)).map_err(|_| {
        "Layer.shrinkCopyFast: the number of averaged rows is out of range".to_string()
    })?;
    let width = rows.first().map_or(0, Vec::len);
    let mut sums = vec![[0i64; 3]; width];
    for row in rows {
        for (column, pixel) in row.iter().enumerate() {
            let Some(sum) = sums.get_mut(column) else {
                continue;
            };
            sum[0] += i64::from(pixel[0]);
            sum[1] += i64::from(pixel[1]);
            sum[2] += i64::from(pixel[2]);
        }
    }
    let mut out = Vec::with_capacity(width);
    for sum in sums {
        let channel = |value: i64| {
            u8::try_from(value / count).map_err(|_| {
                "Layer.shrinkCopyFast: the averaged colour is outside a channel".to_string()
            })
        };
        out.push([channel(sum[0])?, channel(sum[1])?, channel(sum[2])?, 255]);
    }
    Ok(out)
}

/// The source pixel at `(x, y)`.
fn pixel(source: &BitmapState, x: u64, y: u64) -> Option<[u8; 4]> {
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return None;
    };
    let offset = source.pixel_offset(x, y)?;
    let rgba = source.rgba.get(offset..offset + 4)?;
    Some([rgba[0], rgba[1], rgba[2], rgba[3]])
}

/// Write one destination pixel.
fn write_pixel(dest: &mut BitmapState, x: u64, y: u64, pixel: [u8; 4]) -> Result<(), String> {
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return Err("Layer.shrinkCopyFast: the destination pixel is out of range".to_string());
    };
    let offset = dest
        .pixel_offset(x, y)
        .ok_or_else(|| "Layer.shrinkCopyFast: the destination pixel is out of range".to_string())?;
    let target = dest
        .rgba
        .get_mut(offset..offset + 4)
        .ok_or_else(|| "Layer.shrinkCopyFast: the destination pixel is out of range".to_string())?;
    target.copy_from_slice(&pixel);
    Ok(())
}
