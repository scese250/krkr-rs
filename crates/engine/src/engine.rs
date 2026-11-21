//! The frame-owning engine object: layer tree, projection, textures, timers.
//!
//! [`Engine`] owns every evolving value the middle layer needs — the layer
//! [`Scene`], the projected [`RenderScene`], the [`Surface`]/[`RenderState`]
//! pair, the bitmap-id → [`Texture`] table, the [`TimerState`] and the raw
//! input snapshot — and advances them one frame at a time.
//!
//! One frame is: get the clock, fit the VM→device presentation, fire due
//! timers, run the upward hook, fit the presentation again (the hook may have
//! resized a window's primary layer), refresh the render-only overlay, upload
//! dirty bitmaps, project the layer tree when it changed, run the
//! after-projection hook with the emitted layer ids, reproject if that hook
//! changed the scene, carry the VM→device presentation on the render scene
//! root, and draw it with a single pipeline call. A size pair that cannot be
//! fitted skips everything that draws.
//!
//! The frame is drawn into the target the host gave the engine: a surface to
//! present to, or an offscreen target a host with no window reads back.
//!
//! Nothing here reads a wall clock. [`Engine::run`] takes the elapsed
//! time from the caller's clock source, [`Engine::frame`] takes the absolute
//! time and [`Engine::step`] adds a delta to the clock the engine already
//! holds, so a test drives exactly the same code path as the running game.
//!
//! The two traits are the only places where behaviour from higher layers
//! enters: [`GameFrame`] is the per-frame upward edge implemented above this
//! layer, and [`EngineOverlay`] supplies the render-only debug overlay. This
//! module never names those layers.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use api::blend::BlendMode;
use api::error::RenderError;
use api::geometry::PixelRegion;
use api::host::{PresentSkip, Presentation, SurfaceEvent};
use render::{
    BackendKind, FrameStats, FrameTimings, RenderNode, RenderPipeline, RenderScene, RenderState,
    RenderTarget, RenderTargetDesc, Surface, Texture, TextureDesc, TextureFilter, TextureUpload,
    UploadTimings, create_render_target, create_texture, destroy_render_target, drop_texture,
    last_frame_timings, last_upload_timings, read_render_target, recover_surface, render,
    render_to_target, resize as resize_render, supports_effect_pipelines, upload_textures,
};

use crate::clock::{FrameClock, FrameTimeSource, InjectedFrameTime};
use crate::hasher::{Map, Set, map, set};
use crate::imagedecode::{DecodePool, ImageDecoder};
use crate::jobpool::{JobPoolConfig, JobPoolError, JobStats};
use crate::projection::{ProjectionReport, project_scene_report};
use crate::scene::{BitmapState, Scene};
use crate::shader::{PipelineId, Program, ProgramDesc, ShaderError, ShaderId, ShaderLibrary};
use crate::storage::{StoragePoolConfig, StoragePoolError, StorageReadPool, StorageReader};
use crate::timer::{TimerState, deliver_due};
use crate::translate::{PixelTranslation, TranslateError};

/// The engine's rendering policy.
///
/// The policy says how the device draws the scene. It is the one switch the
/// composition root turns: the plain rendering, or the experimental
/// native-resolution rendering.
///
/// The policy is a rendering choice and not a script-visible one: every value
/// a script reads stays in VM pixels, and the logical screen does not move.
/// What it changes is the picture the device draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderPolicy {
    /// The plain rendering: every texture is point-sampled.
    ///
    /// The software renderer starts at the nearest-neighbour stretch type and
    /// selects the point copy for anything below the fast-linear threshold,
    /// so a magnified layer is point-scaled.
    #[default]
    Reference,
    /// The experimental native-resolution rendering: every texture is read
    /// with a bilinear filter.
    ///
    /// This is the fast-linear mode the software renderer also has: it
    /// exposes the mode as a render-manager parameter and does not default to
    /// it. The picture the device draws is smoother; the values a script
    /// reads do not move.
    NativeResolution,
}

impl RenderPolicy {
    /// The filter the samplers read textures with.
    ///
    /// The samplers are device resources created with the device, so the
    /// mode is chosen when the pipeline is created; the policy is what the
    /// composition root derives it from.
    pub fn texture_filter(self) -> TextureFilter {
        match self {
            Self::Reference => TextureFilter::Point,
            Self::NativeResolution => TextureFilter::Linear,
        }
    }
}

/// Where the engine's frames are drawn.
///
/// A host with a window presents every frame to a surface; a host with no
/// window draws the frame into an offscreen target, which the caller reads
/// back. The two need different resources, so the engine holds one of them
/// and never both.
pub enum FrameTarget {
    /// The frame is presented to a presentation surface.
    Surface(Surface),
    /// The frame is drawn into an offscreen target of the engine's surface
    /// size.
    Offscreen(RenderTarget),
}

/// The bottom objects and initial state [`Engine::new`] assembles into an
/// [`Engine`].
///
/// The caller creates these itself: the render pipeline comes from the
/// backend selection, the frame target and the render state from the window
/// handle or from the offscreen entry point, and the overlay from whatever
/// higher layer draws one.
pub struct EngineParts {
    /// Stateless render service.
    pub pipeline: RenderPipeline,
    /// Where the frames are drawn: a surface to present to, or an offscreen
    /// target to draw into.
    pub target: FrameTarget,
    /// GPU/staging/texture bookkeeping for the target.
    pub render_state: RenderState,
    /// Initial surface size in pixels; clamped to at least `1x1`.
    pub surface_size: (u32, u32),
    /// Whether the engine runs offscreen with no window.
    pub headless: bool,
    /// Render-only overlay drawn above every layer, or `None` for none.
    pub overlay: Option<Box<dyn EngineOverlay>>,
}

/// Live object counts the engine exposes to a render-only overlay.
///
/// These are the engine's own tables, not script handles: the counts follow
/// the scene and texture lifetime and are refreshed with the overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectCounts {
    /// Windows in the scene.
    pub windows: usize,
    /// Layers in the scene.
    pub layers: usize,
    /// Bitmaps in the scene.
    pub bitmaps: usize,
    /// Fonts in the scene.
    pub fonts: usize,
    /// Uploaded bitmap textures the engine currently holds.
    pub textures: usize,
}

/// Behaviour the engine calls once per frame before the frame is projected.
///
/// This is the single upward edge of the frame: the layer above runs its VM
/// tick and its per-frame event dispatch here and returns the failure as a
/// message. The engine projects and draws whatever the hook left in the scene.
pub trait GameFrame {
    /// Runs one frame body worth of work.
    ///
    /// `now_ms` is the frame clock and `dt_ms` the delta since the previous
    /// frame (zero on the first [`Engine::frame`] call).
    ///
    /// # Errors
    ///
    /// The message is returned to the caller as [`EngineError::Frame`]; the
    /// frame is abandoned without projecting or drawing.
    fn before_render(&mut self, now_ms: u64, dt_ms: f64) -> Result<(), String>;

    /// Runs after the projection emitted this frame's layers, before the
    /// final upload, projection and draw.
    ///
    /// `drawn_layers` is the projection's report for the current scene. A hook
    /// that changes the scene here is followed by a second upload and
    /// projection, so its changes are in the frame being drawn.
    ///
    /// # Errors
    ///
    /// The message is returned to the caller as [`EngineError::Frame`]; the
    /// frame is abandoned without drawing.
    fn after_project(&mut self, _drawn_layers: &[u32]) -> Result<(), String> {
        Ok(())
    }
}

/// A render-only overlay appended above every projected layer.
///
/// The overlay is not part of the engine [`Scene`] and owns no bitmap a script
/// could reach, so it cannot change the layer tree. The engine refreshes it
/// once per frame and attaches [`EngineOverlay::node`] to the render tree on
/// every projection.
pub trait EngineOverlay {
    /// Refreshes the overlay's contents before the frame is projected.
    ///
    /// `now_ms` is the frame clock; the overlay decides its own refresh
    /// cadence from it.
    ///
    /// # Errors
    ///
    /// The message is returned to the caller as [`EngineError::Overlay`]; the
    /// frame is abandoned without projecting or drawing.
    fn refresh(
        &mut self,
        pipeline: &RenderPipeline,
        state: &mut RenderState,
        now_ms: u64,
        objects: ObjectCounts,
    ) -> Result<(), String>;

    /// Returns the overlay node, in device/framebuffer coordinates.
    ///
    /// The engine sets its parent to the render scene root and replaces its
    /// transform with the inverse of the presentation root transform, so the
    /// node's `dst` and rectangle clip stay in device pixels.
    fn node(&self) -> RenderNode;
}

/// Errors surfaced by [`Engine`] frames and surface actions.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// A bottom-layer render action failed.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// The overlay could not be refreshed.
    #[error("engine overlay: {0}")]
    Overlay(String),
    /// The frame hook reported a failure.
    #[error("engine frame: {0}")]
    Frame(String),
}

/// A frame the backend presented, or the reason it did not.
///
/// A backend that presents nothing but reports no error leaves the previous
/// surface contents on screen, so the frame is counted as skipped and reported
/// instead of being treated as an ordinary frame. A skip is reported without a
/// retry: nothing was submitted, so there is nothing to retry until the next
/// frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PresentOutcome {
    /// Whether the backend presented this frame.
    presented: bool,
    /// The surface event the backend reported, if any.
    surface_event: Option<SurfaceEvent>,
    /// The fault to report when the frame was not presented.
    fault: Option<PresentFault>,
}

/// The reason a frame was not presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresentFault {
    /// The backend presented nothing and named no surface event.
    Unreported,
    /// The backend named the surface event that stopped presentation.
    Surface(SurfaceEvent),
    /// The backend abandoned the frame because the display was not ready.
    Skipped(PresentSkip),
}

/// Reduces a backend presentation to the outcome the frame reports.
fn present_outcome(presentation: Presentation) -> PresentOutcome {
    PresentOutcome {
        presented: presentation.presented,
        surface_event: presentation.surface_event,
        fault: if presentation.presented {
            None
        } else {
            Some(match (presentation.skip, presentation.surface_event) {
                // A skip means the backend abandoned the frame before any GPU
                // work, so it outranks any surface event it may also carry.
                (Some(skip), _) => PresentFault::Skipped(skip),
                (None, Some(event)) => PresentFault::Surface(event),
                (None, None) => PresentFault::Unreported,
            })
        },
    }
}

/// How long a presentation stall may last before its age is reported.
const STALL_REPORT_MS: u64 = 1_000;

/// How often a continuing presentation stall reports its age after the first
/// report.
const STALL_REPORT_INTERVAL_MS: u64 = 10_000;

/// A reason a frame's image did not reach the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StallCause {
    /// The backend did not present the frame.
    Backend(PresentFault),
    /// The game surface and the framebuffer have no presentation mapping, so
    /// the frame drew nothing.
    NoMapping,
}

impl StallCause {
    /// Warns once about this cause when it opens or joins a stall.
    fn warn(self) {
        match self {
            Self::Backend(PresentFault::Unreported) => {
                log::warn!(
                    "engine.frame: the backend presented no frame and reported no surface event"
                )
            }
            // A skipped frame is not a fault: the backend abandoned it before
            // submitting anything and the previous image stays on screen.
            Self::Backend(PresentFault::Skipped(PresentSkip::AcquireTimeout)) => log::warn!(
                "engine.frame: the display did not release a swapchain image within the \
                 presentation budget; this frame was skipped"
            ),
            Self::Backend(PresentFault::Surface(SurfaceEvent::Lost)) => log::warn!(
                "engine.frame: the presentation surface was lost; this frame was not presented"
            ),
            Self::Backend(PresentFault::Surface(SurfaceEvent::OutOfDate)) => log::warn!(
                "engine.frame: the presentation surface is out of date; rebuilding the swapchain"
            ),
            Self::Backend(PresentFault::Surface(SurfaceEvent::Restored)) => {
                log::warn!(
                    "engine.frame: the surface was restored but this frame was not presented"
                )
            }
            Self::Backend(PresentFault::Surface(SurfaceEvent::Resized(width, height))) => {
                log::warn!(
                    "engine.frame: this frame was not presented after a resize to size={width}x{height}"
                )
            }
            // The unfittable-size warning is emitted where the mapping is
            // rebuilt, once per unsupported size pair.
            Self::NoMapping => {}
        }
    }

