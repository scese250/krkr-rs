//! The gameplay frame-time distribution, on the Vulkan backend or the null one.
//!
//! The `gameplay_frames` harness measures the frames with the null backend
//! and the null backend's own clock, whose `elapsed_ms` is the time since the
//! run started rather than the time since the previous frame. The engine's
//! clock is the wall clock either way, so the frames themselves run the same
//! scenario; the difference is in the audio pump, which renders `elapsed_ms`
//! of sound per frame, and in the slow-frame report, which reads the same
//! number as the frame's gap. This harness gives every backend the same
//! clock, the same click schedule and the same pacing, so the two
//! distributions can be compared.
//!
//! The run drives a route through the game's own pages. The title page's
//! buttons are read from the layout the game ships (`uipsd/title.csv`), and
//! the click that presses one of them is delivered in the game's logical
//! pixels — the 1920x1080 space the script sees — so the same click reaches
//! the same button whatever the backend's surface size is. The page the
//! route loads is read back from the log, so the run proves the route it
//! drove: the page the route names is the page the run reached.
//!
//! The route has two clicks. The warm-up delivers the click in the middle of
//! the screen, the click that moves the game's opening sequence along; it
//! ends when the title page is up. The route then delivers the click that
//! presses the title page's button, until the page that button loads has
//! been loaded. The warm-up's click is not the button's place: the opening
//! sequence does not move along for a click on the title page's menu.
//!
//! The null backend runs headless and is the default: a measurement never
//! opens a window. The Vulkan backend has two shapes. The headless one
//! (`--headless`) draws every frame into an offscreen target on the device
//! and opens nothing; the windowed one presents to a real window. The
//! windowed shape is the shipping-path harness, and a measurement uses the
//! headless one, because a measurement never puts a window on the user's
//! desktop.
//!
//! Run: KRKR_TEST_GAME=<dir> cargo run -p app --example gameplay_frames_vulkan -- \
//!       [--backend vulkan|null] [--headless] [--frames N] [--fps N] \
//!       [--scene NAME] [--click-ms N]
//!
//! `--fps 0` drives the frames as fast as the host runs them, so the gap is
//! the engine's cost and not the presentation cadence; `--fps N` paces them
//! at N frames per second, the way the shipping windowed run does.
//!
//! `--first-click-ms N` is the game time after which the first click that
//! moves the game's opening sequence along is due, and `--click-ms N` the
//! game-time period between the clicks. The click that presses the route's
//! button is due once the title page is up, so the route follows the game
//! rather than a clock.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app::{BackendKind, FrameTimeSource, GameHost, Host, HostConfig};
use engine::PixelTranslation;
use log::{Log, Metadata, Record as LogRecord};
use platform::{
    EventHandler, Flow, FramePacing, InputEvent, MouseButton, PlatformConfig, PlatformEvent,
    PlatformWindow, Ticker,
};

/// The frames the run drives when the command line does not say.
const DEFAULT_FRAMES: usize = 6000;

/// The frame gap at which a frame is reported as slow, in milliseconds.
///
/// The default is the application's own (250 ms), so a plain measurement is
/// not perturbed by the reports: a report's own cost lands in the frame after
/// it, and the tail is what the report is for. A run that measures the tail
/// asks for the budget of its target instead - 34 ms is the 30 fps budget - and
/// says so in the output.
const SLOW_FRAME_MS: u64 = 250;

/// The frames per second the run paces at when the command line does not say.
const DEFAULT_FPS: u32 = 60;

/// The size of the offscreen target the headless Vulkan host draws into.
///
/// The size is the game's logical screen, so the presentation mapping is the
/// identity: a click in the game's logical pixels lands on the same device
/// pixel, and the frame the run measures is the frame the game would show
/// on a display of that size.
const HEADLESS_SURFACE_SIZE: (u32, u32) = (1920, 1080);

/// The game-time period between the clicks.
const DEFAULT_CLICK_MS: u64 = 300;

/// The frame the first click is due on, the `gameplay_frames` harness's
/// schedule.
const FIRST_CLICK_FRAME: usize = 30;

/// The frames between the clicks, the `gameplay_frames` harness's schedule.
const CLICK_PERIOD_FRAMES: usize = 600;

/// The game time after which the first click is due.
///
/// The title has to be up before the click that starts the scenario lands,
/// and the title's readiness is a wall-clock thing.
const FIRST_CLICK_MS: u64 = 1000;

/// The wait between the title page's arrival and the click that presses the
/// route's button, in milliseconds.
///
/// The title page's buttons are created before its scenario is loaded, so the
/// wait is a margin and not a requirement.
const TITLE_SETTLE_MS: u64 = 300;

/// The prefix the scenario parser writes when it loads a scenario.
const SCENARIO_PREFIX: &str = "KAGParser: scenario loaded : ";

/// The prefix the slow-frame report writes.
const SLOW_FRAME_PREFIX: &str = "app.frame: slow frame=";

/// The clock the frames are paced from.
///
/// The frame's delta is what the engine accumulates into its own clock
/// (`engine::Engine::run` takes the delta from
/// [`FrameTimeSource::elapsed_ms`]), so the delta decides how fast the game
/// runs.
///
/// Two shapes are available:
///
/// * `gap`: the delta is the time since the previous frame — the shipping
///   windowed loop's shape, and the one the engine expects;
/// * `cumulative`: the delta is the time since the run started — the
///   `gameplay_frames` harness's shape, which makes the engine's clock run
///   ahead (it accumulates the elapsed time on every frame), and with it the
///   game's timers, the transitions and the audio.
struct RunClock {
    /// The instant the run started.
    started: Instant,
    /// The delta the next frame carries.
    delta_ms: f64,
}

impl RunClock {
    /// A clock whose first frame carries no delta.
    fn new() -> Self {
        Self {
            started: Instant::now(),
            delta_ms: 0.0,
        }
    }

