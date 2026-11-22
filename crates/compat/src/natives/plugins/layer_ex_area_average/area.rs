//! The area average of `Layer.stretchCopyAA`.
//!
//! The copy maps the destination rectangle onto the source rectangle and
//! averages the area every destination pixel covers. The lengths are held in
//! a signed fixed-point form with twelve fractional bits — one pixel is 4096
//! units — and every conversion truncates toward zero, so the result is the
//! plugin's own arithmetic and not an exact-rational average.
//!
//! The colour of a source pixel is weighted by the area it covers times its
//! alpha (the weight is the area scaled by `alpha / 256`), while the alpha
//! of the destination pixel is the area-weighted mean of the source alphas.
//! A source pixel that is fully transparent does not weigh on the colour at
//! all; it only weighs on the alpha, and it colours the result when no other
//! pixel does.

use api::geometry::PixelRegion;

use crate::engine_api::BitmapState;

/// The number of fixed-point units in one pixel.
const UNIT: i64 = 4096;

/// The number of fractional bits a stored length carries.
const FRACTION: u32 = 12;

/// The rectangles of one copy, in the two images' own pixels.
pub(super) struct Request {
    /// The destination rectangle's left edge.
    pub dest_left: i64,
    /// The destination rectangle's top edge.
    pub dest_top: i64,
    /// The destination rectangle's width.
    pub dest_width: i64,
    /// The destination rectangle's height.
    pub dest_height: i64,
    /// The source rectangle's left edge.
    pub src_left: i64,
    /// The source rectangle's top edge.
    pub src_top: i64,
    /// The source rectangle's width.
    pub src_width: i64,
    /// The source rectangle's height.
    pub src_height: i64,
}

/// The sizes of the two rectangles after the clipping.
pub(super) struct Clipped {
    /// The destination rectangle's width.
    pub dest_width: i64,
    /// The destination rectangle's height.
    pub dest_height: i64,
    /// The source rectangle's width.
    pub src_width: i64,
    /// The source rectangle's height.
    pub src_height: i64,
}

/// The weighted sums of the area one destination pixel covers.
#[derive(Default)]
struct Sums {
    /// The sum of the covered areas.
    total_area: i64,
    /// The sum of the source alphas, weighted by the covered area.
    alpha_sum: i64,
    /// The sum of the colour weights of the pixels that are not transparent.
    colour_weight: i64,
    /// The red sum of the pixels that are not transparent.
    red: i64,
    /// The green sum of the pixels that are not transparent.
    green: i64,
    /// The blue sum of the pixels that are not transparent.
    blue: i64,
    /// The sum of the covered areas of the transparent pixels.
    transparent_area: i64,
    /// The red sum of the transparent pixels.
    transparent_red: i64,
    /// The green sum of the transparent pixels.
    transparent_green: i64,
    /// The blue sum of the transparent pixels.
    transparent_blue: i64,
}

/// The integer part of a real, truncated toward zero.
///
/// A value that does not fit an integer saturates and a value that is not a
/// number becomes zero. The sizes such a value produces are empty, so the
/// copy that follows writes nothing.
fn trunc(value: f64) -> i64 {
    value.trunc() as i64
}

/// The size a rectangle takes when the rectangle it is paired with is scaled
/// from `old` to `new`.
///
/// The ratio is computed in double precision and the product is truncated
/// toward zero, so a third of a rectangle stays a third and is not rounded
/// up to a half.
fn scale(size: i64, new: i64, old: i64) -> i64 {
    trunc((size as f64) * ((new as f64) / (old as f64)))
}