    /// The phrase a stall summary uses for this cause.
    fn describe(self) -> String {
        match self {
            Self::Backend(PresentFault::Unreported) => {
                "the backend presented no frame and named no surface event".to_owned()
            }
            Self::Backend(PresentFault::Skipped(PresentSkip::AcquireTimeout)) => {
                "the display did not release a swapchain image within the presentation budget"
                    .to_owned()
            }
            Self::Backend(PresentFault::Surface(SurfaceEvent::Lost)) => {
                "the presentation surface was lost".to_owned()
            }
            Self::Backend(PresentFault::Surface(SurfaceEvent::OutOfDate)) => {
                "the presentation surface was out of date".to_owned()
            }
            Self::Backend(PresentFault::Surface(SurfaceEvent::Restored)) => {
                "the presentation surface was restored".to_owned()
            }
            Self::Backend(PresentFault::Surface(SurfaceEvent::Resized(width, height))) => {
                format!("the presentation surface was resized to {width}x{height}")
            }
            Self::NoMapping => "the game surface and the framebuffer cannot be fitted".to_owned(),
        }
    }
}

/// Joins the distinct causes of a stall into one log phrase.
fn describe_stall_causes(causes: &[StallCause]) -> String {
    causes
        .iter()
        .map(|cause| cause.describe())
        .collect::<Vec<String>>()
        .join("; ")
}

/// One run of consecutive frames the display did not accept.
///
/// The stall starts with the first frame whose image did not reach the display
/// and ends with the next presented frame. It keeps the distinct causes, so a
/// resume names every reason the display was starved rather than only the last
/// one, and it reports its own age while it lasts, so a display that stops
/// accepting frames is never silent between the first warning and the resume.
#[derive(Default)]
struct PresentationStall {
    /// Frames the display did not accept since the last presented frame.
    frames: u64,
    /// The frame clock the stall began at.
    started_ms: u64,
    /// Distinct causes in first-seen order.
    causes: Vec<StallCause>,
    /// The frame clock of the next age report.
    next_report_ms: Option<u64>,
}

impl PresentationStall {
    /// Whether the run has recorded a frame.
    fn active(&self) -> bool {
        self.frames > 0
    }

    /// Records a frame the display did not accept, returning the cause when it
    /// is new to this stall.
    fn record(&mut self, cause: StallCause, now_ms: u64) -> Option<StallCause> {
        if !self.active() {
            self.started_ms = now_ms;
            self.next_report_ms = Some(now_ms.saturating_add(STALL_REPORT_MS));
        }
        self.frames = self.frames.saturating_add(1);
        if self.causes.contains(&cause) {
            return None;
        }
        self.causes.push(cause);
        Some(cause)
    }

    /// Returns the stall's age when a report is due at `now_ms`.
    fn due_report(&mut self, now_ms: u64) -> Option<StallAge> {
        let due = self.next_report_ms?;
        if now_ms < due {
            return None;
        }
        self.next_report_ms = Some(now_ms.saturating_add(STALL_REPORT_INTERVAL_MS));
        Some(StallAge {
            frames: self.frames,
            elapsed_ms: now_ms.saturating_sub(self.started_ms),
        })
    }

    /// Ends the run at `now_ms`, returning what to report when it lasted.
    fn resumed(&mut self, now_ms: u64) -> Option<StallSummary> {
        if !self.active() {
            return None;
        }
        let summary = StallSummary {
            frames: self.frames,
            elapsed_ms: now_ms.saturating_sub(self.started_ms),
            causes: std::mem::take(&mut self.causes),
        };
        self.frames = 0;
        self.next_report_ms = None;
        Some(summary)
    }
}

/// The age of a presentation stall that is still running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StallAge {
    /// Frames the display did not accept so far.
    frames: u64,
    /// Milliseconds since the stall began.
    elapsed_ms: u64,
}

/// The span of a presentation stall that ended.
#[derive(Debug, PartialEq, Eq)]
struct StallSummary {
    /// Frames the display did not accept.
    frames: u64,
    /// Milliseconds between the first skipped frame and the resumed one.
    elapsed_ms: u64,
    /// Distinct causes in first-seen order.
    causes: Vec<StallCause>,
}

/// Outcome of one [`Engine::step`] or [`Engine::frame`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameReport {
    /// Number of frames drawn so far, counting this one.
    pub frame_index: u64,
    /// The frame clock this frame ran at, in milliseconds.
    ///
    /// A clock advanced with [`Engine::run`] or [`Engine::step`] keeps its
    /// accumulated fractional part; [`Engine::frame`] sets it to the injected
    /// whole millisecond.
    pub elapsed_ms: f64,
    /// Whether the frame reached its destination.
    ///
    /// A frame drawn into an offscreen target has no surface to be presented
    /// to: it reaches the target, which holds it until the next frame changes
    /// it. A frame with `presented == false` updated the scene but the backend
    /// kept the previous surface contents; the engine reports and retries it
    /// instead of treating it as an ordinary frame.
    pub presented: bool,
    /// Per-frame counters from the null backend, or `None` on a GPU backend.
    pub stats: Option<FrameStats>,
    /// Milliseconds spent syncing dirty bitmaps to textures.
    pub sync_ms: f64,
    /// Milliseconds of `sync_ms` spent uploading dirty bitmaps.
    pub upload_ms: f64,
    /// Milliseconds of `sync_ms` spent dropping textures whose bitmap is gone.
    pub prune_ms: f64,
    /// Milliseconds spent projecting the scene tree.
    pub project_ms: f64,
    /// Milliseconds spent in the pipeline's render call.
    pub render_ms: f64,
    /// The costs of the frame's texture uploads, when the backend measured
    /// them.
    ///
    /// The value names the four steps of an upload: the GPU images the
    /// uploads write into are created, the pixels are written into the mapped
    /// staging memory, the copy command is recorded and submitted, and the
    /// upload waits for the GPU when it reuses a staging buffer. A backend
    /// that measures nothing reports `None`.
    pub upload_timings: Option<UploadTimings>,
}

/// The middle-layer engine: scene, projection, textures, timers, presentation
/// and the frame.
///
/// Every value is a field of this object; there are no globals, statics or
/// hidden caches, so two engines can run in one process. Callers reach the
/// layer system through [`Engine::scene`] / [`Engine::scene_mut`] and drive
/// frames through [`Engine::run`] / [`Engine::step`] / [`Engine::frame`].
pub struct Engine {
    /// The device the frame draws through.
    ///
    /// The device owns the backend's bookkeeping for this caller: the render
    /// state every render action takes, the pipeline that draws with it, the
    /// uploaded textures, and the surfaces the device composites are drawn
    /// into.
    ///
    /// The device belongs to the engine and to the frame thread: the render
    /// state holds a backend handle, so the device never leaves the thread
    /// that owns the frame. The compatibility layer reaches it through the
    /// scene's device requests, which the engine runs with it.
    device: Device,
    /// Layer tree owned by the engine.
    scene: Scene,
    /// The registered shader programs and the pipelines built from them.
    shader: ShaderLibrary,
    /// Projected render tree, rebuilt in place when
    /// [`Scene::structure_revision`] changes.
    render_scene: RenderScene,
    /// [`Scene::structure_revision`] value `render_scene` was projected from,
    /// or `None` when the projection must run regardless.
    ///
    /// `None` until the first projection, so the first frame always projects;
    /// the presentation mapping and the texture table are the other two
    /// projection inputs, and a change to either resets it to `None`.
    render_scene_revision: Option<u64>,
    /// Layer ids the latest projection emitted, including descendants covered
    /// by a cached ancestor.
    drawn_layers: Vec<u32>,
    /// Presented surface for `render_state`, or the offscreen target the
    /// frame is drawn into when the host has no window.
    target: FrameTarget,
    /// Timer registry advanced by the frame clock.
    timer: TimerState,
    /// The image decode pool, or `None` when no decoder is installed.
    ///
    /// The pool is the engine's own state: it owns the worker threads, the
    /// queue and the byte accounting. It is absent until the composition
    /// root installs a decoder, and the frame's image barrier is a no-op
    /// while it is.
    decode_pool: Option<DecodePool>,
    /// The storage read pool, or `None` when no reader is installed.
    ///
    /// The pool is the engine's own state: it owns the worker threads, the
    /// queue and the reads in flight. The composition root installs it and
    /// hands it to the storage the script reads through, and the frame
    /// drains it before the hook runs.
    storage_pool: Option<Arc<StorageReadPool>>,
    /// Latest script-visible input state.
    input: crate::input::InputState,
    /// The text service: the registered faces, the shaping database and the
    /// per-character caches.
    ///
    /// The engine owns it, so every text consumer (the compatibility layer,
    /// a host that draws text itself) reaches the same faces through the same
    /// registry.
    text: crate::text::TextEngine,
    /// Current surface size in pixels.
    surface_size: (u32, u32),
    /// The frame clock of the previous frame, or `None` before the first one.
    last_now_ms: Option<u64>,
    /// Frames drawn so far.
    frame_index: u64,
    /// The frame clock: the time base the frame advances.
    ///
    /// `frame` sets it to the injected whole millisecond and `step`
    /// accumulates each delta, so sub-millisecond frame times add up instead
    /// of being discarded. Its whole-millisecond part is the `now_ms` the
    /// timer, hook and overlay receive.
    clock: FrameClock,
    /// Whether the engine runs offscreen with no window.
    headless: bool,
    /// Render-only overlay, or `None` when none is drawn.
    overlay: Option<Box<dyn EngineOverlay>>,
    /// The layers whose pose the CPU draws, by layer id.
    ///
    /// A backend that cannot draw a layer's pose draws it on the CPU, and
    /// the layer keeps drawing it there: its image holds the pose the CPU
    /// pass drew, so a pose surface over it would draw the pose twice.
    cpu_pose_layers: BTreeSet<u32>,
    /// The VM-pixel → device-pixel mapping for the current sizes, or `None` when
    /// the two sizes cannot be fitted.
    translation: Option<PixelTranslation>,
    /// The first window's client size the most recent presentation sync
    /// observed, or `None` when the scene had no window.
    last_window_client_size: Option<(u32, u32)>,
    /// A script-side window size the composition root has not applied yet.
    pending_window_resize: Option<(u32, u32)>,
    /// Whether the unfittable-size warning has been logged since the sizes
    /// last fitted.
    presentation_skipped_logged: bool,
    /// The run of frames the display has not accepted, ended by the next
    /// presented frame.
    stall: PresentationStall,
    /// The engine's rendering policy: how the device draws the scene.
    policy: RenderPolicy,
}

/// The device the engine draws through.
///
/// The device owns the backend's bookkeeping for this caller: the render
/// state every render action takes, the pipeline that draws with it, the
/// uploaded textures, and the surfaces the device composites are drawn into.
///
/// The device belongs to the engine, and the engine's frame thread is the
/// only thread that touches it: the render state holds a backend handle, so
/// the device never leaves the thread that owns the frame. The compatibility
/// layer reaches the device through the scene's device requests, which the
/// engine runs with it.
pub struct Device {
    /// The backend's bookkeeping for this caller.
    pub state: RenderState,
    /// The pipeline that draws with the state.
    pub pipeline: RenderPipeline,
    /// The uploaded bitmap textures, by bitmap id.
    ///
    /// The frame uploads the dirty bitmaps into these textures, and the
    /// device composite samples the sources from them.
    pub textures: Map<u32, Texture>,
    /// The surfaces the device composites are drawn into, by layer id.
    ///
    /// A surface is the size of the layer's content, and it is kept between
    /// the frames: the content's size rarely changes, and a surface that
    /// followed it would be created and destroyed every frame.
    pub surfaces: Map<u32, DeviceSurface>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Device")
            .field("textures", &self.textures.len())
            .field("surfaces", &self.surfaces.len())
            .finish_non_exhaustive()
    }
}

/// The surface a layer's device composite is drawn into.
///
/// The surface holds the composite's picture in the premultiplied form the
/// backend's composite works in; the read-back brings it into the
/// straight-alpha form the content bitmap holds.
pub struct DeviceSurface {
    /// The surface the composite is drawn into.
    pub target: RenderTarget,
    /// The width of the surface in pixels.
    pub width: u32,
    /// The height of the surface in pixels.
    pub height: u32,
}

impl std::fmt::Debug for DeviceSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceSurface")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// The device composite of a layer's subtree: the subtree's picture drawn on
/// the device.
///
/// The CPU composite ([`crate::layer_ops::render_layer_subtree`]) blends
/// every draw of the tree pixel by pixel on the CPU, and that per-pixel work
/// is what the frame tail pays when a transition asks for its two endpoints.
/// This path draws the same tree on the device and reads the result back into
/// the layer's content bitmap, so the per-pixel work happens in the
/// backend's shaders.
///
/// The CPU composite stays the exact path: it is the fallback for every tree
/// this path refuses to draw, and the two pictures differ by the backend's
/// blend arithmetic, the same tolerance the pose's device path has.
///
/// # Errors
///
/// Returns [`RenderError::Unsupported`] when the tree holds a draw this path
/// cannot reproduce on the device — a coverage plane, an opaque or
/// additive-alpha face, a scaled image, a copy that takes the source's
/// alpha — and the backend's own errors otherwise. A refused tree is not a
/// failure: the caller falls back to the CPU composite.
pub fn composite_subtree_on_device(
    device: &mut Device,
    scene: &mut Scene,
    root: u32,
) -> Result<Option<u32>, RenderError> {
    let Some((width, height, trivial)) = crate::layer_ops::subtree_composite_size(scene, root)
    else {
        return Ok(None);
    };
    if trivial {
        // The trivial subtree's content is the layer's own image: there is
        // nothing to composite, and the surface would be an empty picture.
        return Ok(None);
    }
    let target = device_surface(device, root, width, height)?;
    let textures = &mut device.textures;
    crate::layer_ops::composite_subtree_on_device(
        scene,
        root,
        &device.pipeline,
        &mut device.state,
        &target,
        &mut |id| textures.get(&id).cloned(),
    )
}

