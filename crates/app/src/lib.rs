//! krkr-rs host assembly: the middle-layer engine, wired to the L1
//! platform by the `krkr` binary.
//!
//! [`Host`] wraps the middle-layer [`engine::Engine`] and nothing else.
//! Every evolving value — the layer tree [`Scene`], the projected
//! [`RenderScene`], the [`RenderState`]/[`Surface`] pair, the bitmap-id →
//! [`Texture`] table, the timer registry and the script-visible input state
//! — belongs to the engine, so the host holds no frame state of its own and
//! drives one engine frame per [`Host::step`]. The stateless L1 services are
//! created where they are used: the pipeline and the frame's destination
//! (the surface of a windowed host, the offscreen target of a headless one)
//! by [`Host::new`], the file system and the media backend by
//! [`GameHost::open_with_preload`].
//!
//! There are no globals, statics or hidden caches: every value is a local
//! field of [`Host`] and is passed to the bottom objects explicitly. Two
//! hosts can therefore run in the same process.

// The composition root installs the VM's raw-pointer binding to the L2 scene,
// storage, timer and input (`compat::CompatServices::attach_*`), which is
// inherently `unsafe`. That wiring is the crate's only `unsafe`, confined to
// `game` and covered by SAFETY comments; every other module is safe.
#![deny(unsafe_code)]
#![warn(missing_docs)]

mod audio;
mod frame_dump;
mod frame_times;
mod game;
mod image_codec;
mod overlay;
mod runner;
mod startup;
mod video;
mod window_ex;

use std::time::Duration;

use api::config::{CacheLimit, FrameRateLimit};
use api::error::RenderError;
use api::host::SurfaceEvent;
use engine::Scene;
// The engine's frame target: the frame is presented to a surface, or drawn
// into an offscreen target on a host with no window.
use engine::engine::FrameTarget;
use overlay::Overlay;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use render::{
    FrameTimings, PipelineConfig, PresentedFrame, PresentedFrameSlot, RenderScene, RenderState,
    RenderTargetDesc, Surface, Texture, TextureFormat, begin_offscreen_render_state,
    begin_render_state, begin_render_state_with_display, create_render_pipeline,
    create_render_target, install_presented_frame_slot,
};

pub(crate) use audio::install_audio_decoder_metered;
// The clock source seam, and the source the windowed loops use: the platform
// measures the elapsed time between frames and the source reports the real
// clock that measurement comes from, so the script's tick count moves inside a
// frame.
pub use engine::{FrameTimeSource, InjectedFrameTime, RenderPolicy};

/// The windowed loop's clock source.
///
/// The platform measures the elapsed time between frames and the loop writes it
/// into [`Self::elapsed_ms`] before the frame; the source's reading is the real
/// clock that measurement comes from. A source's address is handed to the
/// compatibility layer once, so a host owns one of these in a `Box`.
pub struct PlatformFrameTime {
    /// The instant the frames' time is measured from.
    started: std::time::Instant,
    /// The milliseconds the platform measured since the previous frame.
    pub elapsed_ms: f64,
}

impl PlatformFrameTime {
    /// A source whose reading starts at zero now.
    pub fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            elapsed_ms: 0.0,
        }
    }
}

impl Default for PlatformFrameTime {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameTimeSource for PlatformFrameTime {
    fn elapsed_ms(&self) -> f64 {
        self.elapsed_ms
    }

    fn now_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
}

pub use frame_times::FrameTimeReport;
pub use game::{GameError, GameHost, vm_report};
pub use image_codec::install_image_codec;
pub(crate) use image_codec::{install_image_codec_metered, install_image_decoder};
pub use render::BackendKind;
// The resolution a run is created at: the platform owns the window, so it
// owns the choice of the resolution the window is created at.
pub use platform::Resolution;
// The emulator entry point. A launcher resolves `Launch` (including the
// `RuntimeConfig` it loaded its own way) and calls `run`; see the `runner`
// module for the entry point contract.
pub use runner::{GameInput, Launch, RunError, RunMode, RunReport, run};
pub(crate) use video::install_video_source_metered;
pub(crate) use window_ex::install_window_ex_facts;

/// Errors surfaced by [`Host`].
///
/// The host reports the engine's own frame, configuration and overlay failures
/// alongside its own configuration check.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HostError {
    /// A bottom-layer render action failed.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// The host configuration is invalid (for example, a non-headless host
    /// with no window handle).
    #[error("host config: {0}")]
    Config(String),
    /// The debug overlay could not be built or refreshed.
    #[error("debug overlay: {0}")]
    Overlay(String),
    /// The frame hook failed; the frame was abandoned before drawing.
    #[error("frame: {0}")]
    Frame(String),
}

