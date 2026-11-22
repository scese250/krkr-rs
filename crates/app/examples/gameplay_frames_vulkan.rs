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
//! pixels — the space the script sees — so the same click reaches the
//! same button whatever the backend's surface size is. The layout's rows
//! are the script's rows: the game's UI loader moves the layer the menu
//! lives on to the layout's base row and places every button at its own
//! row relative to that base. The page the route loads is read back from
//! the log, so the run proves the route it drove: the page the route names
//! is the page the run reached.
//!
//! The route has four parts, and each is a part of the run the report names.
//! The warm-up delivers the click in the middle of the screen, the click
//! that moves the game's opening sequence along; it ends when the title
//! page is up. The route then delivers the click that presses the title
//! page's button, until the page that button loads has been loaded. The
//! warm-up's click is not the button's place: the opening sequence does not
//! move along for a click on the title page's menu. The story then runs:
//! the click in the middle of the screen advances the text, and the run
//! reports that part on its own, because the gameplay is the part the
//! reader feels. The last part is the game's own fast-forward: the click
//! that presses the quick menu's skip toggle, and then the story runs on
//! its own.
//!
//! The report names every part: the startup, the title page, the gameplay
//! and the fast-forward. A measurement of the whole run says little about
//! the gameplay, because the startup's own pages and the one-off loads are
//! in it; the gameplay's own distribution is the number a reader wants.
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
//!
//! `--skip-at-frame N` is the frame the fast-forward begins on: the run
//! presses the quick menu's skip toggle on that frame, and the story runs
//! on its own from there.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app::{BackendKind, FrameTimeSource, GameHost, Host, HostConfig, render_policy};
use engine::{PixelTranslation, RenderPolicy};
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

/// The page the scenario's click glyph is loaded from.
///
/// The click glyph is the picture the scenario draws when it waits for the
/// reader, so the load of the glyph is the marker that says the story is
/// on screen and the reader is in control: the gameplay has begun.
const CLICK_GLYPH: &str = "main/clickglyph.asd";

/// The prefix the container plugin writes when it loads a story container.
///
/// The game's story is a container the plugin reads, and the load of the
/// container and the build of its root are the two one-off costs of
/// entering the story. The report names them, so the reader does not have
/// to find them in the title segment.
const CONTAINER_PREFIX: &str = "psb: the ";

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
///
/// A row of the layout file is a row of the game's screen: the game's UI
/// loader moves the layer the menu lives on to the layout's base row and
/// places every button at its own row relative to that base, so the settings
/// button lies in the screen band 615 to 660, at the left edge of the screen.
struct TitleButton {
    /// The name the layout file gives the button.
    name: &'static str,
    /// The page the button's action loads.
    loads: &'static str,
    /// The button's top edge, in the game's logical pixels.
    top: i32,
}

/// The click's offset inside a button, in the game's logical pixels.
///
/// The click lands on the button's own picture, which sits in the left part
/// of the button's row: 110 pixels from the left edge and 25 pixels below
/// the row's top. That is (110, 640) for the settings button, whose row is
/// the screen band 615 to 660.
const CLICK_OFFSET: (f32, f32) = (110.0, 25.0);