/// Releases the surface a layer's device composite drew into.
///
/// The surface is gone: the layer has no composite to draw, and the device
/// holds no surface for it.
pub fn release_device_surface(device: &mut Device, layer: u32) -> bool {
    let Some(surface) = device.surfaces.remove(&layer) else {
        return false;
    };
    destroy_render_target(&device.pipeline, &mut device.state, surface.target);
    true
}

/// The surface a layer's device composite is drawn into.
///
/// The surface is the size of the layer's content, and it is kept between the
/// frames: the content's size rarely changes, and a surface of another size
/// is replaced, so the surface always holds the content's whole picture.
fn device_surface(
    device: &mut Device,
    layer: u32,
    width: u32,
    height: u32,
) -> Result<RenderTarget, RenderError> {
    if let Some(surface) = device.surfaces.get(&layer)
        && surface.width == width
        && surface.height == height
    {
        return Ok(surface.target.clone());
    }
    if let Some(surface) = device.surfaces.remove(&layer) {
        destroy_render_target(&device.pipeline, &mut device.state, surface.target);
    }
    let target = create_render_target(&device.pipeline, RenderTargetDesc::new(width, height))?;
    device.surfaces.insert(
        layer,
        DeviceSurface {
            target: target.clone(),
            width,
            height,
        },
    );
    Ok(target)
}

impl Engine {
    /// Creates an engine from ready-made bottom objects.
    ///
    /// The surface is sized to [`EngineParts::surface_size`] (clamped to at
    /// least `1x1`), which is the one surface action the engine performs
    /// before its first frame. An offscreen target is created at that size by
    /// the caller, and the engine refuses a target of another size.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Render`] when the backend rejects the initial
    /// surface size, and when the offscreen target does not match the surface
    /// size.
    pub fn new(parts: EngineParts) -> Result<Self, EngineError> {
        let EngineParts {
            pipeline,
            target,
            render_state,
            surface_size,
            headless,
            overlay,
        } = parts;
        let surface_size = (surface_size.0.max(1), surface_size.1.max(1));
        let mut scene = Scene::default();
        // The engine draws the layers' draw lists on the backend, so the
        // compatibility layer leaves them to it and the list is drawn once.
        scene.host_draws_lists = true;
        // The scene belongs to an engine that draws through a device: the
        // engine owns the device and runs the scene's device requests.
        scene.device_present = true;
        let mut engine = Self {
            device: Device {
                state: render_state,
                pipeline,
                textures: map(),
                surfaces: map(),
            },
            scene,
            shader: ShaderLibrary::new(),
            render_scene: RenderScene::with_root_node(),
            render_scene_revision: None,
            drawn_layers: Vec::new(),
            target,
            timer: TimerState::new(),
            decode_pool: None,
            storage_pool: None,
            input: crate::input::InputState::default(),
            text: crate::text::TextEngine::new(),
            surface_size,
            last_now_ms: None,
            frame_index: 0,
            clock: FrameClock::new(),
            headless,
            overlay,
            cpu_pose_layers: BTreeSet::new(),
            translation: None,
            last_window_client_size: None,
            pending_window_resize: None,
            presentation_skipped_logged: false,
            stall: PresentationStall::default(),
            policy: RenderPolicy::default(),
        };
        // The surface is sized to the requested size; an offscreen target is
        // created at that size by the caller, so it is only checked.
        match &engine.target {
            FrameTarget::Surface(surface) => {
                let event = resize_render(
                    &engine.device.pipeline,
                    surface,
                    &mut engine.device.state,
                    surface_size,
                )?;
                // The surface can adopt an extent different from the requested
                // one (a DPI or compositor clamp), so the presentation fits the
                // extent the backend actually took rather than the size we
                // asked for.
                engine.surface_size = reported_surface_size(event, surface_size);
            }
            FrameTarget::Offscreen(target) => {
                // The target holds the frame, so it must be the size the
                // frame is fitted into: a target of another size would crop
                // the picture the presentation maps.
                if (target.width(), target.height()) != surface_size {
                    return Err(RenderError::InvalidArgument {
                        message: format!(
                            "the offscreen target is {}x{} but the frame is fitted into {}x{}",
                            target.width(),
                            target.height(),
                            surface_size.0,
                            surface_size.1
                        ),
                    }
                    .into());
                }
            }
        }
        engine.sync_presentation();
        engine.install_effect_pipelines();
        // The first frame draws: the surface holds nothing yet.
        engine.scene.mark_dirty();
        Ok(engine)
    }