impl From<engine::EngineError> for HostError {
    fn from(error: engine::EngineError) -> Self {
        match error {
            engine::EngineError::Render(error) => Self::Render(error),
            engine::EngineError::Overlay(message) => Self::Overlay(message),
            engine::EngineError::Frame(message) => Self::Frame(message),
            // `EngineError` is non-exhaustive, so a variant this build cannot
            // classify keeps its message as a frame failure.
            other => Self::Frame(other.to_string()),
        }
    }
}

/// Outcome of one [`Host::step`].
pub use engine::FrameReport as StepReport;

/// Immutable configuration for [`Host::new`].
///
/// The type is `#[non_exhaustive]`: start from [`HostConfig::default`] or
/// [`HostConfig::headless`] and assign the fields you need.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct HostConfig {
    /// Render backend to create.
    pub backend: BackendKind,
    /// Initial surface size in pixels.
    pub surface_size: (u32, u32),
    /// Application name passed to the graphics API.
    pub application_name: String,
    /// Whether backend validation should be enabled.
    pub validation: bool,
    /// Whether the host runs offscreen with no native window.
    ///
    /// When `true` the host runs without a window: with the null backend it
    /// uses [`platform::headless_window_handle`] and its own surface, and
    /// with the Vulkan backend it draws every frame into an offscreen target
    /// of the surface size, which [`Host::read_offscreen_frame`] reads back.
    /// When `false`, [`window_handle`]
    /// must be `Some`; a real backend with no handle is an error.
    ///
    /// [`window_handle`]: HostConfig::window_handle
    pub headless: bool,
    /// Window handle that creates the surface.
    ///
    /// Required unless [`HostConfig::headless`] is set. The windowed host
    /// passes the real platform window handle here so a Vulkan surface can
    /// bind to it.
    pub window_handle: Option<RawWindowHandle>,
    /// Display handle (Wayland/X11/XCB connection) used with `window_handle`.
    ///
    /// `VK_KHR_surface` needs the platform display connection in addition to
    /// the window handle. When this is `Some`, the host creates the surface
    /// through `begin_render_state_with_display`; `None` uses the
    /// window-handle-only entry point (or the null backend).
    pub display_handle: Option<RawDisplayHandle>,
    /// Whether the host draws the debug overlay (frame rate and its 1% low,
    /// host usage, memory and live object counts) in the top-left corner.
    ///
    /// The overlay is a render-only node: it is never a script-visible layer
    /// or bitmap, so it cannot change the scene a script observes.
    pub debug_overlay: bool,
    /// The decoded-image cache policy, or `None` to keep the engine default
    /// (disabled). See [`CacheLimit`].
    pub graphic_cache_limit: Option<CacheLimit>,
    /// Frame period the caller paces its loop at.
    ///
    /// It sets the presentation budget a frame may spend waiting for the
    /// display, so the wait is bounded by the loop's own cadence.
    pub frame_interval: Duration,
    /// Whether the host composites the presented frame on the CPU.
    ///
    /// The null backend composites every presented frame with the software
    /// rasterizer and hands it to the host, so a window can show it and a
    /// file can hold it. The compositing costs a full-frame rasterization,
    /// so it is off by default.
    pub cpu_presentation: bool,
    /// The rendering policy: how the device draws the scene.
    ///
    /// The policy is the one switch the launcher turns. The default draws the
    /// way the software renderer does: every texture is point-sampled. The
    /// policy is a rendering choice and not a script-visible one: the
    /// logical screen and every value a script reads stay where they are.
    pub policy: RenderPolicy,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            backend: BackendKind::Null,
            surface_size: (1920, 1080),
            application_name: "krkr-rs".to_owned(),
            validation: false,
            headless: false,
            window_handle: None,
            display_handle: None,
            debug_overlay: false,
            graphic_cache_limit: None,
            frame_interval: Duration::from_secs_f64(FrameRateLimit::DEFAULT.period_secs()),
            cpu_presentation: false,
            policy: RenderPolicy::default(),
        }
    }
}