    /// Restarts the clock at now.
    ///
    /// The game is opened after the clock exists — the host holds the
    /// source's address — and the open takes seconds, so the clock's origin
    /// moves to the moment the game is open: the run's clock starts when
    /// the game is open, the way the `gameplay_frames` harness's does.
    fn reset(&mut self) {
        self.started = Instant::now();
        self.delta_ms = 0.0;
    }

    /// The shape of the delta.
    fn set_delta(&mut self, gap_ms: f64, cumulative: bool) {
        self.delta_ms = if cumulative { self.now_ms() } else { gap_ms };
    }
}

impl FrameTimeSource for RunClock {
    fn elapsed_ms(&self) -> f64 {
        self.delta_ms
    }

    fn now_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
}

/// One button of the title page.
///
/// The game lays its title menu out in the `uipsd/title.csv` file it ships:
/// every row is `type,<name>,<class>,<state>,<x>,<y>,<width>,<height>`, the
/// rows of the menu sit in the game's layout 46 pixels apart, and each
/// button is 256 pixels wide and 45 pixels high. The
/// `main/title.func` file says what a button does: the `system2` button jumps
/// to the `*option` label of `title.ks`, which loads the settings page.
struct TitleButton {
    /// The name the layout file gives the button.
    name: &'static str,
    /// The page the button's action loads.
    loads: &'static str,
    /// The button's top edge, in the layout's pixels.
    top: i32,
}

/// The scale from the layout's screen to the game's logical screen.
///
/// The layout the game ships is drawn on a 1280x720 screen, and the game
/// runs on a 1920x1080 one, so the layout is drawn 1.5 times as large: a
/// button's row is 384 by 67.5 pixels on the game's screen.
const LAYOUT_SCALE: f32 = 1.5;

/// The click's offset inside a button, in the layout's pixels.
///
/// The click lands on the button's own picture, which sits in the left part
/// of the button's row: 110 pixels from the left edge and 25 pixels below
/// the row's top. In the game's logical pixels that is (165, 960) for the
/// settings button, whose row is the screen band 922.5 to 990.
const CLICK_OFFSET: (f32, f32) = (110.0, 25.0);

impl TitleButton {
    /// The point a click lands on to press the button: the game's logical
    /// pixels.
    fn point(&self) -> (f32, f32) {
        (
            CLICK_OFFSET.0 * LAYOUT_SCALE,
            (self.top as f32 + CLICK_OFFSET.1) * LAYOUT_SCALE,
        )
    }
}

/// The title page's buttons, in the order the layout file lists them.
///
/// The layout has two sets: the short one (`start1` … `exit1`) and the long
/// one (`restart2` … `exit2`). The long one is the set the game shows while
/// the title has not been cleared, and it is the one that carries the
/// settings button.
const TITLE_BUTTONS: &[TitleButton] = &[
    TitleButton {
        name: "restart2",
        loads: "start.ks",
        top: 339,
    },
    TitleButton {
        name: "start2",
        loads: "start.ks",
        top: 385,
    },
    TitleButton {
        name: "load2",
        loads: "load.ks",
        top: 431,
    },
    TitleButton {
        name: "continue2",
        loads: "load.ks",
        top: 477,
    },
    TitleButton {
        name: "extra2",
        loads: "extra.ks",
        top: 523,
    },
    TitleButton {
        name: "after2",
        loads: "start.ks",
        top: 569,
    },
    TitleButton {
        name: "system2",
        loads: "option.ks",
        top: 615,
    },
    TitleButton {
        name: "exit2",
        loads: "",
        top: 661,
    },
];

/// The title button the layout file names.
fn button_named(name: &str) -> Option<&'static TitleButton> {
    TITLE_BUTTONS.iter().find(|button| button.name == name)
}

/// A place the run can click.
///
/// The position is in the game's logical pixels — the space the script sees —
/// and the run maps it to the device pixels the platform's input events carry.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ClickTarget {
    /// The horizontal position, in the game's logical pixels.
    x: f32,
    /// The vertical position, in the game's logical pixels.
    y: f32,
}

/// The click in the middle of the screen.
///
/// The opening sequence moves along on a click in the middle of the screen,
/// and the scenario's text advances on the same click. The route's warm-up
/// delivers it.
const CENTRE_CLICK: ClickTarget = ClickTarget { x: 640.0, y: 360.0 };

/// The scene the run drives.
///
/// The title page is a KAG scene with a column of buttons on the left of the
/// screen; each of them opens another scene. The run drives one of them, so
/// the run crosses a scene boundary — the case that costs the most in the
/// game, and the one a text-advancing run never reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    /// The scenario: a click in the middle of the screen advances the text.
    Scenario,
    /// The title page's settings button: it opens the settings page.
    Settings,
    /// The title page's extra button: it opens the extra scene.
    Extra,
    /// The title page's load button: it opens the save-data screen.
    Load,
}

impl Scene {
    /// The title button the scene presses, `None` for the scenario scene.
    fn button(self) -> Option<&'static TitleButton> {
        match self {
            Self::Scenario => None,
            Self::Settings => button_named("system2"),
            Self::Extra => button_named("extra2"),
            Self::Load => button_named("load2"),
        }
    }

    /// The click position of the scene, in the game's logical pixels.
    fn target(self) -> ClickTarget {
        match self.button() {
            Some(button) => {
                let (x, y) = button.point();
                ClickTarget { x, y }
            }
            None => CENTRE_CLICK,
        }
    }

    /// The page the scene's button loads, `None` for the scenario scene.
    fn target_script(self) -> Option<&'static str> {
        self.button().map(|button| button.loads)
    }

    /// The scene a name names.
    fn parse(name: &str) -> Option<Self> {
        match name {
            "scenario" => Some(Self::Scenario),
            "settings" => Some(Self::Settings),
            "extra" => Some(Self::Extra),
            "load" => Some(Self::Load),
            _ => None,
        }
    }
}

/// One log record the run keeps: the frame it was written on and its text.
#[derive(Clone, Debug)]
struct Captured {
    /// The frame the run was on when the record was written.
    frame: usize,
    /// The record's text.
    text: String,
}

