//! The `layerExLongExposure` plugin: the five members a `Layer` class gains
//! when a script links `layerExLongExposure.dll`.
//!
//! The members accumulate a layer's image over many frames and write the
//! normalised accumulation back into the layer's own image:
//!
//! * `initExposure` starts a fresh accumulation for the layer's current image
//!   size.
//! * `snapExposure` adds the layer's current image to the accumulation.
//! * `statExposure` reports the smallest and the largest accumulated value of
//!   each of the four channels.
//! * `copyExposure` writes the normalised accumulation into the layer's image,
//!   stretched by the ranges its argument carries.
//! * `termExposure` releases the accumulation.
//!
//! The plugin defines no class of its own: the members hang on the engine's
//! own `Layer` class, and the plugin table entry is what publishes them. The
//! province plane is never read and never written; every member works on the
//! main image alone.
//!
//! # The accumulation
//!
//! The accumulation holds four 32-bit counters per pixel of the layer's main
//! image — one per colour channel and one for the alpha — together with the
//! image size it was created for. A snap adds the image's four bytes to the
//! counters of the pixel they belong to; the counters are unsigned and wrap.
//!
//! # The normalisation
//!
//! A channel is written as `floor((n - lo) * 255 / (hi - lo))`, where `n` is
//! the accumulated value clamped into `[lo, hi]` and `(lo, hi)` is that
//! channel's own pair of extremes. A pair with `lo >= hi` has no range to
//! stretch and writes 255, so the image comes out opaque white.
//!
//! The product is computed in 32 bits and wraps, and the quotient is then
//! truncated to a byte. A wider computation would answer differently for a
//! wide range, so the wrap belongs to the arithmetic.

#![allow(unsafe_code)]

mod accumulate;
mod report;

use std::collections::HashMap;
use std::ffi::c_void;

use tjs::{NativeInstanceMembers, Tjs2Engine};