impl HostConfig {
    /// Returns the default offscreen configuration.
    ///
    /// This is [`Default`] with [`headless`](HostConfig::headless) set, so a
    /// host can be created without a window or display handle.
    pub fn headless() -> Self {
        Self {
            headless: true,
            ..Self::default()
        }
    }

    /// Returns this configuration with a different backend.
    pub fn with_backend(mut self, backend: BackendKind) -> Self {
        self.backend = backend;
        self
    }

    /// Returns this configuration with a different surface size.
    ///
    /// The size is the framebuffer in pixels: the surface a windowed host
    /// presents to, and the offscreen target a headless host draws into.
    pub fn with_surface_size(mut self, size: (u32, u32)) -> Self {
        self.surface_size = size;
        self
    }

    /// Returns this configuration with the debug overlay enabled or disabled.
    pub fn with_debug_overlay(mut self, debug_overlay: bool) -> Self {
        self.debug_overlay = debug_overlay;
        self
    }

    /// Returns this configuration with a decoded-image cache policy.
    pub fn with_graphic_cache_limit(mut self, limit: Option<CacheLimit>) -> Self {
        self.graphic_cache_limit = limit;
        self
    }

    /// Returns this configuration with the CPU presentation on or off.
    pub fn with_cpu_presentation(mut self, enabled: bool) -> Self {
        self.cpu_presentation = enabled;
        self
    }

    /// Returns this configuration with a different rendering policy.
    pub fn with_policy(mut self, policy: RenderPolicy) -> Self {
        self.policy = policy;
        self
    }
}

/// The assembled host: the middle-layer engine and the frame it owns.
///
/// See the crate documentation for the ownership model. Callers reach the
/// layer system through [`Host::scene`] / [`Host::scene_mut`] and drive
/// frames through [`Host::step`].
pub struct Host {
    /// The middle-layer engine: scene, projection, textures, timers, input
    /// and the frame clock.
    engine: engine::Engine,
    /// The clock source the composition root paces the frames from, when it
    /// attached one.
    ///
    /// [`GameHost::open`](crate::GameHost::open) hands it to the compatibility layer,
    /// where `System.getTickCount` reads it on demand. The pointer is the
    /// caller's: it must be the source every [`Host::step`] is given, and it
    /// must outlive the game that reads it.
    frame_time: Option<*const (dyn FrameTimeSource + 'static)>,
    /// The slot the CPU-presented frame is composited into, when the host
    /// composites on the CPU.
    ///
    /// The slot is installed into the render state before the state is handed
    /// to the engine, so the engine's frames reach it without the engine
    /// knowing about it.
    presented_slot: Option<PresentedFrameSlot>,
}

/// A frame hook that does nothing, for a host with no game above it.
struct EmptyFrame;

impl engine::GameFrame for EmptyFrame {
    fn before_render(&mut self, _now_ms: u64, _dt_ms: f64) -> Result<(), String> {
        Ok(())
    }
}

impl Host {
    /// Creates a host and its surface from `config`.
    ///
    /// The host owns the engine and the render pipeline the engine draws
    /// through; it keeps no file system and no media backend, because it
    /// exercises neither.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Config`] for a non-headless configuration with no
    /// window handle. Returns [`HostError::Render`] when the
    /// requested backend cannot be created, when the surface creation fails,
    /// and when the offscreen target of a headless Vulkan host cannot be
    /// created. A
    /// `--backend vulkan` host reports [`RenderError::Unsupported`] for a
    /// backend this build does not implement, and
    /// [`RenderError::DeviceUnavailable`] when the Vulkan loader, instance or
    /// device cannot be brought up.
    pub fn new(config: HostConfig) -> Result<Self, HostError> {
        // The policy is the one switch the launcher turned: the samplers are
        // created with the pipeline, so the filter is derived from it here,
        // and the same value is handed to the engine.
        let policy = config.policy;
        let pipeline = create_render_pipeline(PipelineConfig {
            backend: config.backend,
            application_name: config.application_name,
            preferred_format: TextureFormat::Rgba8Srgb,
            validation: config.validation,
            filter: policy.texture_filter(),
            frame_interval: config.frame_interval,
        })?;

        // The frames are drawn either into a presentation surface or into an
        // offscreen target. A headless host with the Vulkan backend has no
        // window, so it draws into a target of the surface size; the null
        // backend keeps its headless surface.
        let (target, mut render_state): (FrameTarget, RenderState) = if config.headless
            && config.backend == BackendKind::Vulkan
        {
            let render_state = begin_offscreen_render_state(&pipeline)?;
            let size = (config.surface_size.0.max(1), config.surface_size.1.max(1));
            let target = create_render_target(&pipeline, RenderTargetDesc::new(size.0, size.1))?;
            (FrameTarget::Offscreen(target), render_state)
        } else {
            let headless_handle;
            let handle: &RawWindowHandle = if config.headless {
                headless_handle = platform::headless_window_handle();
                &headless_handle
            } else {
                config.window_handle.as_ref().ok_or_else(|| {
                    HostError::Config(
                        "no window handle; set `headless` for an offscreen host".to_owned(),
                    )
                })?
            };
            let (surface, render_state): (Surface, RenderState) = match &config.display_handle {
                Some(display) => begin_render_state_with_display(&pipeline, display, handle)?,
                None => begin_render_state(&pipeline, handle)?,
            };
            (FrameTarget::Surface(surface), render_state)
        };
        // The CPU presentation is installed into the render state before the
        // state is handed to the engine: the engine presents every frame
        // through that state, and the frame lands in the slot.
        let presented_slot = if config.cpu_presentation {
            let slot = PresentedFrameSlot::new();
            install_presented_frame_slot(&pipeline, &mut render_state, slot.clone())?;
            Some(slot)
        } else {
            None
        };
        let overlay = if config.debug_overlay {
            Some(Overlay::new(&pipeline).map_err(|error| HostError::Overlay(error.to_string()))?)
        } else {
            None
        };

        let mut engine = engine::Engine::new(engine::EngineParts {
            pipeline,
            target,
            render_state,
            surface_size: config.surface_size,
            headless: config.headless,
            overlay: overlay.map(|overlay| Box::new(overlay) as Box<dyn engine::EngineOverlay>),
        })?
        .with_policy(policy);
        if let Some(limit) = config.graphic_cache_limit {
            engine.scene_mut().set_decoded_image_cache_limit(limit);
        }

        Ok(Self {
            engine,
            frame_time: None,
            presented_slot,
        })
    }