    /// Returns this engine with a different rendering policy.
    ///
    /// The policy is fixed here: the samplers that carry it are device
    /// resources, created with the pipeline, so a policy that could change
    /// later could disagree with the sampler the device actually uses. The
    /// composition root builds the policy once, derives the sampler's filter
    /// from it and hands the same value here.
    pub fn with_policy(mut self, policy: RenderPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The engine's rendering policy.
    pub fn policy(&self) -> RenderPolicy {
        self.policy
    }

    /// Creates the pipelines for the programs the engine ships.
    ///
    /// The choice is the backend's: a backend that cannot run effect
    /// pipelines gets none, and the transitions blend their display bitmap on
    /// the CPU, the way they always have. A backend that can, but refuses one
    /// of the programs, is reported and the transitions that need it fall
    /// back to the CPU as well; nothing is drawn with a program the driver
    /// rejected.
    fn install_effect_pipelines(&mut self) {
        if !supports_effect_pipelines(&self.device.pipeline) {
            log::info!(
                "engine.scene: this backend cannot run effect pipelines; the display bitmap is \
                 blended on the CPU"
            );
            return;
        }
        for program in [Program::CrossFade, Program::Scroll] {
            let shader = match self.shader.load(program.desc()) {
                Ok(shader) => shader,
                Err(error) => {
                    log::error!(
                        "engine.scene: the {:?} program was refused: {error}",
                        program.name()
                    );
                    return;
                }
            };
            let pipeline = match self.shader.pipeline(
                &self.device.pipeline,
                &mut self.device.state,
                shader,
                BlendMode::SourceOver,
            ) {
                Ok(pipeline) => pipeline,
                Err(error) => {
                    log::error!(
                        "engine.scene: the {:?} program has no pipeline: {error}",
                        program.name()
                    );
                    return;
                }
            };
            let handle = self.shader.pipeline_handle(pipeline).cloned();
            let Some(handle) = handle else {
                log::error!(
                    "engine.scene: the {:?} program has no pipeline handle",
                    program.name()
                );
                return;
            };
            self.scene.effect_pipelines.insert(program, handle);
        }
    }

    /// Installs the image decoder and starts the decode pool.
    ///
    /// The decoder is the bottom codec the pool drives; the composition root
    /// supplies it, because the concrete implementation lives in the bottom
    /// media layer. Installing a second decoder replaces the first pool, and
    /// the jobs the old pool had not finished are cancelled.
    ///
    /// # Errors
    ///
    /// Returns [`JobPoolError`] when the pool cannot be started.
    pub fn install_decoder(
        &mut self,
        decoder: Arc<dyn ImageDecoder>,
        config: JobPoolConfig,
    ) -> Result<(), JobPoolError> {
        self.decode_pool = Some(DecodePool::new(decoder, config)?);
        Ok(())
    }

    /// The image decode pool, or `None` when no decoder is installed.
    pub fn decode_pool(&self) -> Option<&DecodePool> {
        self.decode_pool.as_ref()
    }

    /// The image decode pool, mutably, or `None` when no decoder is
    /// installed.
    ///
    /// The pool's submit and cancel actions need the mutable borrow; the
    /// compatibility layer reaches the pool through the same attachment the
    /// scene and the timer use.
    pub fn decode_pool_mut(&mut self) -> Option<&mut DecodePool> {
        self.decode_pool.as_mut()
    }

    /// The job accounting of the decode pool, or `None` when no decoder is
    /// installed.
    ///
    /// The counters are the number of jobs the pool ran and the time they
    /// took. A worker updates them under the pool's own lock, so the totals
    /// are read here on the thread that owns the engine.
    pub fn job_stats(&self) -> Option<JobStats> {
        self.decode_pool.as_ref().map(DecodePool::stats)
    }

    /// Installs the storage reader and starts the storage read pool.
    ///
    /// The reader is the bottom backend the pool drives; the composition root
    /// supplies it. The pool is handed to the storage the script reads
    /// through, so the engine and the storage share it: the engine drains it
    /// every frame and the storage schedules the reads on it.
    ///
    /// # Errors
    ///
    /// Returns [`StoragePoolError`] when the pool cannot be started.
    pub fn install_storage_reader(
        &mut self,
        reader: Arc<dyn StorageReader>,
        config: StoragePoolConfig,
    ) -> Result<Arc<StorageReadPool>, StoragePoolError> {
        let pool = Arc::new(StorageReadPool::new(reader, config)?);
        self.storage_pool = Some(Arc::clone(&pool));
        Ok(pool)
    }

    /// The storage read pool, or `None` when no reader is installed.
    pub fn storage_pool(&self) -> Option<&Arc<StorageReadPool>> {
        self.storage_pool.as_ref()
    }

    /// Registers a shader program and returns its identity.
    ///
    /// Registration validates the descriptor and copies the stage code, so
    /// the program outlives the buffers it was loaded from. It does not touch
    /// the backend: the pipeline comes from [`Engine::effect_pipeline`].
    ///
    /// # Errors
    ///
    /// Returns [`ShaderError`] when the descriptor is not usable.
    pub fn load_shader(&mut self, desc: ProgramDesc<'_>) -> Result<ShaderId, ShaderError> {
        self.shader.load(desc)
    }

    /// The pipeline for `shader` built with `blend`, created on first use.
    ///
    /// The blend is part of the backend's pipeline object, so the same program
    /// drawn with two blends needs two pipelines.
    ///
    /// # Errors
    ///
    /// Returns [`ShaderError`] when the program is unknown, when the backend
    /// cannot build effect pipelines at all, or when the driver refuses the
    /// program.
    pub fn effect_pipeline(
        &mut self,
        shader: ShaderId,
        blend: BlendMode,
    ) -> Result<PipelineId, ShaderError> {
        self.shader
            .pipeline(&self.device.pipeline, &mut self.device.state, shader, blend)
    }

    /// Advances the frame clock by the elapsed time `clock` reports and
    /// draws one frame.
    ///
    /// This is the frame's loop-level entry point: the caller chooses where
    /// the elapsed time comes from — the windowed loop injects the real time
    /// the platform measured, the headless runner injects the time it measured
    /// between its frames — and every timeline the frame derives comes from
    /// that one value.
    ///
    /// A negative elapsed time counts as zero. The delta is accumulated with
    /// its fractional part, so sixty 16.6 ms frames advance the clock by
    /// 996 ms, not 960. The `now_ms` handed to the timer, hook and overlay is
    /// the accumulated clock's whole-millisecond part.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the frame hook, the overlay, or a render
    /// action fails.
    pub fn run(
        &mut self,
        clock: &dyn FrameTimeSource,
        hooks: &mut dyn GameFrame,
    ) -> Result<FrameReport, EngineError> {
        let dt_ms = clock.elapsed_ms().max(0.0);
        // The clock keeps the sub-millisecond remainder, so a 60 fps frame
        // does not lose most of a millisecond on every frame.
        let now_ms = self.clock.advance(dt_ms);
        self.run_frame(now_ms, dt_ms, hooks)
    }

    /// Advances the frame clock by `dt_ms` and draws one frame.
    ///
    /// A negative delta counts as zero. The delta is accumulated with its
    /// fractional part, so sixty `step(16.6)` calls advance the clock by
    /// 996 ms, not 960. The `now_ms` handed to the timer, hook and overlay is
    /// the accumulated clock's whole-millisecond part.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the frame hook, the overlay, or a render
    /// action fails.
    pub fn step(
        &mut self,
        dt_ms: f32,
        hooks: &mut dyn GameFrame,
    ) -> Result<FrameReport, EngineError> {
        let clock = InjectedFrameTime::new(f64::from(dt_ms.max(0.0)));
        self.run(&clock, hooks)
    }

    /// Draws one frame at the injected clock value `now_ms`.
    ///
    /// The clock is set to `now_ms` rather than accumulated, so a test can
    /// jump between frames deterministically and any fractional remainder left
    /// by [`Engine::step`] is discarded. The delta handed to the hook is the
    /// distance from the previous frame's clock, and zero on the first frame.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the frame hook, the overlay, or a render
    /// action fails.
    pub fn frame(
        &mut self,
        now_ms: u64,
        hooks: &mut dyn GameFrame,
    ) -> Result<FrameReport, EngineError> {
        let dt_ms = now_ms.saturating_sub(self.last_now_ms.unwrap_or(now_ms)) as f64;
        self.clock.set(now_ms);
        self.run_frame(now_ms, dt_ms, hooks)
    }

    /// Runs one frame at `now_ms`, with `dt_ms` as the hook's delta.
    ///
    /// The clock is already set when this runs: `step` accumulated the delta
    /// into it and `frame` put it at the injected value.
    ///
    /// The order is the frame contract: fit the presentation, fire due
    /// timers, run the upward hook, fit the presentation again (the hook may
    /// have resized a window's primary layer), then refresh the overlay,
    /// upload, project, apply the presentation root transform and draw. A size
    /// pair that cannot be fitted skips everything that draws and reports no
    /// frame statistics.
    fn run_frame(
        &mut self,
        now_ms: u64,
        dt_ms: f64,
        hooks: &mut dyn GameFrame,
    ) -> Result<FrameReport, EngineError> {
        self.last_now_ms = Some(now_ms);
        self.sync_presentation();
        deliver_due(&mut self.timer, now_ms);
        // The storage barrier: the reads the previous frame scheduled are
        // published before the hook runs, so a read the script makes now
        // finds its result ready.
        if let Some(pool) = &self.storage_pool {
            pool.drain();
        }
        // The upward hook is where the compatibility layer asks for the
        // device work it needs: the device composite of a layer's subtree.
        // The engine owns the device and runs the requests, so the scene
        // never holds it.
        hooks
            .before_render(now_ms, dt_ms)
            .map_err(EngineError::Frame)?;
        // The image barrier: every image the hook queued is decoded before
        // the upload and the projection, so the frame draws decoded pixels.
        // The decodes run concurrently, so the wait is the time the last one
        // needs rather than the sum of them.
        self.scene.resolve_pending_images();
        self.sync_presentation();

        if self.translation.is_none() {
            self.frame_index = self.frame_index.saturating_add(1);
            Self::note_presentation_stall(&mut self.stall, StallCause::NoMapping, now_ms);
            return Ok(FrameReport {
                frame_index: self.frame_index,
                elapsed_ms: self.clock.now_f64(),
                presented: false,
                stats: None,
                sync_ms: 0.0,
                upload_ms: 0.0,
                prune_ms: 0.0,
                project_ms: 0.0,
                render_ms: 0.0,
                upload_timings: None,
            });
        }

        self.refresh_overlay(now_ms)?;

        // The layers' draw lists are drawn on the backend: the pose the
        // compatibility layer left is composited there, and the layer's image
        // carries the result into the upload below.
        self.draw_layer_draw_lists();

        let sync_started = Instant::now();
        let upload_started = Instant::now();
        let mut upload_timings = self.sync_dirty_bitmaps();
        let mut upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;
        let prune_started = Instant::now();
        self.prune_stale_textures();
        let mut prune_ms = prune_started.elapsed().as_secs_f64() * 1000.0;
        let mut sync_ms = sync_started.elapsed().as_secs_f64() * 1000.0;

        // The device work the compatibility layer asked for: the composite of
        // a layer's subtree on the device. It runs after the upload, so the
        // sources it samples are the pixels the frame draws, and before the
        // projection, so the picture it produces is in this frame.
        self.service_device_requests();

        let project_started = Instant::now();
        self.reproject_render_scene();
        let mut project_ms = project_started.elapsed().as_secs_f64() * 1000.0;

        // The upward hook sees the projection's actual draw report. A hook
        // that changes the scene (a self-updating transition step) is
        // followed by a second upload and projection so this same frame draws
        // the new state, not the previous one.
        hooks
            .after_project(&self.drawn_layers)
            .map_err(EngineError::Frame)?;
        self.sync_presentation();

        // The hook may have asked for more device work: a self-updating
        // transition steps here and asks for its endpoints' composite. The
        // requests run before the upload below, so the composite reaches the
        // frame this hook draws.
        self.service_device_requests();

        // The hook may have rewritten a transition frame bitmap without
        // changing the scene graph; upload those dirty bitmaps either way.
        let sync2_started = Instant::now();
        let upload2_started = Instant::now();
        upload_timings = add_upload_timings(upload_timings, self.sync_dirty_bitmaps());
        upload_ms += upload2_started.elapsed().as_secs_f64() * 1000.0;
        let prune2_started = Instant::now();
        self.prune_stale_textures();
        prune_ms += prune2_started.elapsed().as_secs_f64() * 1000.0;
        sync_ms += sync2_started.elapsed().as_secs_f64() * 1000.0;
        if self.translation.is_some()
            && self.render_scene_revision != Some(self.scene.structure_revision())
        {
            let project2_started = Instant::now();
            self.reproject_render_scene();
            project_ms += project2_started.elapsed().as_secs_f64() * 1000.0;
        }

        let render_started = Instant::now();
        // The frame draws the whole scene when anything changed. A frame with
        // nothing to show does not draw at all: the window keeps the content
        // of the last presented frame, and the change waits for a frame that
        // has something to show.
        let mut presented = false;
        if self.scene.dirty() {
            // The frame is drawn into the target the host gave the engine: a
            // surface to present to, or an offscreen target to draw into.
            match &mut self.target {
                FrameTarget::Surface(surface) => {
                    let outcome = match render(
                        &self.device.pipeline,
                        surface,
                        &mut self.device.state,
                        &self.render_scene,
                    ) {
                        Ok(presentation) => present_outcome(presentation),
                        // A swapchain that could not be rebuilt is reported here and
                        // retried by the next frame's render call; the backend logged the
                        // precise cause when it happened.
                        Err(RenderError::SurfaceOutOfDate) => present_outcome(Presentation {
                            presented: false,
                            surface_event: Some(SurfaceEvent::OutOfDate),
                            skip: None,
                        }),
                        Err(error) => return Err(error.into()),
                    };
                    // A lost surface is recoverable: rebuild the surface and swapchain in
                    // the bottom layer and keep the scene, textures and VM in place. The
                    // recovered frame is skipped and the next frame presents normally.
                    if outcome.surface_event == Some(SurfaceEvent::Lost) {
                        recover_surface(&self.device.pipeline, surface, &mut self.device.state)?;
                        log::info!(
                            "engine.frame: presentation surface recreated after it was lost"
                        );
                        // The recreated surface holds nothing, so the whole frame is
                        // drawn again.
                        self.scene.mark_dirty();
                    }
                    if outcome.presented {
                        Self::finish_presentation_stall(&mut self.stall, now_ms);
                        // The presented frame carries the change, so the next frame
                        // starts clean. A frame the display did not accept keeps it:
                        // the previous frame's content is still on screen.
                        self.scene.clear_dirty();
                        presented = true;
                    } else {
                        // `present_outcome` names a fault for every non-presented frame; a
                        // frame without one is the backend's silence, reported as such.
                        let fault = match outcome.fault {
                            Some(fault) => fault,
                            None => PresentFault::Unreported,
                        };
                        Self::note_presentation_stall(
                            &mut self.stall,
                            StallCause::Backend(fault),
                            now_ms,
                        );
                    }
                }
                FrameTarget::Offscreen(target) => {
                    // The target holds the frame: there is no surface to
                    // present to, so the frame is complete when the draw
                    // returns, and the target keeps it until the next frame
                    // changes it.
                    render_to_target(
                        &self.device.pipeline,
                        &mut self.device.state,
                        target,
                        &self.render_scene,
                    )?;
                    self.scene.clear_dirty();
                    presented = true;
                }
            }
        }
        let render_ms = render_started.elapsed().as_secs_f64() * 1000.0;

        self.frame_index = self.frame_index.saturating_add(1);

        Ok(FrameReport {
            frame_index: self.frame_index,
            elapsed_ms: self.clock.now_f64(),
            presented,
            stats: self.last_frame(),
            sync_ms,
            upload_ms,
            prune_ms,
            project_ms,
            render_ms,
            upload_timings,
        })
    }

    /// Records a frame the display did not accept.
    ///
    /// A cause is reported the first time it joins the stall, and the stall's
    /// age is reported once it lasts long enough that a game frozen on screen
    /// would otherwise look like a hung process.
    fn note_presentation_stall(stall: &mut PresentationStall, cause: StallCause, now_ms: u64) {
        if let Some(cause) = stall.record(cause, now_ms) {
            cause.warn();
        }
        if let Some(age) = stall.due_report(now_ms) {
            log::warn!(
                "engine.frame: the display has not accepted a frame elapsed_s={:.1} \
                 frames_skipped={} causes={}",
                age.elapsed_ms as f64 / 1000.0,
                age.frames,
                describe_stall_causes(&stall.causes),
            );
        }
    }

    /// Ends a presentation stall and reports how long it lasted and why.
    fn finish_presentation_stall(stall: &mut PresentationStall, now_ms: u64) {
        if let Some(summary) = stall.resumed(now_ms) {
            log::info!(
                "engine.frame: presentation resumed frames_skipped={} elapsed_s={:.1} causes={}",
                summary.frames,
                summary.elapsed_ms as f64 / 1000.0,
                describe_stall_causes(&summary.causes),
            );
        }
    }

    /// Rebinds the surface to a new size in physical pixels.
    ///
    /// The size is clamped to at least `1x1`, and an unchanged size does not
    /// touch the backend. The framebuffer is not the logical screen: the scene
    /// keeps its window client sizes and primary-layer rects, and the
    /// presentation refits the screen into the new framebuffer, so an OS resize
    /// moves no value a script observes.
    ///
    /// A host with no window has no surface: its offscreen target is the
    /// frame's destination, so a new size replaces it with a target of that
    /// size, and the frame is drawn again into the empty target.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Render`] when the backend rejects the resize or
    /// the replacement target cannot be created.
    pub fn resize(&mut self, size: (u32, u32)) -> Result<SurfaceEvent, EngineError> {
        let size = (size.0.max(1), size.1.max(1));
        let event = match &mut self.target {
            FrameTarget::Surface(surface) => {
                if size == self.surface_size {
                    SurfaceEvent::Resized(size.0, size.1)
                } else {
                    resize_render(&self.device.pipeline, surface, &mut self.device.state, size)?
                }
            }
            FrameTarget::Offscreen(target) => {
                if size != self.surface_size {
                    // The replacement is created before the old target is
                    // released, so the frame's destination is never missing.
                    let replacement = create_render_target(
                        &self.device.pipeline,
                        RenderTargetDesc::new(size.0, size.1),
                    )?;
                    let previous = std::mem::replace(target, replacement);
                    destroy_render_target(&self.device.pipeline, &mut self.device.state, previous);
                    // The new target holds nothing, so the frame is drawn
                    // again.
                    self.scene.mark_dirty();
                }
                SurfaceEvent::Resized(size.0, size.1)
            }
        };
        self.surface_size = reported_surface_size(event, size);
        self.sync_presentation();
        Ok(event)
    }

    /// Rebuilds [`Engine::render_scene`] when the layer tree's structure or
    /// the presentation mapping is stale.
    ///
    /// The projection is skipped while [`Scene::structure_revision`] is
    /// unchanged, so a static tree does not reallocate the render tree every
    /// frame and a bitmap content write — which moves no node — does not
    /// re-project at all. A rebuild reuses the existing arena: the node list
    /// is cleared (retaining its capacity) and refilled, so only the
    /// projected nodes' own allocations are touched. The revision is `None`
    /// until the first projection, so the first frame always projects.
    ///
    /// The tree is projected in VM pixels and the render scene root carries the
    /// presentation transform, which is why a changed presentation also
    /// invalidates it (via `render_scene_revision = None`). The overlay is
    /// authored in device coordinates, so it is attached with the inverse root
    /// transform. Nothing is rebuilt while the two sizes cannot be fitted; the
    /// frame path draws nothing in that case.
    fn reproject_render_scene(&mut self) {
        let revision = self.scene.structure_revision();
        if self.render_scene_revision == Some(revision) {
            return;
        }
        let Some(translation) = self.translation else {
            return;
        };

        let textures = &self.device.textures;
        let ProjectionReport {
            scene: projected,
            drawn_layers,
        } = project_scene_report(&self.scene, |bitmap| textures.get(&bitmap).cloned());
        self.drawn_layers = drawn_layers;

        self.render_scene.nodes.clear();
        self.render_scene.root = projected.root;
        self.render_scene.background = projected.background;
        self.render_scene.coverage = projected.coverage;
        self.render_scene.nodes.extend(projected.nodes);

        let root = self.render_scene.root;
        if let Some(root_node) = self.render_scene.node_mut(root) {
            root_node.transform = translation.root_transform();
        }
        self.attach_overlay(&translation);
        self.render_scene_revision = Some(revision);
    }

    /// Refits the VM→device presentation for the current sizes and reports a
    /// script-side window resize to the composition root.
    ///
    /// The VM size is the first window's primary-layer rect: the layer whose
    /// MainImage is the window's screen buffer. Before the window has a primary
    /// layer the window's client size stands in, and with no window the
    /// framebuffer size does. An OS resize moves only the framebuffer and a
    /// script window-client write moves only the window size; only the latter
    /// is surfaced to the composition root. A changed mapping invalidates the
    /// projected tree, which is built in VM pixels. A size pair that cannot be
    /// fitted leaves the frame without a mapping and warns once rather than
    /// substituting a scale.
    fn sync_presentation(&mut self) {
        let vm_size = self.vm_size();
        let translation = match PixelTranslation::fit(vm_size, self.surface_size) {
            Ok(translation) => {
                self.presentation_skipped_logged = false;
                Some(translation)
            }
            Err(TranslateError::EmptyDimension { vm, device }) => {
                if !self.presentation_skipped_logged {
                    log::warn!(
                        "engine.frame: the game surface cannot be fitted into the framebuffer \
                         vm_size={vm:?} device_size={device:?}"
                    );
                    self.presentation_skipped_logged = true;
                }
                None
            }
        };
        if translation != self.translation {
            self.translation = translation;
            self.render_scene_revision = None;
            // A new mapping rescales the whole surface, so every pixel of the
            // previous frame's content is different.
            self.scene.mark_dirty();
            if let Some(translation) = translation {
                log::info!(
                    "engine.frame: the presentation mapping vm_size={}x{} device_size={}x{} scale={:.3}",
                    translation.vm_size.0,
                    translation.vm_size.1,
                    translation.device_size.0,
                    translation.device_size.1,
                    translation.scale
                );
            }
        }

        let window_client_size = self.scene.windows.first().map(|window| window.inner_size);
        if window_client_size != self.last_window_client_size {
            // The first observation asks for a window resize only when the
            // client size already differs from the framebuffer; every later
            // change is a script resize and is surfaced to the composition
            // root. An OS resize leaves the client size alone, so it never
            // asks.
            if let Some(size) = window_client_size
                && (self.last_window_client_size.is_some() || size != self.surface_size)
            {
                self.pending_window_resize = Some(size);
            }
            self.last_window_client_size = window_client_size;
        }
    }

    /// Appends the overlay as the last child of the render scene root.
    ///
    /// The projection clears and refills the arena, so the overlay node is
    /// re-added after every rebuild and never on a skipped one. The node is
    /// authored in device pixels, so its transform is set to the inverse of the
    /// presentation root transform; its texture is stable across refreshes, so
    /// the node needs no refresh here.
    fn attach_overlay(&mut self, translation: &PixelTranslation) {
        let Some(overlay) = self.overlay.as_ref() else {
            return;
        };
        let root = self.render_scene.root;
        let mut node = overlay.node();
        node.parent = Some(root);
        node.transform = translation.inverse_root_transform();
        let id = self.render_scene.push(node);
        if let Some(root_node) = self.render_scene.node_mut(root) {
            root_node.children.push(id);
        }
    }

    /// Advances the overlay by one frame, letting it refresh its own texture.
    fn refresh_overlay(&mut self, now_ms: u64) -> Result<(), EngineError> {
        let objects = self.object_counts();
        let Some(overlay) = self.overlay.as_mut() else {
            return Ok(());
        };
        // The overlay is drawn in device coordinates, outside the screen the
        // scene describes, so the area it covers cannot be named. A frame with
        // an overlay draws the whole surface.
        self.scene.mark_dirty();
        overlay
            .refresh(
                &self.device.pipeline,
                &mut self.device.state,
                now_ms,
                objects,
            )
            .map_err(EngineError::Overlay)
    }

    /// Returns the stateless render pipeline.
    pub fn pipeline(&self) -> &RenderPipeline {
        &self.device.pipeline
    }

    /// Returns the backend the pipeline was created with.
    pub fn backend(&self) -> BackendKind {
        self.device.pipeline.backend()
    }

    /// Returns the layer tree.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Returns the layer tree for mutation.
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// Returns the frame clock.
    ///
    /// The clock is the engine's time base: the frame advances it, the timer
    /// registry and the upward hook read it, and the script reads it through
    /// `System.getTickCount`.
    pub fn clock(&self) -> &FrameClock {
        &self.clock
    }

    /// Returns the frame clock for mutation.
    pub fn clock_mut(&mut self) -> &mut FrameClock {
        &mut self.clock
    }

    /// Returns the most recently projected render tree.
    ///
    /// The nodes keep VM-space coordinates; the root carries the presentation
    /// transform that maps them into device pixels.
    pub fn render_scene(&self) -> &RenderScene {
        &self.render_scene
    }

    /// Returns the layer ids the latest projection actually emitted, including
    /// visible descendants covered by a cached ancestor.
    pub fn drawn_layers(&self) -> &[u32] {
        &self.drawn_layers
    }

    /// Returns the bitmap-id → texture table.
    pub fn textures(&self) -> &Map<u32, Texture> {
        &self.device.textures
    }

    /// Returns the script-visible input state.
    pub fn input(&self) -> &crate::input::InputState {
        &self.input
    }

    /// Returns the script-visible input state for mutation.
    pub fn input_mut(&mut self) -> &mut crate::input::InputState {
        &mut self.input
    }

    /// Returns the timer registry for setup and inspection.
    pub fn timer(&self) -> &TimerState {
        &self.timer
    }

    /// Returns the timer registry for arming, rescheduling and removal.
    pub fn timer_mut(&mut self) -> &mut TimerState {
        &mut self.timer
    }

    /// Returns the text service: the registered faces and the shaping state.
    pub fn text(&self) -> &crate::text::TextEngine {
        &self.text
    }

    /// Returns the text service for registration and shaping.
    pub fn text_mut(&mut self) -> &mut crate::text::TextEngine {
        &mut self.text
    }

    /// Whether the engine runs offscreen with no window.
    pub fn is_headless(&self) -> bool {
        self.headless
    }

    /// Returns the current surface size in pixels.
    pub fn surface_size(&self) -> (u32, u32) {
        self.surface_size
    }

    /// Returns the script-visible screen size: the first window's primary-layer
    /// rect, the window's client size while it has no primary layer, or the
    /// framebuffer size when the scene has no window.
    pub fn vm_size(&self) -> (u32, u32) {
        let Some(window) = self.scene.windows.first() else {
            return self.surface_size;
        };
        match window
            .primary_layer
            .and_then(|primary| self.scene.layer(primary))
        {
            Some(layer) => (layer.rect.w, layer.rect.h),
            None => window.inner_size,
        }
    }

    /// Returns the live scene and texture object counts.
    pub fn object_counts(&self) -> ObjectCounts {
        ObjectCounts {
            windows: self.scene.windows.len(),
            layers: self.scene.layers.len(),
            bitmaps: self.scene.bitmaps.len(),
            fonts: self.scene.fonts.len(),
            textures: self.device.textures.len(),
        }
    }

    /// Returns the current VM-pixel → device-pixel mapping, or `None` when the
    /// VM and framebuffer sizes cannot be fitted.
    pub fn translation(&self) -> Option<PixelTranslation> {
        self.translation
    }

    /// Takes the script-side window size the composition root still has to
    /// apply to the OS window, if any.
    ///
    /// A request is produced when the script creates a window whose client
    /// size differs from the framebuffer and on every later change of that
    /// size. Taking it clears it, so a size that does not change again is not
    /// requested twice.
    pub fn take_window_resize(&mut self) -> Option<(u32, u32)> {
        self.pending_window_resize.take()
    }

    /// Returns the number of frames drawn so far.
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Returns the frame clock of the most recent frame, in milliseconds.
    ///
    /// A clock advanced with [`Engine::step`] carries the accumulated
    /// fractional part; [`Engine::frame`] sets it to the injected whole value.
    pub fn elapsed_ms(&self) -> f64 {
        self.clock.now_f64()
    }

    /// Returns the null backend's counters for the last frame, if available.
    pub fn last_frame(&self) -> Option<FrameStats> {
        render::NullRenderPipeline::last_frame(&self.device.state)
    }

    /// Returns the presentation calls' costs for the last presented frame.
    ///
    /// The Vulkan backend reports the acquire/present/fence/recreate costs it
    /// measured; the null backend reports zeros. `None` means no frame has
    /// been presented through this state yet.
    pub fn last_frame_timings(&self) -> Option<FrameTimings> {
        last_frame_timings(&self.device.state)
    }

    /// Reads the frame the last frame drew into the offscreen target.
    ///
    /// The pixels are RGBA8 in device pixels: the target holds the picture
    /// the projected tree was drawn into, at the engine's surface size. A
    /// host that presents to a surface has no target, so it reads nothing.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Render`] when the read-back fails, which is the
    /// case when no frame has been drawn into the target yet.
    pub fn read_offscreen_frame(&self) -> Result<Option<Vec<u8>>, EngineError> {
        let FrameTarget::Offscreen(target) = &self.target else {
            return Ok(None);
        };
        let width = target.width();
        let height = target.height();
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        read_render_target(
            &self.device.pipeline,
            &self.device.state,
            target,
            PixelRegion::new(0, 0, width, height),
            &mut pixels,
        )?;
        Ok(Some(pixels))
    }

    /// Installs the backend that reads a layer's pose surface back.
    ///
    /// The pose the motion player draws lives on the backend, so a script
    /// that reads the layer's pixels asks the scene for them, and the scene
    /// calls this. The read-back is a backend operation, and the engine
    /// owns the pipeline and the render state, so the composition root
    /// builds the seam.
    pub fn install_pose_sync(&mut self, sync: crate::scene::PoseSync) {
        self.scene.set_pose_sync(sync);
    }

    /// Reads the pose of a layer back into the pixels a script reads.
    ///
    /// The pixels are the layer's image with the pose composited over them.
    /// A layer that shows no pose has nothing to add, and the image's own
    /// pixels are the answer.
    ///
    /// The scene caches the pixels with the pose's revision, so a read that
    /// finds the cache current does not reach this at all: the seam is
    /// called only when the pose was redrawn.
    pub fn sync_pose_pixels(&mut self, bitmap: u32) -> Result<Option<Vec<u8>>, RenderError> {
        // A layer that shows no pose reads nothing.
        let Some(pose) =
            self.scene
                .read_pose_pixels(&self.device.pipeline, &mut self.device.state, bitmap)?
        else {
            return Ok(None);
        };
        self.scene.compose_pose_pixels(bitmap, &pose)
    }

    /// Whether the engine draws a render-only overlay.
    pub fn debug_overlay_enabled(&self) -> bool {
        self.overlay.is_some()
    }

    /// Uploads every bitmap whose dirty flag is set and clears the flag.
    ///
    /// The scene keeps the bitmaps whose pixels may have changed, so the pass
    /// walks that set instead of the bitmap table; a frame that changed
    /// nothing uploads nothing.
    ///
    /// A bitmap whose dimensions changed recreates its texture; one with an
    /// inconsistent pixel length is skipped with a warning so one bad bitmap
    /// cannot abort the frame. All dirty bitmaps are uploaded in one batch, so
    /// the frame submits them once and the present path waits for that batch
    /// instead of waiting once per bitmap.
    /// Runs the device work the compatibility layer asked for.
    ///
    /// The requests are the compatibility layer's device work: the composite
    /// of a layer's subtree and the release of the surface a composite was
    /// drawn into. The engine owns the device, so the work runs here, on the
    /// frame thread, and the picture it produces is in the frame the engine
    /// draws.
    fn service_device_requests(&mut self) {
        for request in self.scene.take_device_requests() {
            match request {
                crate::scene::DeviceRequest::Composite(root) => {
                    self.composite_subtree_on_device(root);
                }
                crate::scene::DeviceRequest::ReleaseSurface(layer) => {
                    release_device_surface(&mut self.device, layer);
                }
            }
        }
    }

    /// The device composite of a layer's subtree: the subtree's picture drawn
    /// on the device.
    ///
    /// A tree the device cannot draw is composited on the CPU, which is the
    /// exact path: the refusal is a typed error, and the tree is drawn by the
    /// path that always draws it.
    fn composite_subtree_on_device(&mut self, root: u32) {
        let result = composite_subtree_on_device(&mut self.device, &mut self.scene, root);
        match result {
            Ok(_) => {}
            Err(error @ RenderError::Unsupported { .. }) => {
                log::debug!(
                    "engine.scene: the device cannot draw the subtree of layer {root}: {error}; \
                     the CPU composite draws it"
                );
                self.composite_subtree_on_cpu(root);
            }
            Err(error) => {
                log::warn!(
                    "engine.scene: the device composite of layer {root} failed: {error}; the CPU \
                     composite draws it"
                );
                self.composite_subtree_on_cpu(root);
            }
        }
    }

    /// The CPU composite of a layer's subtree: the exact path.
    ///
    /// The CPU composite is the fallback for every tree the device refuses to
    /// draw, so the tree is drawn by the path that always draws it.
    fn composite_subtree_on_cpu(&mut self, root: u32) {
        if crate::layer_ops::render_layer_subtree(&mut self.scene, root).is_none() {
            log::warn!("engine.scene: the CPU composite of layer {root} failed");
        }
    }

    /// Draws the layers' draw lists on the backend.
    ///
    /// A list the backend cannot draw - it has no surface, or the draw
    /// failed - falls back to the CPU pass, so the pose is never lost.
    fn draw_layer_draw_lists(&mut self) {
        for layer_id in self.scene.layers_with_draws() {
            // A layer whose pose the backend could not draw keeps drawing it
            // on the CPU: its image is then the whole picture, and a pose
            // surface over it would draw the pose twice.
            if self.cpu_pose_layers.contains(&layer_id) {
                self.scene.draw_layer_draws(layer_id);
                continue;
            }
            let drawn = self.scene.draw_layer_draws_gpu(
                &self.device.pipeline,
                &mut self.device.state,
                layer_id,
            );
            match drawn {
                Ok(true) => {}
                Ok(false) => {
                    self.draw_pose_on_cpu(layer_id, None);
                }
                Err(error) => {
                    self.draw_pose_on_cpu(layer_id, Some(&error.to_string()));
                }
            }
        }
    }

    /// Draws the layer's pose on the CPU and records that it does.
    ///
    /// The pose surface is dropped, so the layer's image carries the pose
    /// alone. The layer keeps drawing the pose on the CPU: the image holds
    /// the pose the CPU pass drew, and a pose surface over it would draw
    /// the pose twice.
    fn draw_pose_on_cpu(&mut self, layer_id: u32, error: Option<&str>) {
        match error {
            Some(error) => log::warn!(
                "engine.frame: the pose of layer {layer_id} was not drawn on the backend: \
                 {error}; the CPU pass draws it"
            ),
            None => log::warn!(
                "engine.frame: the pose of layer {layer_id} was not drawn on the backend; the \
                 CPU pass draws it"
            ),
        }
        if let Some(bitmap_id) = self.scene.layer(layer_id).and_then(|layer| layer.bitmap) {
            self.scene
                .drop_pose_surface(&self.device.pipeline, &mut self.device.state, bitmap_id);
        }
        self.cpu_pose_layers.insert(layer_id);
        self.scene.draw_layer_draws(layer_id);
    }

    ///
    /// A bitmap that changed in a rectangle uploads that rectangle alone;
    /// one that changed wholly uploads every pixel.
    ///
    /// The copies the device holds follow the layers: a bitmap no layer
    /// holds has no copy on the device, so its pixels stay on the host and
    /// the copy is made when a layer shows it. The copies the device holds
    /// are the ones the picture needs, and the descriptor set each of them
    /// takes is the pool's scarcest resource.
    ///
    /// The costs of the upload are returned when the backend measured them,
    /// so the frame's report can name where the upload's time went.
    fn sync_dirty_bitmaps(&mut self) -> Option<UploadTimings> {
        let dirty: Vec<u32> = self.scene.dirty_bitmaps_for_upload();
        // The bitmaps the layers hold: the copies the picture needs.
        let held = self.scene.held_bitmaps();
        // The bitmaps a layer holds whose copy is gone: the copy is made
        // again, and the whole bitmap is uploaded into it.
        let missing: Vec<u32> = held
            .iter()
            .copied()
            .filter(|id| !self.device.textures.contains_key(id))
            .collect();

        let mut candidates: Vec<u32> = Vec::with_capacity(dirty.len() + missing.len());
        let mut seen: Set<u32> = set();
        for id in dirty {
            if !held.contains(&id) {
                // The bitmap is not shown: its pixels stay on the host, and
                // the copy is made when a layer shows the bitmap.
                self.scene.set_bitmap_clean(id);
                continue;
            }
            if seen.insert(id) {
                candidates.push(id);
            }
        }
        for id in missing {
            // A bitmap that is still decoding has no pixels to upload: the
            // decode lands them and the upload sends them then.
            if self
                .scene
                .bitmap(id)
                .is_some_and(|bitmap| bitmap.pending.is_some())
            {
                continue;
            }
            if seen.insert(id) {
                candidates.push(id);
            }
        }

        let mut ready: Vec<u32> = Vec::with_capacity(candidates.len());
        for id in candidates {
            let Some((width, height, len)) = self
                .scene
                .bitmap(id)
                .map(|bitmap| (bitmap.width, bitmap.height, bitmap.rgba.len()))
            else {
                continue;
            };
            if width == 0 || height == 0 {
                log::warn!("engine.scene: bitmap id={id} has a zero dimension; skipping upload");
                self.scene.set_bitmap_clean(id);
                continue;
            }
            let expected = width as usize * height as usize * 4;
            if len != expected {
                log::warn!(
                    "engine.scene: bitmap id={id} has {len} pixel bytes, expected {expected}; \
                     skipping upload"
                );
                self.scene.set_bitmap_clean(id);
                continue;
            }

            let recreate = self
                .device
                .textures
                .get(&id)
                .is_none_or(|texture| texture.width() != width || texture.height() != height);
            if recreate {
                let old = self.device.textures.remove(&id);
                if let Some(old) = old {
                    drop_texture(&self.device.pipeline, &mut self.device.state, old);
                }
                match create_texture(&self.device.pipeline, TextureDesc::new(width, height)) {
                    Ok(texture) => {
                        self.device.textures.insert(id, texture);
                        // A new texture holds no pixels: the copy has to
                        // cover the whole bitmap, or the rest of the texture
                        // stays undefined. Marking the bitmap wholly keeps a
                        // retry of that upload whole as well.
                        self.scene.mark_bitmap_whole(id);
                        // The projection resolves its textures through this
                        // table: a bitmap that had no texture when it was last
                        // projected now has one, so the node it was skipped
                        // for must be projected again even when the tree's
                        // structure is unchanged.
                        self.render_scene_revision = None;
                    }
                    Err(error) => {
                        log::warn!(
                            "engine.scene: failed to create a texture for bitmap id={id}: \
                             {error}; the device holds {} copies",
                            self.device.textures.len()
                        );
                        continue;
                    }
                }
            }
            ready.push(id);
        }

        if ready.is_empty() {
            return None;
        }
        // The region every dirty bitmap uploads, with the packed pixels
        // of the partial ones.
        let mut planned: Vec<(u32, PixelRegion, Vec<u8>)> = Vec::with_capacity(ready.len());
        for id in &ready {
            let Some(bitmap) = self.scene.bitmap(*id) else {
                continue;
            };
            let (region, pixels) = dirty_upload(bitmap);
            planned.push((*id, region, pixels));
        }
        let uploads: Vec<TextureUpload<'_>> = planned
            .iter()
            .filter_map(|(id, region, pixels)| {
                let bitmap = self.scene.bitmap(*id)?;
                let texture = self.device.textures.get(id)?;
                let whole = PixelRegion::new(0, 0, bitmap.width, bitmap.height);
                // The whole bitmap uploads its own buffer; a region
                // uploads the packed copy of its rows.
                let source = if *region == whole {
                    &bitmap.rgba[..]
                } else {
                    pixels.as_slice()
                };
                Some(TextureUpload {
                    texture,
                    region: *region,
                    pixels: source,
                })
            })
            .collect();
        let uploaded = match upload_textures(
            &self.device.pipeline,
            &mut self.device.state,
            &uploads,
        ) {
            Ok(()) => true,
            Err(error) => {
                log::warn!(
                    "engine.scene: failed to upload bitmaps count={}: {error}; the bitmaps stay \
                     dirty and the next frame retries them",
                    uploads.len()
                );
                false
            }
        };
        // The costs of the batch that was just submitted, read right after
        // the call: the value describes this batch and not an earlier one.
        // A batch with no uploads submits nothing, so there is nothing to
        // measure.
        let timings = if uploads.is_empty() {
            None
        } else {
            last_upload_timings(&self.device.state)
        };
        drop(uploads);
        // The uploaded pixels are the ones the layers drawing the bitmap
        // show, so the picture changed.
        self.scene.mark_dirty();
        clear_uploaded_dirty(&mut self.scene, &ready, uploaded);
        timings
    }

    /// Drops textures whose bitmap no longer exists in the scene.
    ///
    /// The scene records the bitmaps it removed, so the prune drops the
    /// textures of those bitmaps instead of walking the texture table.
    ///
    /// The device surfaces go with the layers they belong to: a layer that
    /// no longer exists has no composite to draw, so its surface is released.
    fn prune_stale_textures(&mut self) {
        let removed = self.scene.take_removed_bitmaps();
        for id in removed {
            if let Some(texture) = self.device.textures.remove(&id) {
                drop_texture(&self.device.pipeline, &mut self.device.state, texture);
            }
        }
        // The copies of the bitmaps no layer holds any more: a bitmap nobody
        // shows needs no copy on the device, and the descriptor set each
        // copy holds is the pool's scarcest resource. The copy of a bitmap
        // that is shown again is made again, and the whole bitmap is
        // uploaded into it.
        let held = self.scene.held_bitmaps();
        let stale: Vec<u32> = self
            .device
            .textures
            .keys()
            .copied()
            .filter(|id| !held.contains(id))
            .collect();
        for id in stale {
            if let Some(texture) = self.device.textures.remove(&id) {
                drop_texture(&self.device.pipeline, &mut self.device.state, texture);
            }
        }
        // The copies of the draw sources that were released: a source is
        // per-frame content, and its copy goes with it.
        for texture in self.scene.take_released_draw_textures() {
            drop_texture(&self.device.pipeline, &mut self.device.state, texture);
        }
        // The device surfaces go with the layers they belong to: a layer that
        // no longer exists has no composite to draw, so its surface is
        // released.
        let stale: Vec<u32> = self
            .device
            .surfaces
            .keys()
            .copied()
            .filter(|layer| self.scene.layer(*layer).is_none())
            .collect();
        for layer in stale {
            release_device_surface(&mut self.device, layer);
        }
    }
}

/// Returns the framebuffer size a backend resize event actually took.
///
/// A backend reports the extent the surface accepted; a driver or compositor
/// clamp can make it differ from the requested size, and the presentation must
/// fit the real framebuffer. A non-resize event leaves `requested` in place.
fn reported_surface_size(event: SurfaceEvent, requested: (u32, u32)) -> (u32, u32) {
    match event {
        SurfaceEvent::Resized(width, height) => (width.max(1), height.max(1)),
        SurfaceEvent::Lost | SurfaceEvent::Restored | SurfaceEvent::OutOfDate => requested,
    }
}

/// The region a dirty bitmap uploads, with the packed pixels of a partial
/// region.
///
/// A bitmap that changed wholly uploads every pixel; a bitmap that changed
/// in a rectangle uploads that rectangle alone, with its rows packed
/// together. A region that no longer fits the bitmap (it was resized) or
/// that covers no pixel uploads every pixel.
fn dirty_upload(bitmap: &BitmapState) -> (PixelRegion, Vec<u8>) {
    let whole = PixelRegion::new(0, 0, bitmap.width, bitmap.height);
    let Some(region) = bitmap.dirty_region else {
        return (whole, Vec::new());
    };
    if region.is_empty() || region.right() > bitmap.width || region.bottom() > bitmap.height {
        return (whole, Vec::new());
    }
    if region == whole {
        return (whole, Vec::new());
    }
    match region_pixels(bitmap, region) {
        Some(pixels) => (region, pixels),
        None => (whole, Vec::new()),
    }
}

/// The pixels of `region` inside `bitmap`, packed row after row.
///
/// `None` when the region does not fit the bitmap's buffer.
fn region_pixels(bitmap: &BitmapState, region: PixelRegion) -> Option<Vec<u8>> {
    let width = usize::try_from(bitmap.width).ok()?;
    let row_bytes = usize::try_from(region.width).ok()?.checked_mul(4)?;
    let height = usize::try_from(region.height).ok()?;
    let mut pixels = Vec::with_capacity(row_bytes.checked_mul(height)?);
    for row in 0..height {
        let y = usize::try_from(region.y).ok()?.checked_add(row)?;
        let start = y
            .checked_mul(width)?
            .checked_add(usize::try_from(region.x).ok()?)?;
        let start = start.checked_mul(4)?;
        let end = start.checked_add(row_bytes)?;
        pixels.extend_from_slice(bitmap.rgba.get(start..end)?);
    }
    Some(pixels)
}

/// Adds two upload measurements into one.
///
/// A frame uploads its dirty bitmaps twice: once before the projection and
/// once after the upward hook, so the frame's report holds the sum of the
/// two batches. A backend that measured only one of them contributes that
/// one.
fn add_upload_timings(
    left: Option<UploadTimings>,
    right: Option<UploadTimings>,
) -> Option<UploadTimings> {
    match (left, right) {
        (Some(left), Some(right)) => Some(UploadTimings {
            create_ms: left.create_ms + right.create_ms,
            stage_ms: left.stage_ms + right.stage_ms,
            submit_ms: left.submit_ms + right.submit_ms,
            wait_ms: left.wait_ms + right.wait_ms,
        }),
        (Some(left), None) => Some(left),
        (None, right) => right,
    }
}

/// Clears the dirty flag of every bitmap in `uploaded` after a batch upload.
///
/// A failed batch did not reach the backend, so the bitmaps stay dirty and the
/// next frame retries them; clearing the flag anyway would present a frame
/// that silently omits their textures.
fn clear_uploaded_dirty(scene: &mut Scene, uploaded: &[u32], succeeded: bool) {
    if !succeeded {
        return;
    }
    for id in uploaded {
        scene.set_bitmap_clean(*id);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Engine, EngineParts, FrameTarget, GameFrame, PixelTranslation, PresentFault,
        PresentationStall, RenderPolicy, Scene, StallAge, StallCause, StallSummary,
        add_upload_timings, clear_uploaded_dirty, describe_stall_causes, present_outcome,
        reported_surface_size,
    };
    use crate::Rect;
    use crate::hasher::{Map, map};
    use api::geometry::PixelRegion;
    use api::host::{PresentSkip, Presentation, SurfaceEvent};
    use render::{
        NullRenderPipeline, PipelineConfig, RenderContent, RenderNode, RenderPipeline, Texture,
        TextureDesc, UploadTimings, begin_render_state, create_render_pipeline, create_texture,
    };
    use testkit::headless_window;

    /// A frame hook that changes nothing.
    struct NoFrame;

    impl GameFrame for NoFrame {
        fn before_render(&mut self, _now_ms: u64, _dt_ms: f64) -> Result<(), String> {
            Ok(())
        }
    }

    /// Builds a headless engine over the null backend.
    fn headless_engine(size: (u32, u32)) -> Engine {
        let pipeline = create_render_pipeline(PipelineConfig::null()).expect("null pipeline");
        let handle = headless_window();
        let (surface, render_state) = begin_render_state(&pipeline, &handle).expect("surface");
        Engine::new(EngineParts {
            pipeline,
            target: FrameTarget::Surface(surface),
            render_state,
            surface_size: size,
            headless: true,
            overlay: None,
        })
        .expect("engine")
    }

    /// Builds a headless engine over the null backend, with the sampler's
    /// filter derived from `policy`.
    ///
    /// The filter is the one thing the policy reaches: the samplers are
    /// device resources created with the pipeline, so the composition root
    /// derives the filter from the policy and hands the same value to the
    /// engine.
    fn headless_engine_with_policy(size: (u32, u32), policy: RenderPolicy) -> Engine {
        let pipeline = create_render_pipeline(PipelineConfig {
            filter: policy.texture_filter(),
            ..PipelineConfig::null()
        })
        .expect("null pipeline");
        let handle = headless_window();
        let (surface, render_state) = begin_render_state(&pipeline, &handle).expect("surface");
        Engine::new(EngineParts {
            pipeline,
            target: FrameTarget::Surface(surface),
            render_state,
            surface_size: size,
            headless: true,
            overlay: None,
        })
        .expect("engine")
        .with_policy(policy)
    }

    /// The projected tree, the translation and the uploaded textures of an
    /// engine with `policy`, after one frame.
    ///
    /// The window is half the size of the framebuffer, so the Fit mapping
    /// scales by two and the root transform is not the identity.
    fn projected_tree(
        policy: RenderPolicy,
    ) -> (Vec<RenderNode>, Option<PixelTranslation>, Map<u32, Texture>) {
        let mut engine = headless_engine_with_policy((64, 64), policy);
        let mut hooks = NoFrame;
        let window = engine.scene_mut().add_window("w", (32, 32));
        let layer = engine.scene_mut().add_layer(window, None);
        let bitmap = engine.scene_mut().add_bitmap(8, 8, vec![0u8; 8 * 8 * 4]);
        let state = engine.scene_mut().layer_mut(layer).expect("layer");
        state.rect = crate::Rect::new(0, 0, 8, 8);
        state.visible = true;
        state.bitmap = Some(bitmap);
        state.image_width = 8;
        state.image_height = 8;
        engine.frame(0, &mut hooks).expect("frame");
        (
            engine.render_scene().nodes.clone(),
            engine.translation(),
            engine.textures().clone(),
        )
    }

    /// The picture a texture handle names.
    ///
    /// The handle names a device allocation, so two engines that draw the
    /// same picture hold two handles and a handle is never equal to another
    /// one. The picture is the bitmap the handle holds — the engine's table
    /// says which — together with the descriptor the sampler reads, so two
    /// handles name the same picture when they hold the same bitmap with the
    /// same descriptor.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct Picture {
        /// The bitmap the engine's table holds the handle for, when it holds
        /// one.
        bitmap: Option<u32>,
        /// The descriptor the sampler reads.
        desc: TextureDesc,
    }