/// Clip the two rectangles to the two images.
///
/// Each step shrinks one rectangle and shrinks its partner in the same
/// ratio, so the two keep the proportion the caller asked for. A rectangle
/// that lies outside its image keeps a negative size, and the copy then
/// writes nothing.
pub(super) fn clip(request: &Request, dest_image: (i64, i64), src_image: (i64, i64)) -> Clipped {
    let mut dest_width = request.dest_width;
    let mut dest_height = request.dest_height;
    let mut src_width = request.src_width;
    let mut src_height = request.src_height;

    // The destination rectangle's right edge.
    if request.dest_left.saturating_add(request.dest_width) > dest_image.0 {
        dest_width = dest_image.0 - request.dest_left;
        src_width = scale(request.src_width, dest_width, request.dest_width);
    }
    // The source rectangle's right edge. The divisor is the source width the
    // step above left behind.
    if request.src_left.saturating_add(src_width) > src_image.0 {
        dest_width = scale(dest_width, src_image.0 - request.src_left, src_width);
        src_width = src_image.0 - request.src_left;
    }
    // The destination rectangle's bottom edge.
    if request.dest_top.saturating_add(request.dest_height) > dest_image.1 {
        dest_height = dest_image.1 - request.dest_top;
        src_height = scale(request.src_height, dest_height, request.dest_height);
    }
    // The source rectangle's bottom edge. The divisor is the source height
    // the step above left behind.
    if request.src_top.saturating_add(src_height) > src_image.1 {
        dest_height = scale(dest_height, src_image.1 - request.src_top, src_height);
        src_height = src_image.1 - request.src_top;
    }

    Clipped {
        dest_width,
        dest_height,
        src_width,
        src_height,
    }
}

/// Copy the source rectangle into the destination rectangle, averaging the
/// area every destination pixel covers.
///
/// # Errors
///
/// Returns an error when the source pixel the average reads is not in the
/// source image, or when the destination pixel the average writes is not in
/// the destination image.
pub(super) fn copy_into(
    dest: &mut BitmapState,
    source: &BitmapState,
    request: &Request,
    clipped: &Clipped,
    src_image: (i64, i64),
) -> Result<(), String> {
    if clipped.dest_width <= 0 || clipped.dest_height <= 0 {
        // The copy covers no destination pixel.
        return Ok(());
    }
    // The source pixels one destination pixel covers, in stored units. The
    // step is truncated, so the covered region can fall short of the source
    // rectangle's far edge.
    let step_x = trunc((UNIT as f64) * (clipped.src_width as f64) / (clipped.dest_width as f64));
    let step_y = trunc((UNIT as f64) * (clipped.src_height as f64) / (clipped.dest_height as f64));
    // The covered region is anchored at the source rectangle's origin.
    let src_left = request.src_left * UNIT;
    let src_top = request.src_top * UNIT;
    // The clipping keeps the destination rectangle inside the destination
    // image; the two clamps are the guard that keeps a rectangle the
    // clipping could not shrink from running past the buffer.
    let columns = clipped
        .dest_width
        .min(i64::from(dest.width).saturating_sub(request.dest_left))
        .max(0);
    let rows = clipped
        .dest_height
        .min(i64::from(dest.height).saturating_sub(request.dest_top))
        .max(0);

    for y in 0..rows {
        let region_top = src_top + y * step_y;
        let region_bottom = src_top + (y + 1) * step_y;
        for x in 0..columns {
            let region_left = src_left + x * step_x;
            let region_right = src_left + (x + 1) * step_x;
            let Some(pixel) = average(
                source,
                src_image,
                region_left,
                region_right,
                region_top,
                region_bottom,
            )?
            else {
                // The destination pixel covers no source pixel: it keeps
                // the content it had.
                continue;
            };
            write_pixel(dest, request.dest_left + x, request.dest_top + y, pixel)?;
        }
    }
    // The written rectangle is the part the frame has to upload.
    if let (Ok(left), Ok(top), Ok(width), Ok(height)) = (
        u32::try_from(request.dest_left),
        u32::try_from(request.dest_top),
        u32::try_from(clipped.dest_width),
        u32::try_from(clipped.dest_height),
    ) {
        dest.mark_dirty_rect(PixelRegion::new(left, top, width, height));
    }
    Ok(())
}

