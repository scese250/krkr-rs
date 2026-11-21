//! `Layer.shrinkCopy`: the area average.
//!
//! The destination rectangle is mapped onto the source rectangle, and every
//! destination pixel averages the area it covers. The average is taken at a
//! 1/256-pixel resolution: the covered area of every source pixel is a
//! fraction of that resolution, so the result is an approximation of the
//! exact area average.
//!
//! The source rectangle is clipped to the source image and the destination
//! rectangle to the destination image; a rectangle that covers no pixel
//! leaves the destination untouched.

use api::geometry::PixelRegion;
use tjs::Value;

use crate::CompatCtx;
use crate::engine_api::BitmapState;
use crate::natives::system::value_as_i64;

use super::{bitmap_size, layer_bitmap, scene_mut, value_as_f64};

/// The resolution of the average: 1/256 of a pixel.
const UNIT: i64 = 256;

/// The arguments of one copy.
struct Request {
    /// The destination rectangle's left and top, in the destination image's
    /// pixels. They may be fractional.
    dx: f64,
    dy: f64,
    /// The destination rectangle's width and height.
    dw: f64,
    dh: f64,
    /// The source rectangle's left and top, in source pixels.
    sx: i64,
    sy: i64,
    /// The source rectangle's width and height.
    sw: i64,
    sh: i64,
}

/// The rectangles of one copy, after the clipping.
struct Rectangles {
    /// The destination rectangle's left and top.
    dx: f64,
    dy: f64,
    /// The destination rectangle's width and height.
    dw: f64,
    dh: f64,
    /// The source rectangle's left and top.
    sx: i64,
    sy: i64,
    /// The source rectangle's width and height.
    sw: i64,
    sh: i64,
    /// The destination rectangle's integer origin.
    dtx: i64,
    dty: i64,
    /// The first destination column and row of the copy, counted from the
    /// integer origin.
    dsx: i64,
    dsy: i64,
    /// The last destination column and row of the copy (exclusive), counted
    /// from the integer origin.
    dex: i64,
    dey: i64,
}

/// The weights of one destination pixel's span along one axis.
///
/// The span reaches from the source pixel it starts in to the source pixel it
/// ends in: the first and the last of them are covered only in part, and the
/// pixels between them are covered whole.
struct Span {
    /// The index of the span's first whole pixel. The pixel before it is
    /// the span's first (partial) pixel.
    offset: i64,
    /// The number of whole pixels between the two partial ones.
    step: i64,
    /// The alpha weight of the span's first (partial) pixel.
    ta: i64,
    /// The colour weight of the span's first (partial) pixel.
    tc: i64,
    /// The alpha weight of the span's last (partial) pixel.
    ba: i64,
    /// The colour weight of the span's last (partial) pixel.
    bc: i64,
    /// The colour weight of the whole span.
    total: i64,
}

/// The resolution of the average along the two axes.
struct Resolution {
    /// The horizontal unit.
    hu: i64,
    /// The vertical unit.
    vu: i64,
}

impl Resolution {
    /// The weight of a whole pixel in the middle of a span.
    fn unit(&self) -> i64 {
        self.hu * self.vu
    }
}

/// A rectangle of source pixels.
struct Area {
    /// The rectangle's first column.
    x: i64,
    /// The rectangle's first row.
    y: i64,
    /// The rectangle's width.
    width: i64,
    /// The rectangle's height.
    height: i64,
}

/// The weighted sums of the area one destination pixel covers.
#[derive(Default)]
struct Sum {
    /// The red channel's sum.
    r: i64,
    /// The green channel's sum.
    g: i64,
    /// The blue channel's sum.
    b: i64,
    /// The alpha channel's sum.
    a: i64,
}

impl Sum {
    /// Add one source pixel, weighted for the colour and for the alpha.
    fn add(&mut self, pixel: [u8; 4], colour: i64, alpha: i64) {
        self.r += i64::from(pixel[0]) * colour;
        self.g += i64::from(pixel[1]) * colour;
        self.b += i64::from(pixel[2]) * colour;
        self.a += i64::from(pixel[3]) * alpha;
    }
}

/// The integer part of a real, truncated toward zero.
fn trunc(value: f64) -> i64 {
    value.trunc() as i64
}