/// The records the run keeps, shared with the logger.
type Records = Arc<Mutex<Vec<Captured>>>;

/// The text of a record the run keeps, `None` for the records it does not.
///
/// The two kinds are the scenario loads — the proof that a click reached the
/// page it names — and the slow-frame reports, which name a frame's costs.
fn kept_text(record: &LogRecord<'_>) -> Option<String> {
    let message = record.args().to_string();
    if message.starts_with(SCENARIO_PREFIX) || message.starts_with(SLOW_FRAME_PREFIX) {
        Some(message)
    } else {
        None
    }
}

/// The page a load record names, `None` for a record that is not one.
fn scenario_of(text: &str) -> Option<&str> {
    text.strip_prefix(SCENARIO_PREFIX)
}

/// Whether a record is a slow-frame report.
///
/// The report names the frame the gap spans, which is the frame before the
/// one the report was written on.
fn is_slow_frame(text: &str) -> bool {
    text.starts_with(SLOW_FRAME_PREFIX)
}

/// The logger the run installs.
///
/// Every record is written the way the process logger writes it, and the two
/// kinds the report needs are kept with the frame they were written on.
struct RunLogger {
    /// The logger the records are written through.
    inner: env_logger::Logger,
    /// The records the report keeps.
    records: Records,
    /// The frame the run is on.
    frame: Arc<AtomicUsize>,
}

impl Log for RunLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &LogRecord<'_>) {
        if let Some(text) = kept_text(record) {
            let mut records = match self.records.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            records.push(Captured {
                frame: self.frame.load(Ordering::Relaxed),
                text,
            });
        }
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// The phase of the route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The opening sequence is still running: the clicks move it along.
    Warmup,
    /// The title page is up: the clicks press the route's button.
    Target,
    /// The route is done: the run settles and stops.
    Done,
}

/// The click schedule: when the next click is due.
///
/// The clicks are paced in game time, or in frames when a frame period is
/// given. The frame period replaces the game-time one, the way the
/// `gameplay_frames` harness's schedule works.
#[derive(Clone, Copy)]
struct Schedule {
    /// The game time the first click is due at.
    first_ms: u64,
    /// The game-time period between the clicks.
    period_ms: u64,
    /// The frame the first click is due on.
    first_frame: usize,
    /// The number of frames between the clicks, zero when the clicks are
    /// paced in game time alone.
    period_frames: usize,
}

impl Schedule {
    /// Whether a click is due on the frame.
    fn due(
        &self,
        now_ms: u64,
        frame: usize,
        started: bool,
        last_ms: u64,
        last_frame: usize,
    ) -> bool {
        if self.period_frames > 0 {
            if started {
                frame.saturating_sub(last_frame) >= self.period_frames
            } else {
                frame >= self.first_frame
            }
        } else if started {
            now_ms.saturating_sub(last_ms) >= self.period_ms
        } else {
            now_ms >= self.first_ms
        }
    }
}

/// The route the run drives: the clicks it delivers and the page that proves
/// it.
///
/// The run has two phases. The warm-up phase clicks in the middle of the
/// screen, the click that moves the game's opening sequence along; it ends
/// when the title page is up. The route phase clicks the button the scene
/// names, until the page that button loads has been loaded.
struct Route {
    /// The page the route's button loads, `None` when the scene names none.
    target: Option<&'static str>,
    /// The page whose load ends the warm-up phase.
    title: &'static str,
    /// The click the warm-up phase delivers: it moves the opening along.
    warmup_click: ClickTarget,
    /// The click the route phase delivers: it presses the route's button.
    target_click: ClickTarget,
    /// The phase the route is in.
    phase: Phase,
    /// When the next click is due.
    schedule: Schedule,
    /// The game time the last click was delivered at.
    last_ms: u64,
    /// The frame the last click was delivered on.
    last_frame: usize,
    /// Whether a click of the current phase was delivered.
    started: bool,
    /// The wait between the title page's arrival and the route's first click.
    settle_ms: u64,
    /// The records the run kept.
    records: Records,
    /// The frame the route's page was loaded on.
    reached: Option<usize>,
}

impl Route {
    /// A route that drives the scene.
    ///
    /// An explicit click replaces the scene's own button click: the run
    /// presses the button where the command line says, and the page that
    /// button loads is the page the route proves. The warm-up's click is the
    /// run's own, and the command line does not change it.
    fn new(
        scene: Scene,
        explicit: Option<ClickTarget>,
        records: Records,
        schedule: Schedule,
        settle_ms: u64,
    ) -> Self {
        let target_click = explicit.unwrap_or_else(|| scene.target());
        Self {
            target: scene.target_script(),
            title: "title.ks",
            warmup_click: CENTRE_CLICK,
            target_click,
            phase: Phase::Warmup,
            schedule,
            last_ms: 0,
            last_frame: 0,
            started: false,
            settle_ms,
            records,
            reached: None,
        }
    }

    /// The frame the named page was loaded on, `None` when it was not.
    fn loaded_frame(&self, name: &str) -> Option<usize> {
        let records = match self.records.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        records
            .iter()
            .find(|record| scenario_of(&record.text) == Some(name))
            .map(|record| record.frame)
    }

    /// Moves the route to the phase the game is in.
    ///
    /// The warm-up ends when the title page is up, and the route ends when
    /// the page the route's button loads has been loaded. The route follows
    /// the game rather than a clock, so a slow machine does not make the run
    /// click a page that is not there yet.
    fn advance(&mut self, now_ms: u64, frame: usize) {
        match self.phase {
            Phase::Warmup => {
                if self.loaded_frame(self.title).is_some() {
                    self.phase = Phase::Target;
                    self.started = false;
                    self.schedule.first_ms = now_ms.saturating_add(self.settle_ms);
                    self.last_ms = now_ms;
                    self.last_frame = frame;
                }
            }
            Phase::Target => {
                if let Some(target) = self.target
                    && let Some(loaded) = self.loaded_frame(target)
                {
                    self.phase = Phase::Done;
                    self.reached = Some(loaded);
                }
            }
            Phase::Done => {}
        }
    }