/// The average of the area one destination pixel covers.
///
/// `None` when the covered region touches no source pixel, which leaves the
/// destination pixel as it was.
fn average(
    source: &BitmapState,
    src_image: (i64, i64),
    region_left: i64,
    region_right: i64,
    region_top: i64,
    region_bottom: i64,
) -> Result<Option<[u8; 4]>, String> {
    // The source pixels the covered region touches. The end of the range is
    // the column that holds the region's right edge, and the range itself
    // stops before it, so the source image's last column and last row are
    // never sampled.
    let first_col = region_left >> FRACTION;
    let end_col = ((region_right + UNIT - 1) >> FRACTION).min(src_image.0 - 1);
    let first_row = region_top >> FRACTION;
    let end_row = ((region_bottom + UNIT - 1) >> FRACTION).min(src_image.1 - 1);

    let mut sums = Sums::default();
    for row in first_row..end_row {
        for col in first_col..end_col {
            let pixel = pixel(source, col, row)?;
            // The part of the source pixel the covered region holds.
            let overlap_width =
                ((col + 1) * UNIT).min(region_right) - (col * UNIT).max(region_left);
            let overlap_height =
                ((row + 1) * UNIT).min(region_bottom) - (row * UNIT).max(region_top);
            let area = (overlap_width * overlap_height) >> FRACTION;
            let alpha = i64::from(pixel[3]);
            sums.total_area += area;
            sums.alpha_sum += alpha * area;
            if alpha > 0 {
                // The colour weighs on the area the pixel covers and on its
                // alpha; the alpha scales the area down to a 256th.
                let weight = (area * alpha) >> 8;
                sums.colour_weight += weight;
                sums.red += i64::from(pixel[0]) * weight;
                sums.green += i64::from(pixel[1]) * weight;
                sums.blue += i64::from(pixel[2]) * weight;
            } else {
                // A transparent pixel weighs on the alpha alone; its colour
                // is the fallback for a pixel that no other pixel colours.
                sums.transparent_area += area;
                sums.transparent_red += i64::from(pixel[0]) * area;
                sums.transparent_green += i64::from(pixel[1]) * area;
                sums.transparent_blue += i64::from(pixel[2]) * area;
            }
        }
    }

    if sums.total_area == 0 {
        // The covered region touches no source pixel.
        return Ok(None);
    }
    let alpha = sums.alpha_sum / sums.total_area;
    let (red, green, blue) = if sums.colour_weight > 0 {
        (
            sums.red / sums.colour_weight,
            sums.green / sums.colour_weight,
            sums.blue / sums.colour_weight,
        )
    } else if sums.transparent_area > 0 {
        (
            sums.transparent_red / sums.transparent_area,
            sums.transparent_green / sums.transparent_area,
            sums.transparent_blue / sums.transparent_area,
        )
    } else {
        (0, 0, 0)
    };
    let channel = |value: i64| {
        u8::try_from(value).map_err(|_| {
            "Layer.stretchCopyAA: the averaged colour is outside a channel".to_string()
        })
    };
    Ok(Some([
        channel(red)?,
        channel(green)?,
        channel(blue)?,
        channel(alpha)?,
    ]))
}

/// The source pixel at `(column, row)`.
fn pixel(source: &BitmapState, column: i64, row: i64) -> Result<[u8; 4], String> {
    let outside =
        || "Layer.stretchCopyAA: the source pixel is outside the source image".to_string();
    let (Ok(column), Ok(row)) = (u32::try_from(column), u32::try_from(row)) else {
        return Err(outside());
    };
    let offset = source.pixel_offset(column, row).ok_or_else(outside)?;
    let rgba = source.rgba.get(offset..offset + 4).ok_or_else(outside)?;
    Ok([rgba[0], rgba[1], rgba[2], rgba[3]])
}

/// Write one pixel of the destination image.
fn write_pixel(dest: &mut BitmapState, x: i64, y: i64, pixel: [u8; 4]) -> Result<(), String> {
    let outside = || {
        "Layer.stretchCopyAA: the destination pixel is outside the destination image".to_string()
    };
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return Err(outside());
    };
    let offset = dest.pixel_offset(x, y).ok_or_else(outside)?;
    let target = dest.rgba.get_mut(offset..offset + 4).ok_or_else(outside)?;
    target.copy_from_slice(&pixel);
    Ok(())
}