    /// Points the script's tick count at the clock source the frames are paced
    /// from.
    ///
    /// This is the source every [`Host::step`] must be given, and it must
    /// outlive the game opened over this host: `System.getTickCount` reads it on
    /// demand, so the tick count moves inside a frame instead of only at a
    /// frame boundary. A host with no source reports the frame clock as its
    /// tick count.
    ///
    /// The pointer is the caller's promise, the same shape the compatibility
    /// layer's `attach_*` takes: the source must stay alive, and must be the one
    /// the host is stepped with, for as long as the game reads it. A composition
    /// root owns its source in a `Box`, so the address does not move.
    pub fn with_frame_time_source(
        mut self,
        source: *const (dyn FrameTimeSource + 'static),
    ) -> Self {
        if source.is_null() {
            self.frame_time = None;
        } else {
            self.frame_time = Some(source);
        }
        self
    }

    /// The clock source the composition root attached, if any.
    pub(crate) fn frame_time_source(&self) -> Option<*const (dyn FrameTimeSource + 'static)> {
        self.frame_time
    }

    /// Advances the host by one frame and presents it.
    ///
    /// The engine drives the whole frame: it fits the script-visible size into
    /// the framebuffer, advances its clock by the elapsed time `clock`
    /// reports, fires due timers, runs the (here empty) frame hook, fits the
    /// presentation again (the hook may have changed a window size), refreshes
    /// the debug overlay, uploads dirty bitmaps, projects the layer tree when
    /// it changed, translates it into device pixels and renders the result.
    ///
    /// # Errors
    ///
    /// Returns [`HostError`] when the frame hook, the overlay or a render
    /// action fails.
    pub fn step(&mut self, clock: &dyn FrameTimeSource) -> Result<StepReport, HostError> {
        self.step_with_frame(clock, &mut EmptyFrame)
    }

    /// Advances the host by one frame, running `hooks` before the projection.
    ///
    /// This is the composition root's entry point: the game host passes its
    /// VM/audio hook so its per-frame work runs between the timer advance and
    /// the projection.
    ///
    /// # Errors
    ///
    /// As [`Host::step`], plus a hook failure reported as
    /// [`HostError::Frame`].
    pub(crate) fn step_with_frame(
        &mut self,
        clock: &dyn FrameTimeSource,
        hooks: &mut dyn engine::GameFrame,
    ) -> Result<StepReport, HostError> {
        Ok(self.engine.run(clock, hooks)?)
    }

    /// The middle-layer engine, for the composition root's attachments and
    /// presentation reads.
    pub(crate) fn engine_mut(&mut self) -> &mut engine::Engine {
        &mut self.engine
    }

    /// The middle-layer engine, for reading the presentation the frame uses.
    pub(crate) fn engine(&self) -> &engine::Engine {
        &self.engine
    }

    /// Reports a new framebuffer size in physical pixels.
    ///
    /// The surface size is the device framebuffer, which is independent of the
    /// script-visible size: the scene keeps its window and layer sizes and the
    /// presentation refits them into the new framebuffer, so an OS resize moves
    /// no value a script observes. A script `Window.setInnerSize` is the other
    /// direction and is surfaced through [`Host::take_window_resize`].
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Render`] if the backend rejects the resize.
    pub fn resize(&mut self, size: (u32, u32)) -> Result<SurfaceEvent, HostError> {
        Ok(self.engine.resize(size)?)
    }

    /// Returns the backend the pipeline was created with.
    pub fn backend(&self) -> BackendKind {
        self.engine.backend()
    }

    /// The host's rendering policy.
    pub fn policy(&self) -> RenderPolicy {
        self.engine.policy()
    }

    /// Returns the layer tree.
    pub fn scene(&self) -> &Scene {
        self.engine.scene()
    }

    /// Returns the layer tree for script-driven mutation.
    ///
    /// A field written through this mutable handle does **not** bump the
    /// scene's structure revision, so the next frame reuses the projected tree
    /// and the change is not drawn. A change the projection must see goes
    /// through the scene's own mutable accessors, which bump the revision.
    pub fn scene_mut(&mut self) -> &mut Scene {
        self.engine.scene_mut()
    }

    /// Returns the most recently projected render tree.
    pub fn render_scene(&self) -> &RenderScene {
        self.engine.render_scene()
    }

    /// Returns the bitmap-id → texture table.
    pub fn textures(&self) -> &engine::Map<u32, Texture> {
        self.engine.textures()
    }

    /// Whether this host runs offscreen with no window.
    ///
    /// Mirrors [`HostConfig::headless`]: the game host uses it to select the
    /// L1 null audio backend instead of opening a real device.
    pub fn is_headless(&self) -> bool {
        self.engine.is_headless()
    }

    /// Returns the current surface size in pixels.
    pub fn surface_size(&self) -> (u32, u32) {
        self.engine.surface_size()
    }

    /// Returns the VM-pixel → device-pixel mapping of the presented frame, or
    /// `None` when the script-visible size cannot be fitted into the
    /// framebuffer.
    pub fn translation(&self) -> Option<engine::PixelTranslation> {
        self.engine.translation()
    }

    /// Reads the frame the last frame drew into the offscreen target.
    ///
    /// The pixels are RGBA8 in device pixels: the target holds the picture
    /// the projected tree was drawn into, at the host's surface size. A host
    /// that presents to a surface has no target, so it reads nothing.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Render`] when the read-back fails, which is the
    /// case when no frame has been drawn into the target yet.
    pub fn read_offscreen_frame(&self) -> Result<Option<Vec<u8>>, HostError> {
        Ok(self.engine.read_offscreen_frame()?)
    }