    /// The click this frame delivers, `None` when the frame delivers none.
    fn click(&mut self, now_ms: u64, frame: usize) -> Option<ClickTarget> {
        self.advance(now_ms, frame);
        let target = match self.phase {
            Phase::Warmup => self.warmup_click,
            Phase::Target => self.target_click,
            Phase::Done => return None,
        };
        if !self
            .schedule
            .due(now_ms, frame, self.started, self.last_ms, self.last_frame)
        {
            return None;
        }
        self.started = true;
        self.last_ms = now_ms;
        self.last_frame = frame;
        Some(target)
    }

    /// Whether the route's page has been loaded.
    fn is_reached(&self) -> bool {
        self.reached.is_some()
    }
}

/// One click the run delivered.
#[derive(Clone, Copy, Debug)]
struct Click {
    /// The frame the click was delivered on.
    frame: usize,
    /// The horizontal position, in the device pixels the platform's events
    /// carry.
    x: f32,
    /// The vertical position, in the device pixels the platform's events
    /// carry.
    y: f32,
}

/// What one run measured, and how it ended.
#[derive(Default)]
struct Shared {
    /// The wall-clock gap between the frames: the previous frame's end to
    /// this frame's end, the same gap the null harness measures.
    gaps: Vec<f64>,
    /// The engine's own work in the frame, without the platform's wait.
    steps: Vec<f64>,
    /// The platform's wait before the frame.
    polls: Vec<f64>,
    /// The frame index of each measured frame.
    indexes: Vec<usize>,
    /// The number of frames the engine presented.
    presented: u64,
    /// The clicks the run delivered.
    clicks: Vec<Click>,
    /// The failure that stopped the run.
    failure: Option<String>,
}

/// The run's own parameters: what is mounted, how long the run drives and
/// how the frames are paced.
///
/// The Vulkan path and the null path both read the same run, so the
/// parameters have one owner.
struct RunConfig {
    /// The archives the game is mounted from.
    archives: Vec<PathBuf>,
    /// The game's base directory.
    base_dir: PathBuf,
    /// The frames the run drives.
    frames: usize,
    /// The interval the frames are paced at.
    interval: Duration,
    /// The route the run drives.
    route: Route,
    /// Whether the frame's delta is the time since the run started.
    cumulative: bool,
    /// The time the run waits after every frame.
    frame_delay_ms: u64,
    /// The frame gap at which a frame is reported, in milliseconds.
    slow_frame_ms: f64,
    /// The wait the run takes after the route is done.
    settle_ms: u64,
    /// The frame the run is on, as the logger sees it.
    frame_counter: Arc<AtomicUsize>,
}

/// The harness: the game, the clock and the measurements.
struct Harness {
    /// The archives the game is mounted from.
    archives: Vec<PathBuf>,
    /// The game's base directory.
    base_dir: PathBuf,
    /// The frames the run drives.
    frames: usize,
    /// The presentation budget the host is created with.
    presentation_budget: Duration,
    /// The frame's clock source: the engine's clock reads it, and the
    /// frame's own delta is written into it before every frame.
    clock: Box<RunClock>,
    /// Whether the frame's delta is the time since the run started.
    cumulative: bool,
    /// The time the run waits after every frame.
    frame_delay_ms: u64,
    /// The frame gap at which a frame is reported, in milliseconds.
    slow_frame_ms: f64,
    /// The wait the run takes after the route is done.
    settle_ms: u64,
    /// The game, once the window exists.
    game: Option<GameHost>,
    /// The measurements and the failure, shared with the runner.
    shared: Rc<RefCell<Shared>>,
    /// The route the run drives.
    route: Route,
    /// The frame the run is on, as the logger sees it.
    frame_counter: Arc<AtomicUsize>,
    /// The instant the game opened, the origin of the click schedule's game
    /// time.
    opened: Option<Instant>,
    /// The frame the run is on.
    index: usize,
    /// The instant the previous frame ended.
    last_end: Option<Instant>,
    /// The game time the route was done at.
    reached_at: Option<u64>,
}

impl Harness {
    /// The game time in milliseconds: the time since the game opened.
    fn game_time_ms(&self) -> u64 {
        self.opened
            .map_or(0, |opened| (opened.elapsed().as_secs_f64() * 1000.0) as u64)
    }

    /// Records one frame's measurements.
    fn record(&mut self, gap: f64, step_ms: f64, poll_ms: f64, presented: bool) {
        let mut shared = self.shared.borrow_mut();
        shared.gaps.push(gap);
        shared.steps.push(step_ms);
        shared.polls.push(poll_ms);
        shared.indexes.push(self.index);
        if presented {
            shared.presented = shared.presented.saturating_add(1);
        }
        drop(shared);
        self.index = self.index.saturating_add(1);
    }
}

/// The events a click delivers, and where it landed.
///
/// The click is given in the game's logical pixels and lands in the device
/// pixels the platform's input events carry, so the mapping the frame
/// presents with is the mapping the click is taken back through.
fn click_events(
    frame: usize,
    target: ClickTarget,
    translation: Option<PixelTranslation>,
) -> (Vec<PlatformEvent>, Click) {
    let (x, y) = match translation {
        Some(translation) => translation.to_device_point((target.x, target.y)),
        None => (target.x, target.y),
    };
    let events = vec![
        PlatformEvent::Input(InputEvent::PointerMoved { x, y }),
        PlatformEvent::Input(InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        }),
        PlatformEvent::Input(InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: false,
        }),
    ];
    (events, Click { frame, x, y })
}