use crate::engine_api::Scene;
use crate::natives::system::resolve;
use crate::{CompatCtx, CompatError};

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the five `Layer` members.
pub fn register(ctx: &mut CompatCtx) -> Result<(), CompatError> {
    let mut methods = accumulate::method_defs();
    methods.extend(report::method_defs());
    ctx.register_instance_members(&NativeInstanceMembers {
        class_name: "Layer",
        methods,
        properties: Vec::new(),
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The message a member raises when the receiver is not a layer.
pub(super) const NOT_A_LAYER: &str = "LongExposure: the receiver is not a Layer";

/// The installed engine → context binding for a native callback.
pub(super) fn bound(
    engine: *mut c_void,
) -> Result<(crate::natives::system::EngineBinding, CompatCtx), String> {
    let resolved = resolve(engine)
        .ok_or_else(|| "LongExposure: the compat context is not bound".to_string())?;
    let ctx = resolved.ctx();
    Ok((resolved, ctx))
}

/// The attached scene, mutably.
pub(super) fn scene_mut<'a>(ctx: &CompatCtx) -> Result<&'a mut Scene, String> {
    // SAFETY: the host keeps the attached scene alive for the binding's
    // lifetime and the VM is single-threaded.
    let ptr = unsafe { ctx.services() }
        .scene()
        .ok_or_else(|| "LongExposure: no scene attached".to_string())?;
    // SAFETY: see above.
    Ok(unsafe { &mut *ptr })
}

/// The attached scene, shared.
pub(super) fn scene_ref<'a>(ctx: &CompatCtx) -> Result<&'a Scene, String> {
    // SAFETY: the host keeps the attached scene alive for the binding's
    // lifetime and the VM is single-threaded.
    let ptr = unsafe { ctx.services() }
        .scene()
        .ok_or_else(|| "LongExposure: no scene attached".to_string())?;
    // SAFETY: see above.
    Ok(unsafe { &*ptr })
}

/// The scene id of the layer a member was called on.
///
/// The member is a `Layer` method, so the receiver names a layer; a receiver
/// that names none carries the member's own message.
pub(super) fn receiver_id(
    host: &Tjs2Engine,
    objthis: *mut c_void,
    message: &str,
) -> Result<u32, String> {
    if objthis.is_null() {
        return Err(message.to_string());
    }
    let object = host
        .retain_object_detached(objthis)
        .map_err(|error| format!("{message}: {error}"))?;
    match crate::CompatCtx::native_instance_id(host, object.raw_id(), &["Layer"])? {
        Some(id) => Ok(id),
        None => Err(message.to_string()),
    }
}

/// The layer's image size, when it has an image to work on.
///
/// A layer without a main image, and a layer whose image size is not
/// positive, have no image to accumulate.
pub(super) fn image_size(scene: &Scene, id: u32) -> Option<(u32, u32)> {
    let layer = scene.layer(id)?;
    let _image = layer.bitmap?;
    let (width, height) = (layer.image_width, layer.image_height);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

// ---------------------------------------------------------------------------
// The accumulation
// ---------------------------------------------------------------------------

/// One layer's accumulation: the counters of every pixel of its image.
///
/// The counters are four per pixel, in the order the image's own bytes come
/// in: red, green, blue, alpha.
pub(super) struct Accumulation {
    /// The image width the buffer was created for.
    pub(super) width: u32,
    /// The image height the buffer was created for.
    pub(super) height: u32,
    /// The counters: four per pixel.
    counters: Vec<u32>,
}

impl Accumulation {
    /// A zeroed accumulation for an image of `width` x `height` pixels.
    ///
    /// # Errors
    ///
    /// Returns an error when the size is not representable or when the buffer
    /// cannot be allocated.
    pub(super) fn new(width: u32, height: u32) -> Result<Self, String> {
        let count = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "the image size is not representable".to_string())?;
        let mut counters = Vec::new();
        if counters.try_reserve_exact(count).is_err() {
            return Err("the accumulation buffer could not be allocated".to_string());
        }
        counters.resize(count, 0);
        Ok(Self {
            width,
            height,
            counters,
        })
    }

    /// The number of bytes an image of the accumulated size holds.
    pub(super) fn bytes(&self) -> usize {
        self.counters.len()
    }

    /// Add an image's pixels to the counters.
    ///
    /// The pixels are the image's own bytes, four per pixel; the caller has
    /// checked that the buffer holds the pixels the accumulated size names.
    pub(super) fn add(&mut self, pixels: &[u8]) {
        let count = self.counters.len() / 4;
        for (index, pixel) in pixels.as_chunks::<4>().0.iter().take(count).enumerate() {
            let base = index * 4;
            for (channel, value) in pixel.iter().enumerate() {
                self.counters[base + channel] =
                    self.counters[base + channel].wrapping_add(u32::from(*value));
            }
        }
    }

    /// The smallest and the largest accumulated value of each channel.
    pub(super) fn extremes(&self) -> Ranges {
        let mut lo = [u32::MAX; 4];
        let mut hi = [0u32; 4];
        for pixel in self.counters.as_chunks::<4>().0 {
            for (channel, value) in pixel.iter().enumerate() {
                lo[channel] = lo[channel].min(*value);
                hi[channel] = hi[channel].max(*value);
            }
        }
        Ranges { lo, hi }
    }

    /// Write the normalised counters into an image's pixels.
    ///
    /// The pixels are the image's own bytes, four per pixel; the caller has
    /// checked that the buffer holds the pixels the accumulated size names.
    pub(super) fn write(&self, ranges: &Ranges, pixels: &mut [u8]) {
        let count = self.counters.len() / 4;
        for (index, pixel) in pixels
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .take(count)
            .enumerate()
        {
            let base = index * 4;
            for (channel, value) in pixel.iter_mut().enumerate() {
                *value = ranges.channel(channel, self.counters[base + channel]);
            }
        }
    }
}

/// The per-channel extremes a copy stretches the counters by.
#[derive(Clone, Copy)]
pub(super) struct Ranges {
    /// The lower bound of each channel: red, green, blue, alpha.
    pub(super) lo: [u32; 4],
    /// The upper bound of each channel.
    pub(super) hi: [u32; 4],
}

impl Default for Ranges {
    /// The built-in pair: a lower bound above the upper one, so every channel
    /// is written as 255.
    fn default() -> Self {
        Self {
            lo: [u32::MAX; 4],
            hi: [0; 4],
        }
    }
}

impl Ranges {
    /// The byte one channel is written as.
    ///
    /// The counter is clamped into the range before it is scaled, and the
    /// product of the scaling is computed in 32 bits: it wraps, and the
    /// quotient is truncated to a byte.
    fn channel(&self, channel: usize, value: u32) -> u8 {
        let (lo, hi) = (self.lo[channel], self.hi[channel]);
        if lo >= hi {
            // The pair has no range to stretch, so every value is the top of
            // the range.
            return 255;
        }
        let value = value.clamp(lo, hi);
        let product = (value - lo).wrapping_mul(255);
        (product / (hi - lo)) as u8
    }
}

// ---------------------------------------------------------------------------
// The per-layer state
// ---------------------------------------------------------------------------

/// The accumulation of every layer that has one.
#[derive(Default)]
pub(super) struct ExposureState {
    /// The accumulation of each layer, keyed by the layer's scene id.
    layers: HashMap<u32, Accumulation>,
}

/// The accumulation state of the plugin.
pub(super) fn exposure_state(ctx: &CompatCtx) -> std::cell::RefMut<'_, ExposureState> {
    // SAFETY: `ctx` was resolved from the installed binding.
    unsafe { ctx.services() }.state::<ExposureState>()
}

/// Release the accumulation of the layers that are gone.
///
/// The accumulation belongs to the layer, so it goes with the layer: an entry
/// whose layer no longer exists is dropped. The receiver's own entry is kept,
/// so a layer the engine has torn down still answers with the error its image
/// deserves instead of losing its accumulation.
pub(super) fn release_gone(ctx: &CompatCtx, receiver: u32) {
    // SAFETY: the host keeps the attached scene alive for the binding's
    // lifetime and the VM is single-threaded.
    let Some(ptr) = (unsafe { ctx.services() }).scene() else {
        return;
    };
    // SAFETY: the host keeps the attached scene alive for the binding's
    // lifetime and the VM is single-threaded.
    let scene = unsafe { &*ptr };
    let mut state = exposure_state(ctx);
    state
        .layers
        .retain(|id, _| *id == receiver || scene.layer(*id).is_some());
}

#[cfg(test)]
mod tests {
    use super::Ranges;

    /// The product of the scaling is computed in 32 bits, so a wide range
    /// wraps it: the byte is the one the wrapped product produces.
    ///
    /// The range `(0, 0xffffffff)` and the value `16843010` make the product
    /// `4294967550`, which is 254 past the end of the 32-bit range; the
    /// quotient is therefore zero, where the exact product would give one.
    #[test]
    fn the_scaling_product_wraps() {
        let ranges = Ranges {
            lo: [0; 4],
            hi: [u32::MAX; 4],
        };
        assert_eq!(ranges.channel(0, 16_843_009), 1);
        assert_eq!(ranges.channel(0, 16_843_010), 0);
        // A range that does not overflow scales the value as it stands.
        let narrow = Ranges {
            lo: [0; 4],
            hi: [255; 4],
        };
        assert_eq!(narrow.channel(0, 0), 0);
        assert_eq!(narrow.channel(0, 255), 255);
        assert_eq!(narrow.channel(0, 128), 128);
    }

    /// A pair whose lower bound is not below its upper bound has no range to
    /// stretch and writes the top of the range.
    #[test]
    fn an_empty_range_writes_255() {
        let ranges = Ranges {
            lo: [0; 4],
            hi: [0; 4],
        };
        assert_eq!(ranges.channel(0, 0), 255);
        assert_eq!(ranges.channel(0, 12_345), 255);
        let inverted = Ranges {
            lo: [u32::MAX; 4],
            hi: [0; 4],
        };
        assert_eq!(inverted.channel(0, 0), 255);
    }
}