/// Copy the source rectangle into the destination rectangle, averaging the
/// area every destination pixel covers.
///
/// # Errors
///
/// Returns an error when the source rectangle does not cover the
/// destination rectangle, when either layer has no main image, or when the
/// copy cannot read the source it was given.
pub(super) fn shrink_copy(
    ctx: &CompatCtx,
    dst_id: u32,
    src_id: u32,
    a: &[Value],
) -> Result<(), String> {
    let request = Request {
        dx: value_as_f64(&a[0]),
        dy: value_as_f64(&a[1]),
        dw: value_as_f64(&a[2]),
        dh: value_as_f64(&a[3]),
        sx: value_as_i64(&a[5]),
        sy: value_as_i64(&a[6]),
        sw: value_as_i64(&a[7]),
        sh: value_as_i64(&a[8]),
    };
    // The copy only shrinks: a destination rectangle wider or taller than
    // the source rectangle is refused.
    if request.sw <= 0
        || request.sh <= 0
        || request.dw <= 0.0
        || request.dh <= 0.0
        || request.sw < trunc(request.dw)
        || request.sh < trunc(request.dh)
    {
        return Err(
            "Layer.shrinkCopy: the source rectangle must cover the destination rectangle"
                .to_string(),
        );
    }
    let scene = scene_mut(ctx)?;
    let src_bitmap = layer_bitmap(scene, src_id, "source")?;
    let dst_bitmap = layer_bitmap(scene, dst_id, "destination")?;
    // The images may still be decoding: the copy reads and writes their
    // pixels.
    scene.resolve_bitmap(src_bitmap);
    scene.resolve_bitmap(dst_bitmap);
    let (src_w, src_h) = bitmap_size(scene, src_bitmap)?;
    let (dst_w, dst_h) = bitmap_size(scene, dst_bitmap)?;

    let Some(rects) = clip(request, src_w, src_h, dst_w, dst_h) else {
        // The copy covers no pixel: the destination keeps its content.
        return Ok(());
    };
    // The horizontal table holds one span per destination column, the
    // vertical one one span per destination row.
    let (horz, hu) = make_table(
        rects.dsx,
        rects.dex - 1,
        rects.sw as f64,
        rects.dw,
        rects.dx - rects.dtx as f64,
        rects.sx,
    );
    let (vert, vu) = make_table(
        rects.dsy,
        rects.dey - 1,
        rects.sh as f64,
        rects.dh,
        rects.dy - rects.dty as f64,
        rects.sy,
    );
    let resolution = Resolution { hu, vu };
    let copied = scene.with_two_bitmaps(dst_bitmap, src_bitmap, |dest, source| {
        copy_into(dest, source, &rects, &horz, &vert, &resolution)
    });
    match copied {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(error),
        None => Err("Layer.shrinkCopy: the destination image does not exist".to_string()),
    }
}

/// Clip the source rectangle to the source image and the destination
/// rectangle to the destination image.
///
/// `None` when the copy covers no pixel: the destination keeps its content.
fn clip(request: Request, src_w: i64, src_h: i64, dst_w: i64, dst_h: i64) -> Option<Rectangles> {
    let Request {
        mut dx,
        mut dy,
        mut dw,
        mut dh,
        mut sx,
        mut sy,
        mut sw,
        mut sh,
    } = request;
    // The source pixels one destination pixel covers.
    let zx = dw / sw as f64;
    let zy = dh / sh as f64;
    // The source rectangle is clipped to the source image; the destination
    // rectangle moves with the part that is left.
    if sx + sw <= 0 || sy + sh <= 0 || sx >= src_w || sy >= src_h {
        return None;
    }
    if sx < 0 {
        let cut = zx * (-sx) as f64;
        sw += sx;
        dw -= cut;
        sx = 0;
        dx += cut;
    }
    if sy < 0 {
        let cut = zy * (-sy) as f64;
        sh += sy;
        dh -= cut;
        sy = 0;
        dy += cut;
    }
    let cut = sx + sw - src_w;
    if cut > 0 {
        sw -= cut;
        dw -= zx * cut as f64;
    }
    let cut = sy + sh - src_h;
    if cut > 0 {
        sh -= cut;
        dh -= zy * cut as f64;
    }
    // The destination rectangle's integer position.
    let dtx = trunc(dx);
    let dty = trunc(dy);
    let mut dtw = trunc(dx + dw) - dtx;
    let mut dth = trunc(dy + dh) - dty;
    // A rectangle whose end was truncated away still covers that pixel.
    if dx + dw > (dtx + dtw) as f64 {
        dtw += 1;
    }
    if dy + dh > (dty + dth) as f64 {
        dth += 1;
    }
    if dtx + dtw <= 0 || dty + dth <= 0 || dtx >= dst_w || dty >= dst_h {
        return None;
    }
    // The destination rectangle's range inside the destination image.
    let dsx = if dtx < 0 { -dtx } else { 0 };
    let dsy = if dty < 0 { -dty } else { 0 };
    let dex = if dtx + dtw > dst_w { dst_w - dtx } else { dtw };
    let dey = if dty + dth > dst_h { dst_h - dty } else { dth };
    Some(Rectangles {
        dx,
        dy,
        dw,
        dh,
        sx,
        sy,
        sw,
        sh,
        dtx,
        dty,
        dsx,
        dsy,
        dex,
        dey,
    })
}