impl EventHandler for Harness {
    fn resumed(&mut self, window: &PlatformWindow) -> Flow {
        let window_handle = match window.raw_window_handle() {
            Ok(handle) => handle,
            Err(error) => {
                self.shared.borrow_mut().failure =
                    Some(format!("cannot obtain the window handle: {error}"));
                return Flow::Exit;
            }
        };
        let display_handle = match window.raw_display_handle() {
            Ok(handle) => handle,
            Err(error) => {
                self.shared.borrow_mut().failure =
                    Some(format!("cannot obtain the display handle: {error}"));
                return Flow::Exit;
            }
        };
        let mut config = HostConfig::default().with_backend(BackendKind::Vulkan);
        config.surface_size = window.size();
        config.window_handle = Some(window_handle);
        config.display_handle = Some(display_handle);
        config.headless = false;
        // The presentation budget bounds the wait for a swapchain image. An
        // unpaced run drives the frames faster than the display can hand them
        // out, so it needs a budget the display is not measured against; a
        // paced run uses the cadence's own period, the way the shipping run
        // does.
        config.frame_interval = self.presentation_budget;
        let source: &(dyn FrameTimeSource + 'static) = self.clock.as_ref();
        let source = source as *const (dyn FrameTimeSource + 'static);
        let host = match Host::new(config) {
            Ok(host) => host.with_frame_time_source(source),
            Err(error) => {
                self.shared.borrow_mut().failure = Some(format!("cannot create the host: {error}"));
                return Flow::Exit;
            }
        };
        let archives = self.archives.clone();
        let base_dir = self.base_dir.clone();
        match GameHost::open(host, archives, base_dir) {
            Ok(game) => {
                self.game = Some(
                    game.with_slow_report_budget(None)
                        .with_slow_frame_threshold(self.slow_frame_ms),
                );
                self.clock.reset();
                // The first gap spans the first frame, the way the null
                // harness's first gap spans its first frame: the run's clock
                // starts when the game is open.
                self.last_end = Some(Instant::now());
                self.opened = Some(Instant::now());
                Flow::Continue
            }
            Err(error) => {
                self.shared.borrow_mut().failure = Some(format!("cannot open the game: {error}"));
                Flow::Exit
            }
        }
    }

    fn event(&mut self, event: PlatformEvent) -> Flow {
        match event {
            PlatformEvent::CloseRequested => Flow::Exit,
            _ => Flow::Continue,
        }
    }

    fn frame(&mut self, dt_ms: f32, _window: &PlatformWindow, pacing: FramePacing) -> Flow {
        // The platform's own gap is the engine's elapsed time, the way the
        // shipping windowed loop feeds it.
        self.clock.set_delta(f64::from(dt_ms), self.cumulative);
        let now_ms = self.game_time_ms();
        self.frame_counter.store(self.index, Ordering::Relaxed);
        // No click is due this frame: the frame delivers no event.
        let translation = self
            .game
            .as_ref()
            .and_then(|game| game.host().translation());
        let events = match self.route.click(now_ms, self.index) {
            Some(target) => {
                let (events, click) = click_events(self.index, target, translation);
                self.shared.borrow_mut().clicks.push(click);
                events
            }
            None => Vec::new(),
        };
        let Some(game) = self.game.as_mut() else {
            return Flow::Exit;
        };
        let started = Instant::now();
        let outcome = game.step_with_pacing(self.clock.as_ref(), &events, pacing);
        let step_ms = started.elapsed().as_secs_f64() * 1000.0;
        let report = match outcome {
            Ok(report) => report,
            Err(error) => {
                self.shared.borrow_mut().failure = Some(format!("the frame failed: {error}"));
                return Flow::Exit;
            }
        };
        let now = Instant::now();
        let gap = self
            .last_end
            .map_or(0.0, |last| now.duration_since(last).as_secs_f64() * 1000.0);
        self.last_end = Some(now);
        self.record(gap, step_ms, pacing.poll_ms, report.presented);
        if self.index >= self.frames {
            return Flow::Exit;
        }
        // The route is done: the run settles for a moment and then stops.
        if self.route.is_reached() {
            let reached_at = self.reached_at.get_or_insert(now_ms);
            if now_ms.saturating_sub(*reached_at) >= self.settle_ms {
                return Flow::Exit;
            }
        }
        if self.frame_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.frame_delay_ms));
        }
        Flow::Continue
    }
}

/// Prints the distribution of one measured series.
fn report(name: &str, values: &[f64]) {
    if values.is_empty() {
        println!("{name}: no frames");
        return;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.total_cmp(right));
    let at = |fraction: f64| -> f64 {
        let index = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
        sorted[index]
    };
    let p50 = at(0.50);
    let p90 = at(0.90);
    let p99 = at(0.99);
    let worst = *sorted.last().unwrap_or(&0.0);
    // The 1% low: the rate the slowest 1% of the frames run at.
    let slow: Vec<f64> = sorted.iter().copied().filter(|gap| *gap >= p99).collect();
    let one_low = slow.iter().sum::<f64>() / slow.len().max(1) as f64;
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    println!("{name}: {} frames", sorted.len());
    println!("  the mean: {mean:.2} ms");
    println!("  the p50: {p50:.2} ms ({:.1} fps)", 1000.0 / p50);
    println!("  the p90: {p90:.2} ms ({:.1} fps)", 1000.0 / p90);
    println!("  the p99: {p99:.2} ms ({:.1} fps)", 1000.0 / p99);
    println!(
        "  the 1% low: {one_low:.2} ms ({:.1} fps)",
        1000.0 / one_low
    );
    println!("  the worst: {worst:.2} ms ({:.1} fps)", 1000.0 / worst);
    println!(
        "  the frames over 33 ms: {}",
        sorted.iter().filter(|value| **value > 33.3).count()
    );
    println!(
        "  the frames over 100 ms: {}",
        sorted.iter().filter(|value| **value > 100.0).count()
    );
}

/// The dominant cost a slow-frame report names, `None` when it names none.
///
/// The report's header carries the cost that dominates the frame the gap
/// spans, as `dominant=<section>.<name>`.
fn dominant_cost(text: &str) -> Option<String> {
    let header = text.lines().next()?;
    let rest = header.strip_prefix(SLOW_FRAME_PREFIX)?;
    let at = rest.find(" dominant=")?;
    let tail = &rest[at + " dominant=".len()..];
    let end = tail.find(' ')?;
    Some(tail[..end].to_owned())
}

