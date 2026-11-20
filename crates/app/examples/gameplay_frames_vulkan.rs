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
//! The Vulkan backend runs in a real window: the platform opens it, the
//! Vulkan surface is bound to it, and the frames are presented to it. The
//! null backend runs headless, the way the `gameplay_frames` harness runs it.
//!
//! Run: KRKR_TEST_GAME=<dir> cargo run -p app --example gameplay_frames_vulkan -- \
//!       [--backend vulkan|null] [--frames N] [--fps N] [--click-frames N]
//!
//! `--fps 0` drives the frames as fast as the host runs them, so the gap is
//! the engine's cost and not the presentation cadence; `--fps N` paces them
//! at N frames per second, the way the shipping windowed run does.
//!
//! `--first-click-ms N` is the game time after which the first click that
//! advances the scenario is due, and `--click-ms N` the game-time period
//! between the clicks. The first click has to land after the title is up,
//! and the title's readiness is a wall-clock thing, not a frame count.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use app::{BackendKind, FrameTimeSource, GameHost, Host, HostConfig};
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

/// The game-time period between the clicks that advance the scenario.
const DEFAULT_CLICK_MS: u64 = 1000;

/// The frame the first click is due on, the `gameplay_frames` harness's
/// schedule.
const FIRST_CLICK_FRAME: usize = 30;

/// The frames between the clicks, the `gameplay_frames` harness's schedule.
const CLICK_PERIOD_FRAMES: usize = 600;

/// The game time after which the first click is due.
///
/// The title has to be up before the click that starts the scenario lands,
/// and the title's readiness is a wall-clock thing.
const FIRST_CLICK_MS: u64 = 2500;

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
    /// The number of clicks the run delivered.
    clicks: u64,
    /// The failure that stopped the run.
    failure: Option<String>,
}

/// A place the run can click.
///
/// The position is in the window's device pixels: the title screen's buttons
/// are a column on the left of the window, and their rows are 65 pixels
/// apart.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ClickTarget {
    /// The horizontal position, in device pixels.
    x: f32,
    /// The vertical position, in device pixels.
    y: f32,
}

/// The scene the run drives.
///
/// The title page is a KAG scene with a column of buttons on the left of the
/// window; each of them opens another scene. The run drives one of them, so
/// the run crosses a scene boundary — the case that costs the most in the
/// game, and the one a text-advancing run never reaches.
///
/// The positions are the ones the game answered to: the click at
/// `(165, 1095)` opens the settings page (`option.ks`), the one at
/// `(165, 992)` opens the extra scene (`extra.ks`), and the one at
/// `(165, 732)` opens the save-data screen (`load.ks`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    /// The scenario: a click in the middle of the screen advances the text.
    Scenario,
    /// The title page's settings button: it opens the settings page
    /// (`option.ks`).
    Settings,
    /// The title page's extra button: it opens the extra scene
    /// (`extra.ks`).
    Extra,
    /// The title page's load button: it opens the save-data screen
    /// (`load.ks`).
    Load,
}