/// The weights of every destination pixel's span along one axis.
///
/// The first and the last span are edge spans: their ends reach past the
/// source rectangle, and the part that reaches past it is folded onto the
/// boundary pixel.
///
/// `max` is the source rectangle's size along the axis, `size` the
/// destination rectangle's, `diff` the distance between the destination
/// rectangle's fractional origin and its integer origin, and `stop` the
/// source rectangle's origin.
fn make_table(
    first: i64,
    last: i64,
    max: f64,
    size: f64,
    diff: f64,
    stop: i64,
) -> (Vec<Span>, i64) {
    let ratio = max / size;
    let unit = unit_for(ratio);
    let mut table = Vec::with_capacity((last - first).max(0) as usize);
    for pos in first..=last {
        let span = if pos == first || pos == last {
            edge_span(unit, max, ratio, diff, stop, pos)
        } else {
            inner_span(unit, ratio, diff, stop, pos)
        };
        table.push(span);
    }
    (table, unit)
}

/// The resolution of the average along one axis.
///
/// A span that covers many source pixels lowers the resolution, so the
/// weights of a whole span stay representable.
fn unit_for(ratio: f64) -> i64 {
    if ratio > 1.0 / 16.0 {
        return UNIT;
    }
    let mut unit = UNIT / ((2.0 / 16.0) / ratio) as i64;
    if unit <= 0 {
        unit = 1;
    }
    unit
}

/// The weights of a span that lies inside the source rectangle.
fn inner_span(unit: i64, ratio: f64, diff: f64, stop: i64, pos: i64) -> Span {
    let r1 = (pos as f64 - diff) * ratio;
    let r2 = r1 + ratio;
    let t1 = trunc(r1);
    let t2 = trunc(r2);
    // The first pixel of the span is covered from the right, the last one
    // from the left.
    let tc = unit - trunc((r1 - t1 as f64) * unit as f64);
    let bc = trunc((r2 - t2 as f64) * unit as f64);
    let step = t2 - t1 - 1;
    Span {
        offset: stop + t1 + 1,
        step,
        ta: tc,
        tc,
        ba: bc,
        bc,
        total: tc + bc + step * unit,
    }
}

/// The weights of a span that reaches past the source rectangle.
///
/// The part of the span that reaches past the rectangle is folded onto the
/// boundary pixel: the colour keeps the weight of the whole span, the alpha
/// only the weight of the part that is inside the rectangle.
fn edge_span(unit: i64, max: f64, ratio: f64, diff: f64, stop: i64, pos: i64) -> Span {
    let r1 = (pos as f64 - diff) * ratio;
    let r2 = r1 + ratio;
    let f1 = r1.max(0.0);
    let f2 = r2.min(max);
    let t1 = trunc(r1);
    let t2 = trunc(r2);
    let u1 = trunc(f1);
    let u2 = trunc(f2);
    let tc = unit - trunc((r1 - t1 as f64) * unit as f64);
    let bc = trunc((r2 - t2 as f64) * unit as f64);
    let ta = unit - trunc((f1 - u1 as f64) * unit as f64);
    let ba = trunc((f2 - u2 as f64) * unit as f64);
    let step = u2 - u1 - 1;
    Span {
        offset: stop + u1 + 1,
        step,
        ta,
        tc,
        ba,
        bc,
        total: tc + bc + step * unit,
    }
}

/// Write the averaged area of every destination pixel.
fn copy_into(
    dest: &mut BitmapState,
    source: &BitmapState,
    rects: &Rectangles,
    horz: &[Span],
    vert: &[Span],
    resolution: &Resolution,
) -> Result<(), String> {
    for (row, v) in vert.iter().enumerate() {
        let y = rects.dty + rects.dsy + row as i64;
        for (column, h) in horz.iter().enumerate() {
            let x = rects.dtx + rects.dsx + column as i64;
            let pixel = average(source, h, v, resolution)?;
            let offset = dest
                .pixel_offset(
                    u32::try_from(x)
                        .map_err(|_| "Layer.shrinkCopy: the destination column is out of range")?,
                    u32::try_from(y)
                        .map_err(|_| "Layer.shrinkCopy: the destination row is out of range")?,
                )
                .ok_or_else(|| {
                    "Layer.shrinkCopy: the destination pixel is out of range".to_string()
                })?;
            let target = dest.rgba.get_mut(offset..offset + 4).ok_or_else(|| {
                "Layer.shrinkCopy: the destination pixel is out of range".to_string()
            })?;
            target.copy_from_slice(&pixel);
        }
    }
    // The written rectangle is the part the frame has to upload.
    let left = u32::try_from(rects.dtx + rects.dsx)
        .map_err(|_| "Layer.shrinkCopy: the destination column is out of range".to_string())?;
    let top = u32::try_from(rects.dty + rects.dsy)
        .map_err(|_| "Layer.shrinkCopy: the destination row is out of range".to_string())?;
    let width = u32::try_from(rects.dex - rects.dsx)
        .map_err(|_| "Layer.shrinkCopy: the destination width is out of range".to_string())?;
    let height = u32::try_from(rects.dey - rects.dsy)
        .map_err(|_| "Layer.shrinkCopy: the destination height is out of range".to_string())?;
    dest.mark_dirty_rect(PixelRegion::new(left, top, width, height));
    Ok(())
}

