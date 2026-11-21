//! The pending-save wiring: a write the pool took, and the paths that wait
//! for it.
//!
//! The environment attaches an L2 [`Scene`], a [`Storage`] and — the point of
//! this file — an L2 [`DecodePool`], the same seam the app installs. The
//! tests then drive the real natives through the VM.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use api::media::{DecodedImage, ImageInfo};
use compat::CompatHost;
use engine::imagedecode::{DecodePool, ImageDecodeError, ImageDecoder};
use engine::jobpool::JobPoolConfig;
use engine::{Scene, Storage};
use fs::FileSystem;
use tjs::{Tjs2Engine, TjsValue};

/// The image the synthetic codec and the pool's decoder both produce.
fn image() -> DecodedImage {
    DecodedImage::new(
        2,
        2,
        vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255],
    )
}

/// The bytes of the picture the storage holds.
const PICTURE: &[u8] = b"not-a-real-image";

/// How long a write stays blocked before it goes on by itself.
///
/// The write is held so long that a reader which does **not** wait for it
/// would find nothing: the file is written when the hold expires.
const HOLD: Duration = Duration::from_millis(300);

/// The decoder the pool drives: every decode produces [`image`].
struct FixedDecoder;

impl ImageDecoder for FixedDecoder {
    fn probe(&self, _bytes: &[u8], _hint: Option<&str>) -> Result<ImageInfo, ImageDecodeError> {
        Ok(ImageInfo {
            width: image().width,
            height: image().height,
            has_alpha: true,
            format: Some("img".to_string()),
        })
    }

    fn decode(&self, _bytes: &[u8], _hint: Option<&str>) -> Result<DecodedImage, ImageDecodeError> {
        Ok(image())
    }
}

/// What the synthetic saver does with a write.
#[derive(Clone, Copy)]
enum Save {
    /// The write is held for [`HOLD`] and then writes the file.
    Held,
    /// The write fails.
    Failing,
}

/// A bound VM + host + the attached scene, storage and decode pool.
struct Env {
    _host: CompatHost,
    engine: Box<Tjs2Engine>,
    /// Kept alive for the host raw pointer; never read directly.
    _scene: Box<Scene>,
    /// Kept alive for the host's raw pointer; never read directly.
    _storage: Box<Storage>,
    /// Kept alive for the host's raw pointer; never read directly.
    _pool: Box<DecodePool>,
    /// The game directory the storage is mounted from.
    _game_dir: PathBuf,
}

impl Env {
    /// An environment whose saver behaves as `save` says.
    fn new(save: Save) -> Env {
        let game_dir = common::unique_dir("krkr-pending-save-test");
        std::fs::create_dir_all(&game_dir).expect("create temp game dir");
        std::fs::write(game_dir.join("picture.png"), PICTURE).expect("write the picture");
        let mut storage = Box::new(
            Storage::mount_archives(
                FileSystem::host(&game_dir),
                &game_dir,
                std::iter::empty::<&str>(),
            )
            .expect("mount storage"),
        );

        let engine = Box::new(Tjs2Engine::new().expect("create engine"));
        let mut scene = Box::new(Scene::default());
        let mut host = CompatHost::new();
        let directory = game_dir.clone();
        host.set_image_codec(
            |_bytes, _hint| Ok(image()),
            |_bytes, _hint| {
                Ok(ImageInfo {
                    width: image().width,
                    height: image().height,
                    has_alpha: true,
                    format: Some("img".to_string()),
                })
            },
            move |name, _type, _width, _height, rgba| match save {
                Save::Held => {
                    // The write is held: a reader that does not wait for it
                    // finds nothing.
                    std::thread::sleep(HOLD);
                    let path = directory.join(name);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                    }
                    std::fs::write(&path, rgba).map_err(|error| error.to_string())
                }
                Save::Failing => Err("the disk is full".to_string()),
            },
        );
        // The format table of the synthetic codec: only `bmp` is writable.
        host.set_image_save_type_check(|type_name| {
            if type_name.eq_ignore_ascii_case("bmp") {
                Ok(())
            } else {
                Err(format!("no image encoder for format {type_name:?}"))
            }
        });
        let mut pool = Box::new(
            DecodePool::new(Arc::new(FixedDecoder), JobPoolConfig::default())
                .expect("start the decode pool"),
        );
        // SAFETY: the boxed scene, storage and pool outlive the host (they
        // are declared after `_host`).
        unsafe {
            host.services_mut()
                .attach_scene(scene.as_mut() as *mut Scene);
            host.services_mut()
                .attach_storage(storage.as_mut() as *mut Storage);
            host.services_mut()
                .attach_decode_pool(pool.as_mut() as *mut DecodePool);
        }
        host.install(&engine).expect("install compat surface");
        common::link_all_builtins(&engine);
        Env {
            _host: host,
            engine,
            _scene: scene,
            _storage: storage,
            _pool: pool,
            _game_dir: game_dir,
        }
    }

    /// Runs a script, panicking when it raises.
    fn run(&self, script: &str) {
        self.engine
            .exec_script(script, "pending-save-test")
            .unwrap_or_else(|error| panic!("exec {script:?} failed: {error}"));
    }

    /// Evaluates an expression, panicking when it raises.
    fn eval_ok(&self, expression: &str) -> TjsValue {
        self.engine
            .eval(expression, "pending-save-test")
            .unwrap_or_else(|error| panic!("eval {expression:?} failed: {error}"))
    }

    /// The message a statement raised.
    ///
    /// The script is compiled as a program, not as an expression: a
    /// statement is not an expression, and the engine compiles an
    /// evaluation in expression mode.
    fn error(&self, script: &str) -> String {
        match self.engine.exec_script(script, "pending-save-test") {
            Ok(value) => panic!("{script:?} should have raised, got {value:?}"),
            Err(error) => error.to_string(),
        }
    }
}

