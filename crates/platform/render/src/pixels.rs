//! Pixel-format helpers for the surfaces the pipeline reads back.
//!
//! The pipeline composites in the premultiplied space, so a surface it
//! renders into holds premultiplied pixels, while the engine's bitmaps hold
//! straight-alpha pixels. [`unpremultiply_rgba8`] is the round trip from one
//! to the other.

/// Converts premultiplied RGBA8 pixels into straight-alpha pixels.
///
/// The colour channels of a premultiplied pixel are the straight colour
/// scaled by the pixel's alpha, so the straight colour is the premultiplied
/// one divided by the alpha. A pixel with no alpha has no colour to keep,
/// and its colour is zero.
///
/// The division is rounded down, so the result never exceeds the
/// premultiplied value it came from.
pub fn unpremultiply_rgba8(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }
        for channel in &mut pixel[..3] {
            let value = u32::from(*channel) * 255 / alpha;
            *channel = value.min(255) as u8;
        }
    }
}