/// The average of the area one destination pixel covers.
fn average(
    source: &BitmapState,
    h: &Span,
    v: &Span,
    resolution: &Resolution,
) -> Result<[u8; 4], String> {
    let mut sum = Sum::default();
    // The span's first and last pixel, and the whole pixels between them.
    let x_first = h.offset - 1;
    let x_whole = h.offset;
    let x_last = h.offset + h.step;
    let y_first = v.offset - 1;
    let y_whole = v.offset;
    let y_last = v.offset + v.step;
    // The four corners, the four edges and the middle of the area.
    add_point(source, &mut sum, x_first, y_first, h.tc * v.tc, h.ta * v.ta)?;
    add_region(
        source,
        &mut sum,
        Area {
            x: x_whole,
            y: y_first,
            width: h.step,
            height: 1,
        },
        resolution.hu * v.tc,
        resolution.hu * v.ta,
    )?;
    add_point(source, &mut sum, x_last, y_first, h.bc * v.tc, h.ba * v.ta)?;
    add_region(
        source,
        &mut sum,
        Area {
            x: x_first,
            y: y_whole,
            width: 1,
            height: v.step,
        },
        h.tc * resolution.vu,
        h.ta * resolution.vu,
    )?;
    add_region(
        source,
        &mut sum,
        Area {
            x: x_whole,
            y: y_whole,
            width: h.step,
            height: v.step,
        },
        resolution.unit(),
        resolution.unit(),
    )?;
    add_region(
        source,
        &mut sum,
        Area {
            x: x_last,
            y: y_whole,
            width: 1,
            height: v.step,
        },
        h.bc * resolution.vu,
        h.ba * resolution.vu,
    )?;
    add_point(source, &mut sum, x_first, y_last, h.tc * v.bc, h.ta * v.ba)?;
    add_region(
        source,
        &mut sum,
        Area {
            x: x_whole,
            y: y_last,
            width: h.step,
            height: 1,
        },
        resolution.hu * v.bc,
        resolution.hu * v.ba,
    )?;
    add_point(source, &mut sum, x_last, y_last, h.bc * v.bc, h.ba * v.ba)?;

    let divisor = h.total * v.total;
    if divisor <= 0 {
        return Err("Layer.shrinkCopy: the averaged area has no weight".to_string());
    }
    let channel = |value: i64| {
        u8::try_from(value / divisor)
            .map_err(|_| "Layer.shrinkCopy: the averaged colour is outside a channel".to_string())
    };
    Ok([
        channel(sum.r)?,
        channel(sum.g)?,
        channel(sum.b)?,
        channel(sum.a)?,
    ])
}

/// The source pixel at `(x, y)`.
fn pixel(source: &BitmapState, x: i64, y: i64) -> Option<[u8; 4]> {
    let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
        return None;
    };
    let offset = source.pixel_offset(x, y)?;
    let rgba = source.rgba.get(offset..offset + 4)?;
    Some([rgba[0], rgba[1], rgba[2], rgba[3]])
}

/// Add one source pixel to the sum.
///
/// A region with a zero alpha weight does not contribute and is not read.
fn add_point(
    source: &BitmapState,
    sum: &mut Sum,
    x: i64,
    y: i64,
    colour: i64,
    alpha: i64,
) -> Result<(), String> {
    if alpha == 0 {
        return Ok(());
    }
    let pixel = pixel(source, x, y).ok_or_else(|| {
        format!("Layer.shrinkCopy: the source pixel ({x}, {y}) is outside the source image")
    })?;
    sum.add(pixel, colour, alpha);
    Ok(())
}

/// Add a region of source pixels to the sum.
///
/// A region with a zero alpha weight does not contribute and is not read.
fn add_region(
    source: &BitmapState,
    sum: &mut Sum,
    area: Area,
    colour: i64,
    alpha: i64,
) -> Result<(), String> {
    if alpha == 0 {
        return Ok(());
    }
    for row in area.y..area.y + area.height {
        for column in area.x..area.x + area.width {
            add_point(source, sum, column, row, colour, alpha)?;
        }
    }
    Ok(())
}