impl TitleButton {
    /// The point a click lands on to press the button: the game's logical
    /// pixels.
    fn point(&self) -> (f32, f32) {
        (CLICK_OFFSET.0, self.top as f32 + CLICK_OFFSET.1)
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
/// and the scenario's text advances on the same click: a click on the
/// background advances the story, and the middle of the screen is away from
/// the quick menu and the message window. The route's warm-up and the
/// story's own advance both deliver it.
const CENTRE_CLICK: ClickTarget = ClickTarget { x: 640.0, y: 360.0 };

/// The click that presses the game's own skip.
///
/// The quick menu's skip toggle is the game's own fast-forward: the
/// `uipsd/quickmenu.csv` layout puts the toggle in the screen band 690 to
/// 720, 38 pixels wide at the left edge of the quick menu, and
/// `main/quickmenu.func` binds it to `SystemAction.skip()`, which turns the
/// scenario's own skip mode on. The run presses it and leaves the story to
/// itself: the fast-forward is the game's, not a speed-up of the harness's
/// clock.
const SKIP_CLICK: ClickTarget = ClickTarget {
    x: 1151.0,
    y: 705.0,
};

/// The scene the run drives.
///
/// The title page is a KAG scene with a column of buttons on the left of the
/// screen; each of them opens another scene. The run drives one of them, so
/// the run crosses a scene boundary — the case that costs the most in the
/// game, and the one a text-advancing run never reaches.
///
/// The scenario is the game's story: the run presses the title page's start
/// button, the button that loads the game's scenario entry, and then the
/// story runs and the run advances it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    /// The story: the title page's start button, then the scenario's text.
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
            Self::Scenario => button_named("start2"),
            Self::Settings => button_named("system2"),
            Self::Extra => button_named("extra2"),
            Self::Load => button_named("load2"),
        }
    }

    /// Whether the scene drives the run into the story and keeps it there.
    ///
    /// The story is the one scene the run does not stop at: the run's goal
    /// is the story's own page, and the run drives on from there.
    fn is_story(self) -> bool {
        matches!(self, Self::Scenario)
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
/// The three kinds are the scenario loads — the proof that a click reached
/// the page it names — the story container's own loads, the one-off costs of
/// entering the story, and the slow-frame reports, which name a frame's
/// costs.
fn kept_text(record: &LogRecord<'_>) -> Option<String> {
    let message = record.args().to_string();
    if message.starts_with(SCENARIO_PREFIX)
        || message.starts_with(SLOW_FRAME_PREFIX)
        || message.starts_with(CONTAINER_PREFIX)
    {
        Some(message)
    } else {
        None
    }
}

/// The page a load record names, `None` for a record that is not one.
fn scenario_of(text: &str) -> Option<&str> {
    text.strip_prefix(SCENARIO_PREFIX)
}

/// Whether a load record names the story's click glyph.
///
/// The glyph is the picture the scenario draws when it waits for the reader,
/// so the load of the glyph says the story is on screen.
fn is_click_glyph(text: &str) -> bool {
    scenario_of(text).is_some_and(|page| page.ends_with(CLICK_GLYPH))
}

/// Whether a record is a slow-frame report.
///
/// The report names the frame the gap spans, which is the frame before the
/// one the report was written on.
fn is_slow_frame(text: &str) -> bool {
    text.starts_with(SLOW_FRAME_PREFIX)
}

/// Whether a record is a story container's own load.
///
/// The container plugin writes the load of the container and the build of
/// its root; the two are the one-off costs of entering the story.
fn is_container_load(text: &str) -> bool {
    text.starts_with(CONTAINER_PREFIX)
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
///
/// The phase is what the run's clicks do; the segment is what the report
/// names. They are two things because the story's own load sits between
/// the title page and the gameplay: the run is already driving the story
/// while the story is still loading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The opening sequence is still running: the clicks move it along.
    Warmup,
    /// The title page is up: the clicks press the route's button.
    Title,
    /// The story is running: the clicks advance the text.
    Story,
    /// The story is running with the game's own skip on.
    FastForward,
    /// The route is done: the run settles and stops.
    Done,
}

/// The part of the run a frame belongs to.
///
/// The parts are the ones a reader asks about: how the game starts, what
/// the title page costs, how the gameplay behaves, and what the game's own
/// fast-forward costs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Segment {
    /// The game's opening: the startup sequence and the title page's load.
    Startup,
    /// The title page: the buttons, and the story's entry.
    Title,
    /// The story: the text the reader advances.
    Gameplay,
    /// The fast-forward: the story with the game's own skip on.
    FastForward,
}

impl Segment {
    /// The name the report gives the segment.
    fn name(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Title => "title",
            Self::Gameplay => "gameplay",
            Self::FastForward => "fast-forward",
        }
    }

    /// Every segment, in the order the run drives them.
    fn all() -> [Segment; 4] {
        [
            Segment::Startup,
            Segment::Title,
            Segment::Gameplay,
            Segment::FastForward,
        ]
    }
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

/// The markers the run reads out of the log.
///
/// The markers are found once, as the records arrive, and kept: the route
/// asks the markers whether the title page is up, whether the page the
/// route's button loads has been loaded, and whether the story is on
/// screen.
struct Markers {
    /// The records the run kept.
    records: Records,
    /// The number of records the markers have looked at.
    scanned: usize,
    /// The frame the title page was loaded on.
    title: Option<usize>,
    /// The frame the route's page was loaded on.
    target: Option<usize>,
    /// The frame the story was on screen: the scenario waited for the reader.
    story: Option<usize>,
}