/// Reference: `tTJSNI_BaseLayer::SaveLayerImage` encodes the main image and
/// writes it (`reference/cpp/core/visual/LayerIntf.cpp:2699-2807`). The
/// write is deferred here, so the save returns before the file is written.
#[test]
fn the_save_returns_before_the_write_finished() {
    let _lock = common::vm_lock();
    let env = Env::new(Save::Held);

    let started = Instant::now();
    env.run("var b = new Bitmap(2, 2); b.save('out/shot.bmp', 'bmp');");
    let elapsed = started.elapsed();

    assert!(
        elapsed < HOLD,
        "the save must not wait for the write: it took {elapsed:?}, the write holds for {HOLD:?}"
    );
}

/// Reference: the reference writes the file before `saveLayerImage` returns,
/// so a load of the name right after the save sees the written file
/// (`reference/cpp/core/visual/LayerIntf.cpp:2699-2740`). The deferred write
/// keeps that: the reader waits for the write.
#[test]
fn a_reader_of_the_name_waits_for_the_write() {
    let _lock = common::vm_lock();
    let env = Env::new(Save::Held);

    env.run("var b = new Bitmap(2, 2); b.save('out/shot.bmp', 'bmp');");
    // The load waits for the write: the file is there when it returns.
    env.run("var c = new Bitmap('out/shot.bmp');");
    assert_eq!(env.eval_ok("c.width"), TjsValue::Integer(2));
    assert_eq!(env.eval_ok("c.height"), TjsValue::Integer(2));
}

/// Reference: the reference raises the write failure from `saveLayerImage`
/// (`reference/cpp/core/visual/LayerIntf.cpp:2699-2740`). The deferred write
/// raises it at the next read of the name, so the failure is not swallowed.
#[test]
fn a_failed_write_is_raised_at_the_next_read() {
    let _lock = common::vm_lock();
    let env = Env::new(Save::Failing);

    env.run("var b = new Bitmap(2, 2); b.save('out/shot.bmp', 'bmp');");
    let message = env.error("var c = new Bitmap('out/shot.bmp');");
    assert!(
        message.contains("the disk is full"),
        "the write failure must be raised at the next read, got: {message}"
    );
}

/// Reference: `TVPSaveImage` finds the handler that accepts the mode and
/// raises `TVPUnknownGraphicFormat` when there is none — before it writes
/// (`reference/cpp/core/visual/GraphicsLoaderIntf.cpp:348-367`). The type
/// check does not need the pixels, so the error is raised at the save.
#[test]
fn an_unknown_type_is_raised_at_the_save() {
    let _lock = common::vm_lock();
    let env = Env::new(Save::Held);

    let message = env.error("var b = new Bitmap(2, 2); b.save('out/shot.xyz', 'xyz');");
    assert!(
        message.contains("no image encoder for format"),
        "the unknown format must be raised at the save, got: {message}"
    );
}
