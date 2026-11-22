//! Integration tests for the `VideoOverlay` event dispatch.
//!
//! The overlay's events reach the script object that owns the overlay. The
//! delivery asks that object for the member first: an owner that does not
//! define it receives no event, and the delivery reports nothing.
//!
//! The reference delivers an event with a call whose result it never
//! inspects (`reference/cpp/core/base/EventIntf.cpp:97-111` — the call at
//! `:104-105`, the discarded result), and a member that is not there makes
//! that call return `TJS_E_MEMBERNOTFOUND` rather than raise
//! (`reference/cpp/core/tjs2/tjsObject.cpp:1214-1226`). So an owner without
//! the member is not an error there, and the port must not turn it into one.
//!
//! The whole family of events takes this path: `onStatusChanged`
//! (`reference/cpp/core/visual/VideoOvlIntf.cpp:91-111`),
//! `onPeriod` (`:151-165`) and `onFrameUpdate` (`:167-181`) are posted to the
//! owner the same way, and each of them is delivered through the same call.

mod common;

use std::cell::RefCell;

use api::media::{RgbaFrame, VideoSpec};
use common::vm_lock;
use compat::CompatHost;
use engine::FrameClock;
use engine::Scene;
use tjs::{Tjs2Engine, TjsValue};

thread_local! {
    static CAPTURED_LOGS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// A `log` sink that records every record on the calling thread.
struct CaptureLogger;

impl log::Log for CaptureLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        CAPTURED_LOGS.with(|logs| {
            logs.borrow_mut()
                .push(format!("{} {}", record.level(), record.args()));
        });
    }

    fn flush(&self) {}
}

/// Install the capturing sink once per test binary, clear this thread's
/// buffer, run `f`, and return its result plus the thread's log lines.
fn capture_logs<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = log::set_boxed_logger(Box::new(CaptureLogger));
        log::set_max_level(log::LevelFilter::Debug);
    });
    CAPTURED_LOGS.with(|logs| logs.borrow_mut().clear());
    let result = f();
    let logs = CAPTURED_LOGS.with(|logs| logs.borrow().clone());
    (result, logs)
}

/// A bound VM + host + attached scene + the engine's frame clock.
///
/// The host is declared first so it drops before the engine (releasing every
/// retained script object while the engine is alive) and the scene.
struct Env {
    host: CompatHost,
    engine: Box<Tjs2Engine>,
    _scene: Box<Scene>,
    /// The engine's frame clock: the picture follows it.
    clock: RefCell<Box<FrameClock>>,
}

impl Env {
    fn new() -> Env {
        let engine = Box::new(Tjs2Engine::new().expect("create engine"));
        let mut scene = Box::new(Scene::default());
        let mut clock = Box::new(FrameClock::new());
        let mut host = CompatHost::new();
        // SAFETY: the boxed scene and clock outlive the host (declared after
        // it).
        unsafe {
            host.services_mut()
                .attach_scene(scene.as_mut() as *mut Scene);
            host.services_mut()
                .attach_clock(clock.as_mut() as *mut FrameClock);
        }
        host.install(&engine).expect("install compat surface");
        common::link_all_builtins(&engine);
        Env {
            host,
            engine,
            _scene: scene,
            clock: RefCell::new(clock),
        }
    }

    /// Move the engine's clock forward by `dt_ms` and advance the overlay.
    fn advance_video(&self, dt_ms: f64) -> Result<(), compat::CompatError> {
        self.clock.borrow_mut().advance(dt_ms);
        let ctx = self.host.ctx().expect("bound host");
        compat::video_advance(&ctx, f64::MAX)
    }

    /// Install the host-decoded source: a 4x4 @ 10 fps stream with one
    /// decoded picture.
    fn set_default_source(&self) {
        self.host
            .set_video_source(movie_spec(), vec![solid_frame()]);
    }

    fn run(&self, script: &str) {
        self.engine
            .exec_script(script, "video-event-dispatch")
            .unwrap_or_else(|error| panic!("exec {script:?} failed: {error}"));
    }

    fn eval_ok(&self, expression: &str) -> TjsValue {
        self.engine
            .eval(expression, "video-event-dispatch")
            .unwrap_or_else(|error| panic!("eval {expression:?} failed: {error}"))
    }

    fn eval_int(&self, expression: &str) -> i64 {
        match self.eval_ok(expression) {
            TjsValue::Integer(value) => value,
            other => panic!("{expression:?} returned {other:?}, expected an integer"),
        }
    }
}

/// A 4x4 frame, at timestamp 0.
fn solid_frame() -> RgbaFrame {
    RgbaFrame::new(4, 4, 0, vec![0; 4 * 4 * 4])
}