    /// The tree's nodes with their texture handles replaced by canonical
    /// ones.
    ///
    /// Every handle is replaced by the one canonical handle for its picture,
    /// so two trees that draw the same pictures compare equal and two trees
    /// that draw different ones do not. Every other field of the node is
    /// compared as it is.
    fn canonical_picture(
        nodes: &[RenderNode],
        sources: &Map<u32, Texture>,
        handles: &mut Map<Picture, Texture>,
        pipeline: &RenderPipeline,
    ) -> Vec<RenderNode> {
        nodes
            .iter()
            .map(|node| {
                let mut node = node.clone();
                if let RenderContent::Texture(texture) = &node.content {
                    node.content = RenderContent::Texture(canonical_handle(
                        handles,
                        pipeline,
                        picture_of(texture, sources),
                    ));
                }
                node
            })
            .collect()
    }

    /// The picture a handle names.
    fn picture_of(texture: &Texture, sources: &Map<u32, Texture>) -> Picture {
        let bitmap = sources
            .iter()
            .find(|(_bitmap, held)| **held == *texture)
            .map(|(bitmap, _held)| *bitmap);
        Picture {
            bitmap,
            desc: *texture.desc(),
        }
    }

    /// The canonical handle for one picture.
    ///
    /// The handle is created once per picture and shared by every tree the
    /// comparison reads, so the same picture names the same handle and a
    /// different picture names a different one.
    fn canonical_handle(
        handles: &mut Map<Picture, Texture>,
        pipeline: &RenderPipeline,
        picture: Picture,
    ) -> Texture {
        if let Some(texture) = handles.get(&picture) {
            return texture.clone();
        }
        let texture = create_texture(pipeline, picture.desc).expect("the canonical texture");
        handles.insert(picture, texture.clone());
        texture
    }