    /// Returns the logical screen size the presentation fits into the window:
    /// the first scene window's primary layer when it has one, and the
    /// framebuffer size otherwise.
    pub fn vm_size(&self) -> (u32, u32) {
        self.engine.vm_size()
    }

    /// Takes the script-side window size the platform window still has to
    /// adopt, if any.
    ///
    /// The engine produces a request when the script creates a window whose
    /// client size differs from the framebuffer and whenever that size changes
    /// afterwards. Taking it clears it, so a size that does not change again is
    /// applied once.
    pub fn take_window_resize(&mut self) -> Option<(u32, u32)> {
        self.engine.take_window_resize()
    }

    /// Whether the host draws the debug overlay.
    pub fn debug_overlay_enabled(&self) -> bool {
        self.engine.debug_overlay_enabled()
    }

    /// Returns the number of frames rendered so far.
    pub fn frame_index(&self) -> u64 {
        self.engine.frame_index()
    }

    /// Returns the frame clock of the most recent frame.
    pub fn elapsed_ms(&self) -> f64 {
        self.engine.elapsed_ms()
    }

    /// Returns the presentation calls' costs for the last presented frame.
    ///
    /// The Vulkan backend reports the acquire/present/fence/recreate costs it
    /// measured; the null backend reports zeros. `None` means no frame has
    /// been presented through this state yet.
    pub fn last_frame_timings(&self) -> Option<FrameTimings> {
        self.engine.last_frame_timings()
    }