/// Prints what the slow frames were spent on.
///
/// Every slow-frame report names the cost that dominates the frame it
/// spans, so the summary says what the tail of the run was made of, ranked
/// by how many frames each cost dominated.
fn report_dominant_costs(records: &[Captured]) {
    let mut costs: Vec<(String, usize)> = Vec::new();
    for record in records {
        if !is_slow_frame(&record.text) {
            continue;
        }
        let Some(dominant) = dominant_cost(&record.text) else {
            continue;
        };
        match costs.iter_mut().find(|(name, _)| *name == dominant) {
            Some((_, count)) => *count += 1,
            None => costs.push((dominant, 1)),
        }
    }
    if costs.is_empty() {
        println!("the slow frames' dominant costs: no frame was slow");
        return;
    }
    costs.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    println!("the slow frames' dominant costs:");
    for (name, count) in costs {
        println!("  {name}: {count} frames");
    }
}

/// Prints the route the run drove and the frames around the transition.
///
/// The pages the run loaded are the pages the log recorded, and the click
/// that was delivered on a frame's own page is the click that reached it.
fn report_route(shared: &Shared, records: &[Captured], scene: Scene) {
    println!("the pages the run loaded:");
    for record in records {
        let Some(page) = scenario_of(&record.text) else {
            continue;
        };
        match shared
            .clicks
            .iter()
            .find(|click| click.frame == record.frame)
        {
            Some(click) => println!(
                "  frame {}: {page} (the click at ({:.1}, {:.1}) in device pixels)",
                record.frame, click.x, click.y
            ),
            None => println!("  frame {}: {page}", record.frame),
        }
    }
    let reached = records.iter().find_map(|record| {
        let page = scenario_of(&record.text)?;
        if scene.target_script() == Some(page) {
            Some(record.frame)
        } else {
            None
        }
    });
    let Some(reached) = reached else {
        println!("the route: the page the route names was not reached");
        return;
    };
    println!(
        "the route: {} was loaded on frame {reached}",
        scene.target_script().unwrap_or("the page")
    );
    let Some(position) = shared.indexes.iter().position(|index| *index == reached) else {
        return;
    };
    let first = position.saturating_sub(5);
    let last = (position + 5).min(shared.gaps.len().saturating_sub(1));
    println!("the frames around the transition:");
    for index in first..=last {
        println!(
            "  frame {}: the gap {:.1} ms, the engine's own work {:.1} ms",
            shared.indexes[index], shared.gaps[index], shared.steps[index]
        );
    }
    println!("the slow-frame reports around the transition:");
    for record in records {
        if !is_slow_frame(&record.text) {
            continue;
        }
        // The report names the frame the gap spans, which is the frame
        // before the one the report was written on.
        let spanned = record.frame.saturating_sub(1);
        if spanned < reached.saturating_sub(5) || spanned > reached + 5 {
            continue;
        }
        for line in record.text.lines() {
            println!("  {line}");
        }
    }
}

/// Runs the game offscreen on `backend` and returns the measurements.
///
/// The null backend composites the frame on the CPU and the Vulkan backend
/// draws it on the device; both run the same loop, the same click schedule
/// and the same reports, and neither opens a window: the frame goes into an
/// offscreen target and the game reads it the way a windowed run reads the
/// presented frame.
fn run_offscreen(run: RunConfig, shared: Rc<RefCell<Shared>>, backend: BackendKind) {
    let RunConfig {
        archives,
        base_dir,
        frames,
        interval,
        mut route,
        cumulative,
        frame_delay_ms,
        slow_frame_ms,
        settle_ms,
        frame_counter,
    } = run;
    let mut clock = Box::new(RunClock::new());
    let mut config = HostConfig::headless().with_backend(backend);
    if backend == BackendKind::Vulkan {
        // The Vulkan backend draws into a target of the surface size, and
        // the target is the size of the game's logical screen, so the
        // presentation mapping is the identity and a click in the game's
        // logical pixels lands on the same device pixel.
        config = config.with_surface_size(HEADLESS_SURFACE_SIZE);
    }
    // The frame period is the cadence the loop paces at, and it is the
    // budget a frame may spend waiting for the display. A headless host has
    // no display, so the budget is never spent.
    config.frame_interval = interval;
    let host = match Host::new(config) {
        Ok(host) => host,
        Err(error) => {
            shared.borrow_mut().failure = Some(format!("cannot create the host: {error}"));
            return;
        }
    };
    let source: &(dyn FrameTimeSource + 'static) = clock.as_ref();
    let source = source as *const (dyn FrameTimeSource + 'static);
    let host = host.with_frame_time_source(source);
    let mut game = match GameHost::open(host, archives, base_dir) {
        Ok(game) => game
            .with_slow_report_budget(None)
            .with_slow_frame_threshold(slow_frame_ms),
        Err(error) => {
            shared.borrow_mut().failure = Some(format!("cannot open the game: {error}"));
            return;
        }
    };
    clock.reset();
    let mut ticker = Ticker::new(interval, Instant::now());
    let opened = Instant::now();
    let mut last_end = Instant::now();
    let mut last_start = Instant::now();
    let mut reached_at: Option<u64> = None;
    for frame in 0..frames {
        // The frame is due: the loop waits for the deadline, so the gap the
        // run measures is the cadence's, the way the shipping loop's is.
        loop {
            let now = Instant::now();
            if ticker.poll(now) {
                break;
            }
            std::thread::sleep(ticker.deadline().saturating_duration_since(now));
        }
        // The frame's own delta: the time since the previous frame started.
        clock.set_delta(last_start.elapsed().as_secs_f64() * 1000.0, cumulative);
        last_start = Instant::now();
        let now_ms = (opened.elapsed().as_secs_f64() * 1000.0) as u64;
        frame_counter.store(frame, Ordering::Relaxed);
        // No click is due this frame: the frame delivers no event.
        let translation = game.host().translation();
        let events = match route.click(now_ms, frame) {
            Some(target) => {
                let (events, click) = click_events(frame, target, translation);
                shared.borrow_mut().clicks.push(click);
                events
            }
            None => Vec::new(),
        };
        let started = Instant::now();
        let outcome = game.step(clock.as_ref(), &events);
        let step_ms = started.elapsed().as_secs_f64() * 1000.0;
        let report = match outcome {
            Ok(report) => report,
            Err(error) => {
                shared.borrow_mut().failure = Some(format!("the frame failed: {error}"));
                return;
            }
        };
        let now = Instant::now();
        let gap = now.duration_since(last_end).as_secs_f64() * 1000.0;
        last_end = now;
        if frame_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(frame_delay_ms));
        }
        let mut shared = shared.borrow_mut();
        shared.gaps.push(gap);
        shared.steps.push(step_ms);
        shared.polls.push(0.0);
        shared.indexes.push(frame);
        if report.presented {
            shared.presented = shared.presented.saturating_add(1);
        }
        drop(shared);
        // The route is done: the run settles for a moment and then stops.
        if route.is_reached() {
            let reached = reached_at.get_or_insert(now_ms);
            if now_ms.saturating_sub(*reached) >= settle_ms {
                break;
            }
        }
    }
}