    /// The rendering policy does not move the picture the engine projects.
    ///
    /// The policy is a rendering choice and not a script-visible one: the
    /// projected tree — its nodes, their `dst` and `src`, their transform,
    /// their affine, their blend, their opacity, their colour, their `z`,
    /// their children and their order, and the root's Fit transform — and
    /// the translation are the same with the policy on and off. The policy's
    /// only reach is the sampler's filter, which the composition root
    /// derives from it when it creates the pipeline.
    ///
    /// The one field that may differ is the texture handle the node draws:
    /// a handle names a device allocation, so the two engines hold two
    /// handles for the same picture and a handle is never equal to another
    /// one. The comparison replaces every handle with the canonical handle
    /// for the picture it names, so the picture is compared and the
    /// allocation is not.
    #[test]
    fn the_policy_does_not_move_the_projected_tree() {
        assert_ne!(
            RenderPolicy::Reference.texture_filter(),
            RenderPolicy::NativeResolution.texture_filter(),
            "the two policies sample the textures differently"
        );
        let (reference_nodes, reference_translation, reference_sources) =
            projected_tree(RenderPolicy::Reference);
        let (native_nodes, native_translation, native_sources) =
            projected_tree(RenderPolicy::NativeResolution);
        assert!(
            !reference_nodes.is_empty(),
            "the window's layer is projected into a node"
        );
        // The two engines uploaded the same pictures: the tables agree,
        // bitmap by bitmap.
        for (bitmap, texture) in &reference_sources {
            let native = native_sources
                .get(bitmap)
                .expect("both engines uploaded the same bitmap");
            assert_eq!(
                texture.desc(),
                native.desc(),
                "the bitmap {bitmap} is the same picture in both engines"
            );
        }
        let pipeline = create_render_pipeline(PipelineConfig::null()).expect("canonical pipeline");
        let mut handles: Map<Picture, Texture> = map();
        let reference_picture = canonical_picture(
            &reference_nodes,
            &reference_sources,
            &mut handles,
            &pipeline,
        );
        let native_picture =
            canonical_picture(&native_nodes, &native_sources, &mut handles, &pipeline);
        assert_eq!(
            reference_picture, native_picture,
            "the projected tree is the same picture"
        );
        assert_eq!(
            reference_translation, native_translation,
            "the translation is the same mapping"
        );
    }