impl Markers {
    /// A set of markers that reads the run's records.
    fn new(records: Records) -> Self {
        Self {
            records,
            scanned: 0,
            title: None,
            target: None,
            story: None,
        }
    }

    /// Reads the records the run has written since the last look.
    ///
    /// The records are appended in order, so the markers read only the new
    /// ones: a long run does not read the whole log on every frame.
    fn scan(&mut self, title: &str, target: Option<&str>) {
        let mut found_title = None;
        let mut found_target = None;
        let mut found_story = None;
        {
            let records = match self.records.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            for record in records.iter().skip(self.scanned) {
                if found_title.is_none() && scenario_of(&record.text) == Some(title) {
                    found_title = Some(record.frame);
                }
                if found_target.is_none() && target == scenario_of(&record.text) {
                    found_target = Some(record.frame);
                }
                if found_story.is_none() && is_click_glyph(&record.text) {
                    found_story = Some(record.frame);
                }
            }
            self.scanned = records.len();
        }
        if found_title.is_some() {
            self.title = found_title;
        }
        if found_target.is_some() {
            self.target = found_target;
        }
        if found_story.is_some() {
            self.story = found_story;
        }
    }
}

/// The route the run drives: the clicks it delivers and the page that proves
/// it.
///
/// The run has four phases. The warm-up phase clicks in the middle of the
/// screen, the click that moves the game's opening sequence along; it ends
/// when the title page is up. The title phase clicks the button the scene
/// names, until the page that button loads has been loaded. The story phase
/// clicks in the middle of the screen, the click that advances the text.
/// The fast-forward phase presses the game's own skip toggle and then leaves
/// the story to itself.
struct Route {
    /// The page the route's button loads, `None` when the scene names none.
    target: Option<&'static str>,
    /// The page whose load ends the warm-up phase.
    title: &'static str,
    /// The click the warm-up phase delivers: it moves the opening along.
    warmup_click: ClickTarget,
    /// The click the story phase delivers: it advances the text.
    advance_click: ClickTarget,
    /// The click the title phase delivers: it presses the route's button.
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
    /// The markers the run reads out of the log.
    markers: Markers,
    /// The frame the route's page was loaded on.
    reached: Option<usize>,
    /// The frame the fast-forward begins on.
    skip_at_frame: Option<usize>,
    /// The click that presses the game's own skip.
    skip_click: ClickTarget,
    /// Whether the run drives on when the route's page has been loaded.
    drives_on: bool,
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
        skip_at_frame: Option<usize>,
    ) -> Self {
        let target_click = explicit.unwrap_or_else(|| scene.target());
        Self {
            target: scene.target_script(),
            title: "title.ks",
            warmup_click: CENTRE_CLICK,
            advance_click: CENTRE_CLICK,
            target_click,
            phase: Phase::Warmup,
            schedule,
            last_ms: 0,
            last_frame: 0,
            started: false,
            settle_ms,
            markers: Markers::new(records),
            reached: None,
            skip_at_frame,
            skip_click: SKIP_CLICK,
            drives_on: scene.is_story(),
        }
    }

    /// Moves the route to the phase the game is in.
    ///
    /// The warm-up ends when the title page is up, and the title phase ends
    /// when the page the route's button loads has been loaded. The route
    /// follows the game rather than a clock, so a slow machine does not make
    /// the run click a page that is not there yet.
    fn advance(&mut self, now_ms: u64, frame: usize) {
        self.markers.scan(self.title, self.target);
        match self.phase {
            Phase::Warmup => {
                if self.markers.title.is_some() {
                    self.phase = Phase::Title;
                    self.started = false;
                    self.schedule.first_ms = now_ms.saturating_add(self.settle_ms);
                    self.last_ms = now_ms;
                    self.last_frame = frame;
                }
            }
            Phase::Title => {
                if let Some(loaded) = self.markers.target {
                    self.reached = Some(loaded);
                    self.phase = if self.drives_on {
                        Phase::Story
                    } else {
                        Phase::Done
                    };
                }
            }
            Phase::Story | Phase::FastForward | Phase::Done => {}
        }
    }

    /// The click this frame delivers, `None` when the frame delivers none.
    fn click(&mut self, now_ms: u64, frame: usize) -> Option<ClickTarget> {
        self.advance(now_ms, frame);
        // The fast-forward: the click that presses the game's own skip. The
        // story runs on its own from here: the run stops clicking, because
        // a click is the game's own "skip the wait", and it would end the
        // skip the segment measures.
        if self.phase == Phase::Story && self.skip_at_frame.is_some_and(|skip_at| skip_at == frame)
        {
            self.phase = Phase::FastForward;
            return Some(self.skip_click);
        }
        let target = match self.phase {
            Phase::Warmup => self.warmup_click,
            Phase::Title => self.target_click,
            Phase::Story => self.advance_click,
            Phase::FastForward | Phase::Done => return None,
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

    /// The part of the run the frame belongs to.
    ///
    /// The gameplay begins when the story is on screen: the container's own
    /// load and the build of its root are one-off costs, and they belong to
    /// the part of the run that entered the story.
    fn segment(&self) -> Segment {
        match self.phase {
            Phase::Warmup => Segment::Startup,
            Phase::Title | Phase::Story => {
                if self.markers.story.is_some() {
                    Segment::Gameplay
                } else {
                    Segment::Title
                }
            }
            Phase::FastForward => Segment::FastForward,
            Phase::Done => Segment::Title,
        }
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
    /// The part of the run each measured frame belongs to.
    segments: Vec<Segment>,
    /// The number of bitmaps the scene held at each frame.
    ///
    /// The scene's bitmaps are the game's own images, and the device keeps
    /// a texture for each of them: the count is what says whether the
    /// device's memory grows with the run.
    bitmaps: Vec<u32>,
    /// The number of frames the engine presented.
    presented: u64,
    /// The clicks the run delivered.
    clicks: Vec<Click>,
    /// The failure that stopped the run.
    failure: Option<String>,
}

impl Shared {
    /// The part of the run the frame belongs to.
    ///
    /// The frames are measured in order and the frame numbers are the
    /// places in the series, so the frame's own number is its place in the
    /// segment series.
    fn segment_of(&self, frame: usize) -> Option<Segment> {
        self.segments.get(frame).copied()
    }
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
    /// The size of the surface the run draws into.
    ///
    /// The default is the game's logical screen, so the presentation mapping
    /// is the identity. A size whose ratio to the logical screen is not one
    /// gives the mapping a scale, and the run then exercises the path a
    /// window on a scaled display takes.
    surface_size: (u32, u32),
    /// The rendering policy the host draws with.
    policy: RenderPolicy,
    /// The file the last frame is written to, when the run writes one.
    dump: Option<PathBuf>,
    /// The frame whose picture the run writes out.
    dump_frame: Option<usize>,
    /// The size the run tells the game its window has.
    ///
    /// A windowed run tells the game the window's size when the compositor
    /// configures it, and the size it tells the game is the window's
    /// physical size. The headless run has no window, so the option says
    /// which size the game is told.
    resize: Option<(u32, u32)>,
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
    /// The rendering policy the host draws with.
    policy: RenderPolicy,
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
    fn record(&mut self, gap: f64, step_ms: f64, poll_ms: f64, presented: bool, segment: Segment) {
        let mut shared = self.shared.borrow_mut();
        shared.gaps.push(gap);
        shared.steps.push(step_ms);
        shared.polls.push(poll_ms);
        shared.indexes.push(self.index);
        shared.segments.push(segment);
        shared.bitmaps.push(self.bitmaps());
        if presented {
            shared.presented = shared.presented.saturating_add(1);
        }
        drop(shared);
        self.index = self.index.saturating_add(1);
    }

    /// The number of bitmaps the scene holds.
    fn bitmaps(&self) -> u32 {
        self.game
            .as_ref()
            .map_or(0, |game| game.host().scene().bitmaps.len() as u32)
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
        config.policy = self.policy;
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
        // The part of the run this frame belongs to, read after the click:
        // the click that presses the game's own skip opens the fast-forward,
        // and that frame is the fast-forward's first.
        let segment = self.route.segment();
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
        self.record(gap, step_ms, pacing.poll_ms, report.presented, segment);
        if self.index >= self.frames {
            return Flow::Exit;
        }
        // The route is done: the run settles for a moment and then stops.
        // The story route drives on: its goal is the story's own page, and
        // the run measures the story itself.
        if self.route.is_reached() && !self.route.drives_on {
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

/// The positions of the frames one segment holds.
fn segment_positions(shared: &Shared, segment: Segment) -> Vec<usize> {
    shared
        .segments
        .iter()
        .enumerate()
        .filter(|(_, part)| **part == segment)
        .map(|(position, _)| position)
        .collect()
}

/// Prints the distribution of one segment's frames.
///
/// The whole run's distribution says little about the gameplay: the
/// startup's own pages and the story's one-off load are in it. The
/// segment's own distribution is the number a reader wants.
fn report_segment(shared: &Shared, segment: Segment) {
    let positions = segment_positions(shared, segment);
    if positions.is_empty() {
        println!("the {} segment: no frames", segment.name());
        return;
    }
    let gaps: Vec<f64> = positions
        .iter()
        .map(|position| shared.gaps[*position])
        .collect();
    let steps: Vec<f64> = positions
        .iter()
        .map(|position| shared.steps[*position])
        .collect();
    let first = shared.indexes[positions[0]];
    let last = shared.indexes[*positions.last().expect("the segment has frames")];
    println!(
        "the {} segment: the frames {first} to {last}, {} frames",
        segment.name(),
        positions.len()
    );
    report(
        &format!("the {} segment's frame gap", segment.name()),
        &gaps,
    );
    report(
        &format!("the {} segment's engine work", segment.name()),
        &steps,
    );
}

/// Prints the parts of the run: the frames each one holds and how far the
/// story advanced in it.
///
/// The story waits for the reader: the click glyph is the picture the
/// scenario draws then, so the count of the glyph's loads is the count of
/// the reader's advances. The fast-forward does not wait — the story runs
/// on — so there the story's own containers are the mark that it moved.
fn report_segments(shared: &Shared, records: &[Captured]) {
    println!("the parts of the run:");
    for segment in Segment::all() {
        let positions = segment_positions(shared, segment);
        if positions.is_empty() {
            println!("  {}: no frames", segment.name());
            continue;
        }
        let first = shared.indexes[positions[0]];
        let last = shared.indexes[*positions.last().expect("the segment has frames")];
        let advances = records
            .iter()
            .filter(|record| is_click_glyph(&record.text))
            .filter(|record| shared.segment_of(record.frame) == Some(segment))
            .count();
        let containers = records
            .iter()
            .filter(|record| is_container_load(&record.text))
            .filter(|record| shared.segment_of(record.frame) == Some(segment))
            .count();
        println!(
            "  {}: the frames {first} to {last}, {} frames, the reader's advances {advances}, \
             the story's containers {containers}",
            segment.name(),
            positions.len()
        );
    }
}

/// Prints the story container's own loads.
///
/// The story is a container the plugin reads: the load of the container and
/// the build of its root are the two one-off costs of entering the story,
/// and they are in the part of the run that entered it, not in the
/// gameplay.
fn report_container_loads(records: &[Captured]) {
    let mut loads = 0;
    for record in records {
        if is_container_load(&record.text) {
            loads += 1;
            println!("the story's container: {}", record.text);
        }
    }
    if loads == 0 {
        println!("the story's container: the run loaded no container");
    }
}

/// The machine's load average, as the kernel reports it.
///
/// The load is what says whether the numbers are a measurement: a number
/// taken while the machine was busy is not a result.
///
/// The file is not everywhere, so its absence is a `None`: a host without
/// it has no load to report, and that is not a failure. A file that is
/// there and does not parse is an error, not an absent file: the reader
/// is told what the kernel wrote.
fn load_average() -> Result<Option<(f64, f64, f64)>, String> {
    let text = match std::fs::read_to_string("/proc/loadavg") {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read /proc/loadavg: {error}")),
    };
    let mut parts = text.split_whitespace();
    let mut next = |name: &str| -> Result<f64, String> {
        let value = parts
            .next()
            .ok_or_else(|| format!("/proc/loadavg names no {name}"))?;
        value
            .parse()
            .map_err(|_| format!("/proc/loadavg names a {name} that is not a number: {value}"))
    };
    let one = next("one-minute load")?;
    let five = next("five-minute load")?;
    let fifteen = next("fifteen-minute load")?;
    Ok(Some((one, five, fifteen)))
}

/// Prints the machine's load average.
///
/// The load is the machine's own account of how busy it was: a reader needs
/// it to know whether the numbers beside it are a measurement. A host
/// without the file has no load to report; a file the kernel wrote and the
/// harness cannot read is reported as the error it is.
fn report_load(when: &str) {
    match load_average() {
        Ok(Some((one, five, fifteen))) => {
            println!("the load average {when}: {one:.2} {five:.2} {fifteen:.2}")
        }
        Ok(None) => println!("the load average {when}: the kernel did not report one"),
        Err(error) => eprintln!("the load average {when}: {error}"),
    }
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
        surface_size,
        policy,
        dump,
        dump_frame,
        resize,
    } = run;
    let mut clock = Box::new(RunClock::new());
    // The click the route presses, and the picture the run dumps.
    let target_click = route.target_click;
    let mut dumped: Option<Vec<u8>> = None;
    let mut dumped_frame: Option<usize> = None;
    let mut config = HostConfig::headless().with_backend(backend);
    if backend == BackendKind::Vulkan {
        // The Vulkan backend draws into a target of the surface size, and
        // the target is the size of the game's logical screen, so the
        // presentation mapping is the identity and a click in the game's
        // logical pixels lands on the same device pixel. A surface of
        // another size gives the mapping a scale, and the run then
        // exercises the path a window on a scaled display takes.
        config = config.with_surface_size(surface_size);
    }
    config = config.with_policy(policy);
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
        let mut frame_events = Vec::new();
        // The window's size, as the compositor reports it: the game learns
        // its window's size from the event, and the size is the window's
        // physical size.
        if let Some((width, height)) = resize
            && frame == 0
        {
            frame_events.push(PlatformEvent::Resized(width, height));
        }
        let click = match route.click(now_ms, frame) {
            Some(target) => {
                // The picture the click is pressed on: the frame before the
                // click lands is the last frame of the page the button is
                // on, so the dump shows the button where the picture drew
                // it.
                if dump.is_some() && dump_frame.is_none() && target == target_click {
                    dumped = game.host().read_offscreen_frame().ok().flatten();
                    dumped_frame = Some(frame);
                }
                let (events, click) = click_events(frame, target, translation);
                shared.borrow_mut().clicks.push(click);
                // The mapping the picture is drawn with: the projected tree's
                // root transform, against the mapping the click is taken
                // back through.
                if let Some(translation) = translation {
                    let scene = game.host().render_scene();
                    let root = scene.node(scene.root);
                    let transform = root.map(|node| node.transform);
                    println!(
                        "the click at frame {frame}: the mapping scale={:.3} offset=({:.1}, {:.1}) \
                         vm={}x{} device={}x{}; the picture's root transform {:?}",
                        translation.scale,
                        translation.offset.0,
                        translation.offset.1,
                        translation.vm_size.0,
                        translation.vm_size.1,
                        translation.device_size.0,
                        translation.device_size.1,
                        transform
                    );
                }
                events
            }
            None => Vec::new(),
        };
        frame_events.extend(click);
        let events = frame_events;
        // The part of the run this frame belongs to, read after the click:
        // the click that presses the game's own skip opens the fast-forward,
        // and that frame is the fast-forward's first.
        let segment = route.segment();
        let started = Instant::now();
        let outcome = game.step(clock.as_ref(), &events);
        let step_ms = started.elapsed().as_secs_f64() * 1000.0;
        // The picture of a frame the command line names, so the run can show
        // what the game drew at a chosen moment.
        if dump_frame == Some(frame) {
            dumped = game.host().read_offscreen_frame().ok().flatten();
            dumped_frame = Some(frame);
        }
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
        shared.segments.push(segment);
        shared
            .bitmaps
            .push(game.host().scene().bitmaps.len() as u32);
        if report.presented {
            shared.presented = shared.presented.saturating_add(1);
        }
        drop(shared);
        // The route is done: the run settles for a moment and then stops.
        // The story route drives on: its goal is the story's own page, and
        // the run measures the story itself.
        if route.is_reached() && !route.drives_on {
            let reached = reached_at.get_or_insert(now_ms);
            if now_ms.saturating_sub(*reached) >= settle_ms {
                break;
            }
        }
    }
    // The mapping the run drew and clicked with: the scale and the offset
    // the picture and the click share.
    if let Some(translation) = game.host().translation() {
        println!(
            "the mapping: vm={}x{} device={}x{} scale={:.3} offset=({:.1}, {:.1})",
            translation.vm_size.0,
            translation.vm_size.1,
            translation.device_size.0,
            translation.device_size.1,
            translation.scale,
            translation.offset.0,
            translation.offset.1
        );
    }
    // The picture the run drew: the frame the host's target holds, written
    // out so the place the button is drawn at can be looked at.
    if let Some(path) = dump {
        let media = media::Media::new();
        // The last frame the run drew, beside the frame the click landed on.
        let last = game.host().read_offscreen_frame().ok().flatten();
        if let Some(pixels) = &last {
            let raw = path.with_extension("last.raw");
            if let Err(error) = std::fs::write(&raw, pixels) {
                eprintln!("cannot write {}: {error}", raw.display());
            }
        }
        match dumped {
            Some(pixels) => {
                let width = surface_size.0;
                let height = surface_size.1;
                // The raw pixels beside the picture: the raw file is what a
                // measurement reads, and the picture is what a person looks
                // at.
                let raw = path.with_extension("raw");
                if let Err(error) = std::fs::write(&raw, &pixels) {
                    eprintln!("cannot write {}: {error}", raw.display());
                }
                match media::encode_image(&media, width, height, &pixels, "png") {
                    Ok(bytes) => {
                        if let Err(error) = std::fs::write(&path, bytes) {
                            eprintln!("cannot write {}: {error}", path.display());
                        }
                    }
                    Err(error) => eprintln!("cannot encode the frame: {error}"),
                }
                println!(
                    "the dump: {} frame={} size={}x{}",
                    path.display(),
                    dumped_frame.unwrap_or(0),
                    width,
                    height
                );
            }
            None => eprintln!("the host read no frame"),
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

/// Why a size the command line named is not one.
///
/// The three cases are three different mistakes: a name that is not a size,
/// a dimension that is not a number, and a dimension that is zero. The
/// error carries the name, so the reader is told what to fix.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SizeError {
    /// The name does not have the `WIDTHxHEIGHT` shape.
    NotASize(String),
    /// A dimension of the name is not a number.
    NotANumber(String),
    /// A dimension of the name is zero: a surface with no pixels has no
    /// picture.
    ZeroDimension(String),
}

impl std::fmt::Display for SizeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotASize(value) => {
                write!(formatter, "the size is not WIDTHxHEIGHT: {value}")
            }
            Self::NotANumber(value) => {
                write!(
                    formatter,
                    "a dimension of the size is not a number: {value}"
                )
            }
            Self::ZeroDimension(value) => {
                write!(formatter, "a dimension of the size is zero: {value}")
            }
        }
    }
}

/// Reads a `WIDTHxHEIGHT` size.
///
/// A malformed size is an error the caller reports, with the name the
/// command line gave: the reader is told which of the three mistakes it
/// is. A zero dimension is not the same mistake as a malformed one, and
/// the error says which is which.
fn parse_size(value: &str) -> Result<(u32, u32), SizeError> {
    let Some((width, height)) = value.split_once(['x', 'X']) else {
        return Err(SizeError::NotASize(value.to_owned()));
    };
    let width: u32 = width
        .parse()
        .map_err(|_| SizeError::NotANumber(value.to_owned()))?;
    let height: u32 = height
        .parse()
        .map_err(|_| SizeError::NotANumber(value.to_owned()))?;
    if width == 0 || height == 0 {
        return Err(SizeError::ZeroDimension(value.to_owned()));
    }
    Ok((width, height))
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
    let mut surface_size = HEADLESS_SURFACE_SIZE;
    let mut native_resolution = false;
    let mut dump: Option<PathBuf> = None;
    let mut dump_frame: Option<usize> = None;
    let mut resize: Option<(u32, u32)> = None;
    let mut skip_at_frame: Option<usize> = None;
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
            "--surface" => {
                let value = match args.next() {
                    Some(value) => value,
                    None => {
                        eprintln!("--surface needs a value");
                        std::process::exit(2);
                    }
                };
                match parse_size(&value) {
                    Ok(size) => surface_size = size,
                    Err(error) => {
                        eprintln!("--surface: {error}");
                        std::process::exit(2);
                    }
                }
            }
            "--native-resolution" => native_resolution = true,
            "--skip-at-frame" => {
                skip_at_frame = Some(number(&mut args, "the fast-forward frame", 0));
            }
            "--dump-frame" => {
                dump_frame = Some(number(&mut args, "the frame to dump", 0));
            }
            "--resize" => {
                let value = match args.next() {
                    Some(value) => value,
                    None => {
                        eprintln!("--resize needs a value");
                        std::process::exit(2);
                    }
                };
                match parse_size(&value) {
                    Ok(size) => resize = Some(size),
                    Err(error) => {
                        eprintln!("--resize: {error}");
                        std::process::exit(2);
                    }
                }
            }
            "--dump" => {
                let value = match args.next() {
                    Some(value) => value,
                    None => {
                        eprintln!("--dump needs a value");
                        std::process::exit(2);
                    }
                };
                dump = Some(PathBuf::from(value));
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    // The rendering policy the flag asks for. The mapping that decides what
    // the choice means is the shared one, so the example and the launchers
    // draw the same picture.
    let policy = render_policy(native_resolution);
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
    // The load the machine carries before the run: a reader needs it to
    // know whether the numbers the run produced are a measurement.
    report_load("before the run");
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
    let route = Route::new(
        scene,
        explicit,
        Arc::clone(&records),
        schedule,
        settle_ms,
        skip_at_frame,
    );
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
        surface_size,
        policy,
        dump,
        dump_frame,
        resize,
    };
    if vulkan && !headless {
        let mut config = PlatformConfig::default();
        config.title = "krkr-rs (vulkan)".to_owned();
        config.size = surface_size;
        config.frame_interval = run.interval;
        let harness = Harness {
            archives: run.archives,
            base_dir: run.base_dir,
            frames: run.frames,
            presentation_budget,
            policy: run.policy,
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
    match skip_at_frame {
        Some(frame) => println!(
            "the fast-forward: the skip click at frame {frame}, at ({}, {}) in the game's \
             logical pixels",
            SKIP_CLICK.x, SKIP_CLICK.y
        ),
        None => println!("the fast-forward: the run drives no fast-forward"),
    }
    println!("the frame delay: {frame_delay_ms} ms");
    println!("the surface: {}x{}", surface_size.0, surface_size.1);
    println!(
        "the policy: {}",
        // The policy is named through the shared mapping too, so the example
        // names no variant of its own.
        if policy == render_policy(true) {
            "native resolution"
        } else {
            "reference"
        }
    );
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
    // The scene's bitmaps: the game's own images, each of which the device
    // keeps a texture for.
    match (shared.bitmaps.first(), shared.bitmaps.last()) {
        (Some(first), Some(last)) => {
            println!("the scene's bitmaps: {first} at the first frame, {last} at the last")
        }
        _ => println!("the scene's bitmaps: the run measured no frame"),
    }
    // The load the machine carries after the run: the numbers above are a
    // measurement only when the machine was quiet.
    report_load("after the run");
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
    // The records the run kept: the pages it loaded, the slow frames it
    // reported and the story container's own loads.
    let kept_records = match records.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    // The parts of the run: the startup, the title page, the gameplay and
    // the fast-forward. The gameplay's own distribution is the number a
    // reader wants; the whole run's says little about it.
    report_segments(&shared, &kept_records);
    for segment in Segment::all() {
        report_segment(&shared, segment);
    }
    report_container_loads(&kept_records);
    if vm_report {
        println!("{}", app::vm_report(&compat::compile_stats()));
    }
    let kept = kept_records;
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
    // The report is printed even when the run failed: the frames the run
    // measured before the failure are the measurement, and the failure says
    // where it ended.
    if shared.failure.is_some() {
        std::process::exit(1);
    }
}