/// Reads one command-line argument as a number.
///
/// A missing argument is the default; a malformed one is an error the run
/// reports and leaves on.
fn number<T>(args: &mut impl Iterator<Item = String>, name: &str, default: T) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match args.next() {
        Some(value) => match value.parse() {
            Ok(parsed) => parsed,
            Err(error) => {
                eprintln!("the {name} is not a number: {error}");
                std::process::exit(2);
            }
        },
        None => default,
    }
}

fn main() {
    let Ok(dir) = std::env::var("KRKR_TEST_GAME") else {
        eprintln!("set KRKR_TEST_GAME to the game directory");
        std::process::exit(2);
    };
    let dir = PathBuf::from(dir);
    let archives = testkit::game_archives(&dir).expect("the archives");
    let mut frames = DEFAULT_FRAMES;
    let mut fps = DEFAULT_FPS;
    let mut click_ms = DEFAULT_CLICK_MS;
    let mut first_click_ms = FIRST_CLICK_MS;
    let mut settle_ms = TITLE_SETTLE_MS;
    let mut click_frames = 0usize;
    let mut first_click = FIRST_CLICK_FRAME;
    let mut cumulative = false;
    let mut frame_delay_ms = 0u64;
    let mut click_x: Option<f32> = None;
    let mut click_y: Option<f32> = None;
    let mut scene = Scene::Scenario;
    let mut scene_name = "scenario".to_owned();
    let mut slow_frame_ms = SLOW_FRAME_MS;
    let mut vm_report = false;
    let mut vulkan = false;
    let mut headless = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = number(&mut args, "frame count", DEFAULT_FRAMES),
            "--fps" => fps = number(&mut args, "frame rate", DEFAULT_FPS),
            "--click-ms" => click_ms = number(&mut args, "click period", DEFAULT_CLICK_MS),
            "--first-click-ms" => {
                first_click_ms = number(&mut args, "first click delay", FIRST_CLICK_MS);
            }
            "--settle-ms" => {
                settle_ms = number(&mut args, "title settle delay", TITLE_SETTLE_MS);
            }
            "--click-frames" => {
                click_frames = number(&mut args, "click period in frames", CLICK_PERIOD_FRAMES);
            }
            "--first-click" => {
                first_click = number(&mut args, "first click frame", FIRST_CLICK_FRAME);
            }
            "--scene" => {
                let value = match args.next() {
                    Some(value) => value,
                    None => {
                        eprintln!("--scene needs a value");
                        std::process::exit(2);
                    }
                };
                match Scene::parse(&value) {
                    Some(parsed) => {
                        scene = parsed;
                        scene_name = value;
                    }
                    None => {
                        eprintln!("unknown scene: {value}");
                        std::process::exit(2);
                    }
                }
            }
            "--click-x" => {
                click_x = Some(number(&mut args, "click x", 640.0));
                scene_name = "explicit".to_owned();
            }
            "--click-y" => {
                click_y = Some(number(&mut args, "click y", 360.0));
                scene_name = "explicit".to_owned();
            }
            "--frame-delay-ms" => {
                frame_delay_ms = number(&mut args, "frame delay", 0);
            }
            "--vm-report" => vm_report = true,
            "--slow-frame-ms" => {
                slow_frame_ms = number(&mut args, "slow frame threshold", SLOW_FRAME_MS);
            }
            "--clock" => {
                let value = match args.next() {
                    Some(value) => value,
                    None => {
                        eprintln!("--clock needs a value");
                        std::process::exit(2);
                    }
                };
                cumulative = match value.as_str() {
                    "gap" => false,
                    "cumulative" => true,
                    other => {
                        eprintln!("unknown clock: {other}");
                        std::process::exit(2);
                    }
                };
            }
            "--backend" => {
                let value = match args.next() {
                    Some(value) => value,
                    None => {
                        eprintln!("--backend needs a value");
                        std::process::exit(2);
                    }
                };
                vulkan = match value.as_str() {
                    "vulkan" => true,
                    "null" => false,
                    other => {
                        eprintln!("unknown backend: {other}");
                        std::process::exit(2);
                    }
                };
            }
            "--headless" => headless = true,
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    if vm_report {
        compat::set_compile_stats_enabled(true);
    }
    // The frame interval paces the frames. Zero means the frames are driven
    // as fast as the host runs them, so the gap is the engine's cost and not
    // the presentation cadence.
    let paced = fps > 0;
    let ticker_interval = if paced {
        Duration::from_secs_f64(1.0 / f64::from(fps))
    } else {
        Duration::from_millis(1)
    };
    // A paced run presents at the cadence, so the presentation budget is the
    // cadence's own period; an unpaced one needs a budget the display's
    // refresh rate is not measured against.
    let presentation_budget = if paced {
        ticker_interval
    } else {
        Duration::from_secs(1)
    };
    // The records the run keeps, and the frame the logger sees.
    let records: Records = Arc::new(Mutex::new(Vec::new()));
    let frame_counter = Arc::new(AtomicUsize::new(0));
    // The logger writes every record the way the process logger writes it and
    // keeps the two kinds the report needs.
    let logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    let max_level = logger.filter();
    let run_logger = RunLogger {
        inner: logger,
        records: Arc::clone(&records),
        frame: Arc::clone(&frame_counter),
    };
    log::set_boxed_logger(Box::new(run_logger)).expect("the logger is installed once");
    log::set_max_level(max_level);
    let shared = Rc::new(RefCell::new(Shared::default()));
    // The click the command line names, if it names one.
    let explicit = match (click_x, click_y) {
        (Some(x), Some(y)) => Some(ClickTarget { x, y }),
        (None, None) => None,
        (Some(x), None) => Some(ClickTarget {
            x,
            y: scene.target().y,
        }),
        (None, Some(y)) => Some(ClickTarget {
            x: scene.target().x,
            y,
        }),
    };
    let schedule = Schedule {
        first_ms: first_click_ms,
        period_ms: click_ms,
        first_frame: first_click,
        period_frames: click_frames,
    };
    let route = Route::new(scene, explicit, Arc::clone(&records), schedule, settle_ms);
    let run = RunConfig {
        archives,
        base_dir: dir,
        frames,
        interval: ticker_interval,
        route,
        cumulative,
        frame_delay_ms,
        slow_frame_ms: slow_frame_ms as f64,
        settle_ms,
        frame_counter: Arc::clone(&frame_counter),
    };
    if vulkan && !headless {
        let mut config = PlatformConfig::default();
        config.title = "krkr-rs (vulkan)".to_owned();
        config.size = (1280, 720);
        config.frame_interval = run.interval;
        let harness = Harness {
            archives: run.archives,
            base_dir: run.base_dir,
            frames: run.frames,
            presentation_budget,
            clock: Box::new(RunClock::new()),
            cumulative: run.cumulative,
            frame_delay_ms: run.frame_delay_ms,
            slow_frame_ms: run.slow_frame_ms,
            settle_ms: run.settle_ms,
            game: None,
            shared: Rc::clone(&shared),
            route: run.route,
            frame_counter: Arc::clone(&frame_counter),
            opened: None,
            index: 0,
            last_end: None,
            reached_at: None,
        };
        let outcome = platform::run(config, harness);
        if let Err(error) = outcome {
            eprintln!("krkr: the platform loop failed: {error}");
            std::process::exit(1);
        }
    } else {
        // The run draws into an offscreen target and opens no window: the
        // null backend composites on the CPU and the Vulkan backend draws on
        // the device.
        let backend = if vulkan {
            BackendKind::Vulkan
        } else {
            BackendKind::Null
        };
        run_offscreen(run, Rc::clone(&shared), backend);
    }
    let shared = shared.borrow();
    if let Some(failure) = &shared.failure {
        eprintln!("krkr: {failure}");
        std::process::exit(1);
    }
    println!("the backend: {}", if vulkan { "vulkan" } else { "null" });
    println!(
        "the mode: {}",
        if vulkan && !headless {
            "windowed"
        } else {
            "headless"
        }
    );
    println!(
        "the cadence: {}",
        if paced {
            format!("{fps} fps")
        } else {
            "as fast as the host runs them".to_owned()
        }
    );
    if click_frames > 0 {
        println!(
            "the click schedule: the first click on frame {first_click}, then every \
             {click_frames} frames"
        );
    } else {
        println!(
            "the click schedule: the first click {first_click_ms} ms after the game \
             opened, then every {click_ms} ms"
        );
    }
    let target = explicit.unwrap_or_else(|| scene.target());
    println!(
        "the scene: {scene_name}, the click at ({}, {}) in the game's logical pixels",
        target.x, target.y
    );
    println!(
        "the warm-up click: ({}, {}) in the game's logical pixels",
        CENTRE_CLICK.x, CENTRE_CLICK.y
    );
    println!("the frame delay: {frame_delay_ms} ms");
    println!(
        "the clock: {}",
        if cumulative {
            "the frame's delta is the time since the run started"
        } else {
            "the frame's delta is the time since the previous frame"
        }
    );
    println!("the frames: {}", shared.gaps.len());
    println!("the frames presented: {}", shared.presented);
    println!("the clicks: {}", shared.clicks.len());
    // The first gap spans the first frame, the way the null harness's first
    // gap does; the distribution is the frames after it.
    let startup = shared.gaps.first().copied().unwrap_or(0.0);
    println!("the startup gap: {startup:.1} ms");
    report("the frame gap", shared.gaps.get(1..).unwrap_or(&[]));
    report(
        "the engine's own work",
        shared.steps.get(1..).unwrap_or(&[]),
    );
    report("the platform's wait", shared.polls.get(1..).unwrap_or(&[]));
    if vm_report {
        println!("{}", app::vm_report(&compat::compile_stats()));
    }
    let kept = match records.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    report_route(&shared, &kept, scene);
    report_dominant_costs(&kept);
    // The slowest frames: the frames the run spent its time in.
    let mut order: Vec<usize> = (0..shared.steps.len()).collect();
    order.sort_by(|left, right| shared.steps[*right].total_cmp(&shared.steps[*left]));
    println!("the slowest frames, by the engine's own work:");
    for index in order.iter().take(10) {
        println!(
            "  frame {}: the engine's own work {:.1} ms, the gap {:.1} ms",
            shared.indexes[*index], shared.steps[*index], shared.gaps[*index]
        );
    }
    // The time the run spent in the frames over a threshold: the tail.
    for threshold in [33.3, 100.0] {
        let over: Vec<f64> = shared
            .steps
            .iter()
            .copied()
            .filter(|step| *step > threshold)
            .collect();
        let sum: f64 = over.iter().sum();
        println!(
            "the frames over {threshold:.1} ms: {} ({sum:.1} ms in total)",
            over.len()
        );
    }
}