    /// Calls `visit` with the CPU-presented frame, if the host composites on
    /// the CPU and a frame has been presented.
    ///
    /// The frame is RGBA8 and holds the composed picture in device pixels:
    /// it is what the window shows when the null backend presents it.
    pub fn with_presented_frame<R>(&self, visit: impl FnOnce(&PresentedFrame) -> R) -> Option<R> {
        self.presented_slot
            .as_ref()
            .and_then(|slot| slot.with_frame(visit))
    }
}

#[cfg(test)]
mod tests {
    use super::{BackendKind, Host, HostConfig, HostError, InjectedFrameTime};
    use api::blend::BlendMode;
    use api::error::RenderError;
    use engine::Rect;

    /// The size of the frame the host draws.
    const SIZE: (u32, u32) = (64, 48);

    /// The colour of the layer that covers the frame.
    const COLOUR: [u8; 4] = [24, 28, 40, 255];

    /// Fills the host's scene with one opaque layer that covers the frame.
    fn fill_scene(host: &mut Host) {
        let scene = host.scene_mut();
        let window = scene.add_window("test", SIZE);
        let layer = scene.add_layer(window, None);
        let pixels = COLOUR.repeat((SIZE.0 * SIZE.1) as usize);
        let bitmap = scene.add_bitmap(SIZE.0, SIZE.1, pixels);
        if let Some(layer) = scene.layer_mut(layer) {
            layer.visible = true;
            layer.rect = Rect::new(0, 0, SIZE.0, SIZE.1);
            layer.bitmap = Some(bitmap);
            layer.image_width = SIZE.0;
            layer.image_height = SIZE.1;
            layer.blend = BlendMode::Replace;
        }
    }

    /// The headless Vulkan host draws its frame into an offscreen target.
    ///
    /// The host has no window, so the frame is drawn into a target of the
    /// surface size and read back, on a machine that has a GPU but no
    /// display.
    ///
    /// A machine without a Vulkan device has no GPU path to measure, so the
    /// test skips there, exactly like the Vulkan cases in the render
    /// backend's own tests.
    #[test]
    fn the_headless_vulkan_host_draws_into_an_offscreen_target() {
        let config = HostConfig::headless()
            .with_backend(BackendKind::Vulkan)
            .with_surface_size(SIZE);
        let mut host = match Host::new(config) {
            Ok(host) => host,
            Err(HostError::Render(RenderError::DeviceUnavailable { .. }))
            | Err(HostError::Render(RenderError::Unsupported { .. })) => return,
            Err(error) => panic!("the headless Vulkan host: {error}"),
        };
        fill_scene(&mut host);

        let report = host
            .step(&InjectedFrameTime::new(16.0))
            .expect("the frame runs");
        assert_eq!(report.frame_index, 1);
        assert!(report.presented, "the frame reached the target");

        let pixels = host
            .read_offscreen_frame()
            .expect("the read-back succeeds")
            .expect("the host draws into a target");
        assert_eq!(pixels.len(), (SIZE.0 * SIZE.1 * 4) as usize);
        // The GPU path samples the texture in the target's own colour space,
        // so the round trip is within a level or two of the layer's colour.
        for (channel, expected) in pixels[0..4].iter().zip(COLOUR) {
            assert!(
                channel.abs_diff(expected) <= 2,
                "the target holds the picture the frame drew: {pixels:?}"
            );
        }
    }
}