    /// The engine holds the policy it was built with, so the composition root
    /// can report what the run draws with.
    #[test]
    fn the_engine_holds_its_rendering_policy() {
        let engine = headless_engine((64, 64));
        assert_eq!(engine.policy(), RenderPolicy::Reference);
        let engine = engine.with_policy(RenderPolicy::NativeResolution);
        assert_eq!(engine.policy(), RenderPolicy::NativeResolution);
    }

    /// A bitmap that changed in a rectangle uploads that rectangle alone.
    ///
    /// The bitmap is the one a layer shows: the copies the device holds
    /// follow the layers, so the bitmap of a layer is the one that is
    /// uploaded.
    #[test]
    fn a_dirty_region_uploads_only_its_own_pixels() {
        let mut engine = headless_engine((64, 64));
        let mut hooks = NoFrame;
        let id = engine.scene_mut().add_bitmap(8, 8, vec![0u8; 8 * 8 * 4]);
        {
            let scene = engine.scene_mut();
            let window = scene.add_window("test", (64, 64));
            let layer = scene.add_layer(window, None);
            let state = scene.layer_mut(layer).expect("the layer");
            state.visible = true;
            state.rect = Rect::new(0, 0, 8, 8);
            state.bitmap = Some(id);
        }

        // The first frame creates the texture, so it takes every pixel.
        engine.frame(0, &mut hooks).expect("first frame");
        let texture = engine.textures().get(&id).expect("texture").clone();
        let uploaded = NullRenderPipeline::texture_uploaded_bytes(&engine.device.state, &texture)
            .expect("uploaded bytes");
        assert_eq!(uploaded, 8 * 8 * 4, "a new texture takes every pixel");

        // The next frame uploads the marked rectangle alone.
        engine
            .scene_mut()
            .bitmap_mut(id)
            .expect("bitmap")
            .mark_dirty_rect(PixelRegion::new(2, 2, 3, 3));
        engine.frame(16, &mut hooks).expect("second frame");
        let uploaded = NullRenderPipeline::texture_uploaded_bytes(&engine.device.state, &texture)
            .expect("uploaded bytes");
        assert_eq!(
            uploaded,
            8 * 8 * 4 + 3 * 3 * 4,
            "only the marked rectangle is uploaded"
        );
        assert!(
            !engine.scene().bitmap(id).expect("bitmap").dirty,
            "an uploaded bitmap is clean"
        );
    }

    /// A failed upload keeps the bitmap's dirty region for the retry.
    #[test]
    fn a_failed_upload_keeps_the_dirty_region_for_the_retry() {
        let mut scene = Scene::default();
        let id = scene.add_bitmap(4, 4, vec![0u8; 4 * 4 * 4]);
        scene.set_bitmap_clean(id);
        scene
            .bitmap_mut(id)
            .expect("bitmap")
            .mark_dirty_rect(PixelRegion::new(1, 1, 2, 2));

        clear_uploaded_dirty(&mut scene, &[id], false);
        let bitmap = scene.bitmap(id).expect("bitmap");
        assert!(bitmap.dirty, "a batch that failed is retried");
        assert_eq!(
            bitmap.dirty_region,
            Some(PixelRegion::new(1, 1, 2, 2)),
            "the failed region is uploaded again"
        );

        clear_uploaded_dirty(&mut scene, &[id], true);
        let bitmap = scene.bitmap(id).expect("bitmap");
        assert!(!bitmap.dirty, "an uploaded bitmap is clean");
        assert_eq!(bitmap.dirty_region, None, "a clean bitmap has no region");
    }