impl Scene {
    /// The click position of the scene.
    fn target(self) -> ClickTarget {
        match self {
            // The middle of the screen: the message window's own area.
            Self::Scenario => ClickTarget { x: 640.0, y: 360.0 },
            // The title page's button column.
            Self::Settings => ClickTarget {
                x: 165.0,
                y: 1095.0,
            },
            Self::Extra => ClickTarget { x: 165.0, y: 992.0 },
            Self::Load => ClickTarget { x: 165.0, y: 732.0 },
        }
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

/// The click schedule: when the next click is due.
///
/// The schedule is either in frames (the `gameplay_frames` harness's
/// schedule: the first click at frame 30, then one every 600 frames) or in
/// game time (the first click `--first-click-ms` after the game opened, then
/// one every `--click-ms` of game time).
#[derive(Clone, Copy)]
struct Clicks {
    /// The game-time period between the clicks.
    period_ms: u64,
    /// The frame schedule's period, in frames.
    period_frames: usize,
    /// The frame the first click is due on.
    first_frame: usize,
    /// The game time after which the first click is due.
    first_ms: u64,
    /// The game time of the last click.
    last_ms: u64,
    /// The frame the last click was delivered on.
    last_frame: usize,
    /// Whether a click was delivered.
    started: bool,
    /// The number of clicks delivered.
    count: u64,
    /// The position the click lands on, in device pixels.
    x: f32,
    y: f32,
}

impl Clicks {
    /// A schedule whose first click is due `first_ms` after the game opened
    /// and whose next ones are every `period_ms` of game time.
    fn new(first_ms: u64, period_ms: u64) -> Self {
        Self {
            period_ms,
            period_frames: 0,
            first_frame: 0,
            first_ms,
            last_ms: 0,
            last_frame: 0,
            started: false,
            count: 0,
            x: 640.0,
            y: 360.0,
        }
    }

    /// A schedule whose first click is due on `first_frame` and whose next
    /// ones are every `period_frames` frames.
    fn every_frames(first_frame: usize, period_frames: usize) -> Self {
        Self {
            period_ms: 0,
            period_frames,
            first_frame,
            first_ms: 0,
            last_ms: 0,
            last_frame: 0,
            started: false,
            count: 0,
            x: 640.0,
            y: 360.0,
        }
    }

    /// Moves the click to a position, in device pixels.
    fn at(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }

    /// The events this frame delivers, if a click is due.
    ///
    /// The title menu is idle, so the run clicks its "start" and then clicks
    /// once per scenario step.
    fn take_due(&mut self, now_ms: u64, frame: usize) -> Option<Vec<PlatformEvent>> {
        let due = if self.period_frames == 0 {
            if self.started {
                now_ms.saturating_sub(self.last_ms) >= self.period_ms
            } else {
                now_ms >= self.first_ms
            }
        } else if self.started {
            frame.saturating_sub(self.last_frame) >= self.period_frames
        } else {
            frame >= self.first_frame
        };
        if !due {
            return None;
        }
        self.started = true;
        self.last_ms = now_ms;
        self.last_frame = frame;
        self.count = self.count.saturating_add(1);
        Some(vec![
            PlatformEvent::Input(InputEvent::PointerMoved {
                x: self.x,
                y: self.y,
            }),
            PlatformEvent::Input(InputEvent::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            }),
            PlatformEvent::Input(InputEvent::MouseButton {
                button: MouseButton::Left,
                pressed: false,
            }),
        ])
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
    /// The click schedule.
    clicks: Clicks,
    /// Whether the frame's delta is the time since the run started.
    cumulative: bool,
    /// The time the run waits after every frame.
    frame_delay_ms: u64,
    /// The frame gap at which a frame is reported, in milliseconds.
    slow_frame_ms: f64,
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
    /// The game, once the window exists.
    game: Option<GameHost>,
    /// The measurements and the failure, shared with the runner.
    shared: Rc<RefCell<Shared>>,
    /// The click schedule.
    clicks: Clicks,
    /// The instant the game opened, the origin of the click schedule's game
    /// time.
    opened: Option<Instant>,
    /// The frame the run is on.
    index: usize,
    /// The instant the previous frame ended.
    last_end: Option<Instant>,
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
        // No click is due this frame: the frame delivers no event.
        let events = self.clicks.take_due(now_ms, self.index).unwrap_or_default();
        self.shared.borrow_mut().clicks = self.clicks.count;
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

/// Runs the game headless on the null backend and returns the measurements.
fn run_null(run: RunConfig, shared: Rc<RefCell<Shared>>) {
    let RunConfig {
        archives,
        base_dir,
        frames,
        interval,
        clicks,
        cumulative,
        frame_delay_ms,
        slow_frame_ms,
    } = run;
    let mut clock = Box::new(RunClock::new());
    let host = match Host::new(HostConfig::headless()) {
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
        Ok(game) => {
            game.with_slow_report_budget(None)
                .with_slow_frame_threshold(slow_frame_ms)
        }
        Err(error) => {
            shared.borrow_mut().failure = Some(format!("cannot open the game: {error}"));
            return;
        }
    };
    clock.reset();
    let mut ticker = Ticker::new(interval, Instant::now());
    let mut clicks = clicks;
    let opened = Instant::now();
    let mut last_end = Instant::now();
    let mut last_start = Instant::now();
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
        // No click is due this frame: the frame delivers no event.
        let events = clicks.take_due(now_ms, frame).unwrap_or_default();
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
    }
    shared.borrow_mut().clicks = clicks.count;
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
    // The slow-frame report is a log line, so the run needs a logger.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
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
    let mut click_frames = 0usize;
    let mut first_click = FIRST_CLICK_FRAME;
    let mut cumulative = false;
    let mut frame_delay_ms = 0u64;
    let mut click_x = 640.0f32;
    let mut click_y = 360.0f32;
    let mut scene_name = "scenario".to_owned();
    let mut slow_frame_ms = SLOW_FRAME_MS;
    let mut vm_report = false;
    let mut vulkan = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = number(&mut args, "frame count", DEFAULT_FRAMES),
            "--fps" => fps = number(&mut args, "frame rate", DEFAULT_FPS),
            "--click-ms" => click_ms = number(&mut args, "click period", DEFAULT_CLICK_MS),
            "--first-click-ms" => {
                first_click_ms = number(&mut args, "first click delay", FIRST_CLICK_MS);
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
                    Some(scene) => {
                        let target = scene.target();
                        click_x = target.x;
                        click_y = target.y;
                        scene_name = value;
                    }
                    None => {
                        eprintln!("unknown scene: {value}");
                        std::process::exit(2);
                    }
                }
            }
            "--click-x" => {
                click_x = number(&mut args, "click x", 640.0);
                scene_name = "explicit".to_owned();
            }
            "--click-y" => {
                click_y = number(&mut args, "click y", 360.0);
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
    let shared = Rc::new(RefCell::new(Shared::default()));
    // The frame schedule is the `gameplay_frames` harness's schedule; the
    // game-time schedule clicks every `--click-ms` of game time.
    let mut clicks = if click_frames > 0 {
        Clicks::every_frames(first_click, click_frames)
    } else {
        Clicks::new(first_click_ms, click_ms)
    };
    clicks.at(click_x, click_y);
    let run = RunConfig {
        archives,
        base_dir: dir,
        frames,
        interval: ticker_interval,
        clicks,
        cumulative,
        frame_delay_ms,
        slow_frame_ms: slow_frame_ms as f64,
    };
    if vulkan {
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
            game: None,
            shared: Rc::clone(&shared),
            clicks: run.clicks,
            opened: None,
            index: 0,
            last_end: None,
        };
        let outcome = platform::run(config, harness);
        if let Err(error) = outcome {
            eprintln!("krkr: the platform loop failed: {error}");
            std::process::exit(1);
        }
    } else {
        run_null(run, Rc::clone(&shared));
    }
    let shared = shared.borrow();
    if let Some(failure) = &shared.failure {
        eprintln!("krkr: {failure}");
        std::process::exit(1);
    }
    println!("the backend: {}", if vulkan { "vulkan" } else { "null" });
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
            "the click schedule: the first click {} ms after the game opened, then every \
             {} ms",
            first_click_ms, click_ms
        );
    }
    println!("the scene: {scene_name}, the click at ({click_x}, {click_y})");
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
    println!("the clicks: {}", shared.clicks);
    // The first gap spans the first frame, the way the null harness's first
    // gap does; the distribution is the frames after it.
    let startup = shared.gaps.first().copied().unwrap_or(0.0);
    println!("the startup gap: {startup:.1} ms");
    report("the frame gap", &shared.gaps[1..]);
    report("the engine's own work", &shared.steps[1..]);
    report("the platform's wait", &shared.polls[1..]);
    if vm_report {
        println!("{}", app::vm_report(&compat::compile_stats()));
    }
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