/// A 4x4 @ 10 fps, 10-frame video-only stream.
fn movie_spec() -> VideoSpec {
    VideoSpec {
        width: 4,
        height: 4,
        fps: 10.0,
        total_frames: Some(10),
        sample_rate: 0,
        channels: 0,
        audio_streams: 0,
        video_streams: 1,
        audio_stream: -1,
        video_stream: 0,
    }
}

/// An owner that does not define the event member receives no event, and the
/// frame does not report one.
///
/// The reference's delivery calls the member and never looks at the result
/// (`reference/cpp/core/base/EventIntf.cpp:97-111`), and a member that is not
/// there makes the call return `TJS_E_MEMBERNOTFOUND`
/// (`reference/cpp/core/tjs2/tjsObject.cpp:1214-1226`). The port turned that
/// absence into a raised error it caught and logged on every frame, which is
/// what a device run of a game whose owner does not define `onFrameUpdate`
/// showed: one warning per frame.
#[test]
fn an_owner_without_the_event_member_is_not_reported() {
    let _lock = vm_lock();
    let env = Env::new();
    env.set_default_source();
    env.run(
        "var v = new VideoOverlay(new Window()); \
         v.open('movie.mpg'); v.play(); \
         delete v.onFrameUpdate;",
    );
    assert_eq!(
        env.eval_ok("typeof v.onFrameUpdate"),
        TjsValue::String("undefined".into()),
        "the owner must not define the member"
    );

    let (result, logs) = capture_logs(|| -> Result<(), compat::CompatError> {
        for _ in 0..3 {
            env.advance_video(100.0)?;
        }
        Ok(())
    });
    result.expect("the frame must not fail on an owner without the member");
    assert!(
        !logs.iter().any(|line| line.contains("onFrameUpdate")),
        "an owner without the member must not be reported: {logs:?}"
    );
    assert!(
        !logs.iter().any(|line| line.contains("compat.video")),
        "the video dispatch must not report an absent member: {logs:?}"
    );
    assert!(
        env.eval_int("v.position") > 0,
        "the picture must keep running while the owner has no member"
    );
}

/// An owner that defines the event member still receives the event, with its
/// argument.
///
/// The reference posts the frame index to the owner
/// (`reference/cpp/core/visual/VideoOvlIntf.cpp:167-181`) and the delivery
/// calls the member with it (`reference/cpp/core/base/EventIntf.cpp:100-105`).
#[test]
fn the_owner_receives_the_event_it_defines() {
    let _lock = vm_lock();
    let env = Env::new();
    env.set_default_source();
    env.run(
        "var updates = 0; var last = -1; \
         var v = new VideoOverlay(new Window()); \
         v.open('movie.mpg'); v.play(); \
         v.onFrameUpdate = function(frame) { updates++; last = frame; };",
    );

    let (result, logs) = capture_logs(|| -> Result<(), compat::CompatError> {
        for expected in 1..=3 {
            env.advance_video(100.0)?;
            assert_eq!(
                env.eval_int("updates"),
                expected,
                "pump {expected} must fire one frame update"
            );
        }
        Ok(())
    });
    result.expect("the frame advance");
    assert!(
        !logs.iter().any(|line| line.contains("compat.video")),
        "a handler that runs must not be reported: {logs:?}"
    );
    assert!(
        env.eval_int("last") >= 0,
        "the handler must receive the frame index"
    );
}

/// An owner whose event member holds nothing receives no event, and the
/// frame does not report one.
///
/// The reference's delivery calls the member whatever it holds
/// (`reference/cpp/core/base/EventIntf.cpp:104-105`); a member that is not an
/// object makes that call fail with `TJS_E_INVALIDTYPE`
/// (`reference/cpp/core/tjs2/tjsObject.cpp:1228-1229`), and the delivery
/// never inspects that result either.
#[test]
fn an_owner_whose_event_member_holds_nothing_is_not_reported() {
    let _lock = vm_lock();
    let env = Env::new();
    env.set_default_source();
    env.run(
        "var v = new VideoOverlay(new Window()); \
         v.open('movie.mpg'); v.play(); \
         v.onFrameUpdate = null;",
    );

    let (result, logs) = capture_logs(|| -> Result<(), compat::CompatError> {
        for _ in 0..3 {
            env.advance_video(100.0)?;
        }
        Ok(())
    });
    result.expect("the frame must not fail on an owner without a handler");
    assert!(
        !logs.iter().any(|line| line.contains("compat.video")),
        "an owner whose member holds nothing must not be reported: {logs:?}"
    );
}