    #[test]
    fn a_failed_upload_leaves_every_bitmap_dirty_for_a_retry() {
        let mut scene = Scene::default();
        let id = scene.add_bitmap(1, 1, vec![0u8; 4]);
        assert!(scene.bitmap(id).expect("bitmap").dirty);

        clear_uploaded_dirty(&mut scene, &[id], false);
        assert!(
            scene.bitmap(id).expect("bitmap").dirty,
            "a batch that did not reach the backend must be retried"
        );

        clear_uploaded_dirty(&mut scene, &[id], true);
        assert!(!scene.bitmap(id).expect("bitmap").dirty);
    }

    /// The device composite of a layer's subtree: the tree is drawn on the
    /// device into a surface of the content's size, and the release drops
    /// that surface.
    ///
    /// The compatibility layer asks for the composite and for the release;
    /// the engine owns the device and runs the requests with it, so the scene
    /// never holds the device.
    #[test]
    fn the_device_composite_draws_into_a_surface_the_release_drops() {
        let mut engine = headless_engine((32, 32));
        let window = engine.scene_mut().add_window("device", (32, 32));
        let root = engine.scene_mut().add_layer(window, None);
        let child = engine.scene_mut().add_layer(window, Some(root));
        let root_image = engine
            .scene_mut()
            .add_bitmap(32, 32, vec![40u8; 32 * 32 * 4]);
        let child_image = engine.scene_mut().add_bitmap(8, 8, vec![80u8; 8 * 8 * 4]);
        engine
            .scene_mut()
            .update_layer(root, |state| {
                state.rect = crate::Rect::new(0, 0, 32, 32);
                state.visible = true;
                state.bitmap = Some(root_image);
                true
            })
            .expect("the root layer is updated");
        engine
            .scene_mut()
            .update_layer(child, |state| {
                state.rect = crate::Rect::new(4, 4, 8, 8);
                state.visible = true;
                state.bitmap = Some(child_image);
                true
            })
            .expect("the child layer is updated");

        // The frame uploads the images, so the composite has textures to
        // sample.
        let mut hooks = NoFrame;
        engine.frame(0, &mut hooks).expect("the frame");

        // The compatibility layer's request: the engine runs it with the
        // device, on the frame thread.
        engine.scene_mut().request_device_composite(root);
        engine
            .frame(16, &mut hooks)
            .expect("the frame with the composite");
        assert_eq!(
            engine.device.surfaces.len(),
            1,
            "the composite drew the subtree into a surface"
        );
        let content = engine
            .scene_mut()
            .subtree_content_id(root)
            .expect("the subtree's content");
        let pixels = engine
            .scene()
            .bitmap(content)
            .expect("the content bitmap")
            .rgba
            .clone();
        assert!(
            pixels.chunks(4).any(|pixel| *pixel != [0, 0, 0, 0]),
            "the composite wrote the subtree's picture into the content"
        );

        // The transition's teardown releases the endpoints' surfaces.
        engine.scene_mut().request_device_surface_release(root);
        engine.scene_mut().request_device_surface_release(child);
        engine
            .frame(32, &mut hooks)
            .expect("the frame with the release");
        assert_eq!(
            engine.device.surfaces.len(),
            0,
            "the surfaces the transition needed are released"
        );
    }

    /// A release for a layer the device never drew into finds no surface, so
    /// the device holds none for it.
    #[test]
    fn releasing_a_surface_that_was_never_drawn_reports_nothing() {
        let mut engine = headless_engine((32, 32));
        let window = engine.scene_mut().add_window("device", (32, 32));
        let root = engine.scene_mut().add_layer(window, None);
        let mut hooks = NoFrame;
        engine.scene_mut().request_device_surface_release(root);
        engine
            .frame(0, &mut hooks)
            .expect("the frame with the release");
        assert_eq!(
            engine.device.surfaces.len(),
            0,
            "the device holds no surface"
        );
    }

    /// A tree the device refuses is composited on the CPU: the device has no
    /// bitmap to sample a mask plane from, and the CPU path draws the tree.
    #[test]
    fn a_tree_the_device_refuses_is_composited_on_the_cpu() {
        let mut engine = headless_engine((32, 32));
        let window = engine.scene_mut().add_window("device", (32, 32));
        let root = engine.scene_mut().add_layer(window, None);
        let child = engine.scene_mut().add_layer(window, Some(root));
        let root_image = engine
            .scene_mut()
            .add_bitmap(32, 32, vec![40u8; 32 * 32 * 4]);
        let child_image = engine.scene_mut().add_bitmap(8, 8, vec![80u8; 8 * 8 * 4]);
        engine
            .scene_mut()
            .update_layer(root, |state| {
                state.rect = crate::Rect::new(0, 0, 32, 32);
                state.visible = true;
                state.bitmap = Some(root_image);
                // The mask face: the device cannot sample the plane the
                // coverage is derived from.
                state.face = 2;
                true
            })
            .expect("the root layer is updated");
        engine
            .scene_mut()
            .update_layer(child, |state| {
                state.rect = crate::Rect::new(4, 4, 8, 8);
                state.visible = true;
                state.bitmap = Some(child_image);
                true
            })
            .expect("the child layer is updated");

        let mut hooks = NoFrame;
        engine.frame(0, &mut hooks).expect("the frame");
        engine.scene_mut().request_device_composite(root);
        engine
            .frame(16, &mut hooks)
            .expect("the frame with the composite");

        let content = engine
            .scene_mut()
            .subtree_content_id(root)
            .expect("the subtree's content");
        let pixels = engine
            .scene()
            .bitmap(content)
            .expect("the content bitmap")
            .rgba
            .clone();
        assert!(
            pixels.chunks(4).any(|pixel| *pixel != [0, 0, 0, 0]),
            "the CPU composite drew the tree the device refused"
        );
    }

    /// The frame uploads its dirty bitmaps twice, so the frame's report holds
    /// the sum of the two batches. A backend that measured only one of them
    /// contributes that one.
    #[test]
    fn the_frame_reports_the_sum_of_its_two_uploads() {
        let first = UploadTimings {
            create_ms: 1.0,
            stage_ms: 2.0,
            submit_ms: 3.0,
            wait_ms: 4.0,
        };
        let second = UploadTimings {
            create_ms: 10.0,
            stage_ms: 20.0,
            submit_ms: 30.0,
            wait_ms: 40.0,
        };
        let sum = add_upload_timings(Some(first), Some(second)).expect("both measured");
        assert_eq!(sum.create_ms, 11.0);
        assert_eq!(sum.stage_ms, 22.0);
        assert_eq!(sum.submit_ms, 33.0);
        assert_eq!(sum.wait_ms, 44.0);

        assert!(add_upload_timings(None, None).is_none(), "no measurement");
        let only = add_upload_timings(None, Some(second)).expect("one measured");
        assert_eq!(only.stage_ms, 20.0);
    }

    #[test]
    fn a_resize_event_reports_the_extent_the_surface_accepted() {
        assert_eq!(
            reported_surface_size(SurfaceEvent::Resized(640, 480), (1280, 720)),
            (640, 480)
        );
        // A non-resize event leaves the requested size in place.
        assert_eq!(
            reported_surface_size(SurfaceEvent::OutOfDate, (1280, 720)),
            (1280, 720)
        );
    }

    #[test]
    fn a_frame_the_backend_did_not_present_is_reported() {
        // reference/cpp/core/visual/impl/VSyncTimingThread.cpp:114,141-142
        // The reference presents on every vsync wake; a wake that produced no
        // frame is a skipped, reported frame rather than a normal one.
        let outcome = present_outcome(Presentation {
            presented: false,
            surface_event: Some(SurfaceEvent::OutOfDate),
            skip: None,
        });
        assert!(!outcome.presented);
        assert_eq!(
            outcome.fault,
            Some(PresentFault::Surface(SurfaceEvent::OutOfDate))
        );

        let normal = present_outcome(Presentation {
            presented: true,
            surface_event: None,
            skip: None,
        });
        assert!(normal.presented);
        assert_eq!(normal.fault, None);
    }

    #[test]
    fn a_frame_the_display_never_released_is_skipped_and_reported() {
        // reference/cpp/core/visual/impl/BasicDrawDevice.cpp:244
        // The reference's present never blocks on the display, so an image the
        // presentation engine does not release is a missed frame, not a stall
        // and not a surface fault.
        let skipped = present_outcome(Presentation {
            presented: false,
            surface_event: None,
            skip: Some(PresentSkip::AcquireTimeout),
        });
        assert!(!skipped.presented);
        assert_eq!(skipped.surface_event, None);
        assert_eq!(
            skipped.fault,
            Some(PresentFault::Skipped(PresentSkip::AcquireTimeout))
        );
    }

    #[test]
    fn a_stall_names_every_cause_it_saw_and_its_span() {
        // reference/cpp/core/visual/impl/VSyncTimingThread.cpp:114,141-142
        // The reference presents on every vsync wake; a wake that produced no
        // frame is a skipped frame, so one stall is the run of such wakes and
        // its length is the frames they produced.
        let mut stall = PresentationStall::default();
        assert!(!stall.active());
        assert_eq!(
            stall.resumed(5_000),
            None,
            "a run that never skipped has no span to report"
        );

        let skip = StallCause::Backend(PresentFault::Skipped(PresentSkip::AcquireTimeout));
        assert_eq!(stall.record(skip, 1_000), Some(skip));
        assert_eq!(
            stall.record(skip, 1_008),
            None,
            "a cause already in the run is named once"
        );

        let out_of_date = StallCause::Backend(PresentFault::Surface(SurfaceEvent::OutOfDate));
        assert_eq!(stall.record(out_of_date, 1_016), Some(out_of_date));
        assert_eq!(stall.record(out_of_date, 1_024), None);
        // The unfittable mapping keeps the same frame count, so a run that
        // changes cause does not lose the frames it already skipped.
        assert_eq!(
            stall.record(StallCause::NoMapping, 1_032),
            Some(StallCause::NoMapping)
        );

        assert_eq!(
            stall.resumed(2_032),
            Some(StallSummary {
                frames: 5,
                elapsed_ms: 1_032,
                causes: vec![skip, out_of_date, StallCause::NoMapping],
            })
        );
        assert!(!stall.active(), "the resume ends the run");
        assert_eq!(stall.resumed(2_040), None);
    }

    #[test]
    fn a_stall_reports_its_age_on_a_bounded_schedule() {
        let mut stall = PresentationStall::default();
        let cause = StallCause::Backend(PresentFault::Skipped(PresentSkip::AcquireTimeout));
        assert_eq!(stall.record(cause, 0), Some(cause));

        assert_eq!(
            stall.due_report(999),
            None,
            "nothing is reported before the first interval"
        );
        assert_eq!(
            stall.due_report(1_000),
            Some(StallAge {
                frames: 1,
                elapsed_ms: 1_000,
            })
        );
        assert_eq!(
            stall.due_report(1_100),
            None,
            "an age is reported once per interval"
        );
        assert_eq!(stall.record(cause, 10_900), None);
        assert_eq!(
            stall.due_report(11_000),
            Some(StallAge {
                frames: 2,
                elapsed_ms: 11_000,
            })
        );
    }

    #[test]
    fn a_stall_summary_names_each_cause_once() {
        let causes = [
            StallCause::Backend(PresentFault::Skipped(PresentSkip::AcquireTimeout)),
            StallCause::NoMapping,
        ];
        assert_eq!(
            describe_stall_causes(&causes),
            "the display did not release a swapchain image within the presentation budget; \
             the game surface and the framebuffer cannot be fitted"
        );
    }

    #[test]
    fn a_lost_surface_is_reported_as_a_surface_fault_that_recovery_owns() {
        // The engine recovers from `surface_event`, never by rebuilding the
        // swapchain a second time inside the frame.
        let lost = present_outcome(Presentation {
            presented: false,
            surface_event: Some(SurfaceEvent::Lost),
            skip: None,
        });
        assert!(!lost.presented);
        assert_eq!(lost.fault, Some(PresentFault::Surface(SurfaceEvent::Lost)));

        let unreported = present_outcome(Presentation {
            presented: false,
            surface_event: None,
            skip: None,
        });
        assert_eq!(unreported.fault, Some(PresentFault::Unreported));
    }
}
