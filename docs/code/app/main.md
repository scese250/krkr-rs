# `app` — the `krkr` CLI and the emulator entry point

Source: `crates/app/src/main.rs` (the binary), `crates/app/src/runner.rs`
(the entry point and the platform bridge)

Two files make up the launch path. `main.rs` is the **desktop launcher**: it
parses the command line, reads the configuration file, resolves the game
path and installs the process logger. `runner.rs` is the **emulator entry
point**: it takes the finished `Launch` and runs it, offscreen or in a
window.

The split is the point: the emulator core performs no configuration I/O and
knows nothing about the command line. Another platform's launcher resolves
its own configuration and calls the same `app::run`.

---

## 1. The command line

```
krkr [--headless] [--frames N] [--fps N] [--backend null|vulkan]
     [--preload-archives] [--debug-overlay] [--graphic-cache-limit LIMIT]
     [--dump-frame PATH] [--resolution WxH] [--fullscreen]
     [--slow-report-ms N] [--no-frame-report]
     [--vm-report] [--native-resolution] [--archive XP3 ...] [--base-dir DIR] [GAME]
```

| option | value | default | effect |
|---|---|---|---|
| `--headless` | — | off | run the offscreen frame loop; it needs no display, and no GPU unless `--backend vulkan` asks for one |
| `--frames N` | frames | 120 | how many frames the headless loop steps |
| `--fps N` | `1..=1000` | 60 | the frame-rate limit; paces the windowed loop and the headless step |
| `--backend` | `null`, `vulkan` (`vk`) | `null` | the render backend to create |
| `--preload-archives` | — | off | read every mounted archive into memory in the background |
| `--debug-overlay` | — | off | draw the debug panel in the top-left corner |
| `--graphic-cache-limit` | `none`, `unlimited`, `0`, bytes | the reference default (disabled) | the decoded-image cache policy |
| `--dump-frame PATH` | path | — | write the presented frame as a PNG; needs the null backend |
| `--resolution WxH` | `WIDTHxHEIGHT` in device pixels | the platform's window size | create the window at that resolution and keep it |
| `--fullscreen` | — | off | create the window on the primary monitor at the display's own resolution, and keep it |
| `--slow-report-ms` | `0..` | unbounded | the total time the run may spend producing slow-frame reports; `0` reports none |
| `--no-frame-report` | — | off | skip the frame-time distribution the run prints as its last line |
| `--vm-report` | — | off | measure the VM's script compilation and print its analysis at the end of the run |
| `--native-resolution` | — | off | read every texture with a bilinear filter instead of point-sampling it, so a magnified picture is smooth (experimental) |
| `--archive XP3` | path | — | mount an archive; repeatable, order-significant, later wins |
| `--base-dir DIR` | path | the first archive's parent | the mount root; every relative archive path resolves against it |
| `GAME` | path | — | a game directory (becomes the mount root) or an XP3 file (is mounted) |

`-h` / `--help` prints the usage table. The table is generated from the same
`OPTIONS` constant the parser uses, and a test walks it, so an option cannot
be added without help text.

**Parsing is total.** Every value is parsed and validated before anything
takes effect, so a malformed command line has no partial effect. A parse or
configuration failure prints the message plus the usage and exits with code
`2`; a failed run exits with code `1`.

`--backend vulkan` needs a real window with a display connection (Wayland /
X11 / XCB) for a **windowed** run: the windowed host passes both the window
and the display handle so a Vulkan surface can be created. A **headless** run
has no window, so it draws every frame into an offscreen target of the
surface size, which `Host::read_offscreen_frame` reads back.

`--backend null` needs no GPU: the scene is composited by the CPU software
rasterizer, and a windowed run shows that frame through the platform's
software presenter. `--dump-frame PATH` writes the presented frame as a PNG;
the dumped frame is the one the CPU rasterizer composited, so the option
needs the null backend and the launcher refuses it with the Vulkan one. A
headless Vulkan frame is read back through `Host::read_offscreen_frame`
instead, so a host can inspect the picture without a display.

**The resolution is chosen once, at startup.** `--resolution WxH` declares
the resolution in device pixels; `--fullscreen` asks for the display's own
resolution in a window that covers the primary monitor. The two options
cannot be combined — the resolution is chosen once — and `--fullscreen` needs
a window, so it cannot be combined with `--headless` either; both are usage
errors that print the usage and exit with code `2`.

Without either, the window is the platform's own: it is created at the
platform's default size in logical pixels, and it is resizable, so the
player can resize it and the framebuffer follows the window.

With one of them, the window is created at that resolution, in device pixels,
and is not resizable: neither the player nor the window manager changes it,
and a script's `Window.setInnerSize` is not applied to it. That is the whole
of what the option fixes — the *window* keeps the resolution it was created
at. It does not freeze the engine's view of the window's size. The
framebuffer the run draws into is the window's extent: the picture is drawn
into the swapchain, whose extent is the window's extent, so the compositor
owns it and a size it reports reaches the engine's surface in every mode.
The surface size and the presentation mapping are therefore one size, and
the click the script receives is mapped back through that same mapping, so
the picture and the click agree. The **logical screen** is a different space
and does not follow the resolution: the script sees the VM's own screen
(1920×1080 by default), and the L2 translation fits it into the framebuffer
with the usual letterbox rule
([`../../arch/l2.md`](../../arch/l2.md) §4).

A **headless** run has no window, so its resolution is the size of the
offscreen target it draws into: the declared size, or the host's default
(1920×1080). The fullscreen resolution has no display to take it from, so a
headless run that asks for it is reported as an error instead of being given
a substitute.

The run reports the resolution it was created at in its startup records: the
windowed run logs the chosen resolution, whether the window keeps it, and
the framebuffer's size; the headless run logs the offscreen target's size.
Both records name the resolution, so a log says which picture the run drew.

`--slow-report-ms N` sets the **report budget**: the total wall-clock time
the run may spend producing slow-frame reports. The budget measures the
time spent producing the reports themselves — the one-line summary and the
detailed report alike. The option is absent by default, which leaves the
reporting unbounded, and `0` reports none. When the budget is spent the run
stops reporting and logs one note that names the budget and the number of
reports it had already emitted.

`--vm-report` switches the **VM's own measurement** of the script
compilation on, and prints the analysis at the end of the run. The
measurement is off by default: it reads the clock around every token the
analyser produces and around every command the parser drives the generator
with, which costs tens of milliseconds over a corpus. The report names the
number of compiles, the size of the scripts they read, and the time the
compile and its phases took — the lexer, the parser, the generator and the
image build. Every number in it is measured. The measurement is also what
the slow-frame report's VM section needs for the script compilation of a
frame, so a run that asks for the analysis gets that breakdown too.

`--no-frame-report` skips the **frame-time distribution** the run prints as
its last line: the number of frames, the fastest and the slowest frame, the
percentiles, and the 1% low — the rate the slowest 1% of the frames run at.
The distribution is on by default, and the option leaves the run without it:
the run then measures no history at all. The shape of the line is in
[`../../perf.md`](../../perf.md) §4.3.

`--native-resolution` is an **experimental rendering choice**, and it is a
flag with no value: it turns the choice on and nothing else. With it on,
every texture is read with a bilinear filter instead of point-sampled, so a
magnified layer is smooth instead of blocky.

It is not a scaling mode and it does not reopen that door: the
Fit-with-letterbox presentation is untouched, the logical screen the script
sees does not move, and every value a script reads stays where it is. The
flag changes the picture the device draws, which is not a value a script
reads.

The flag is the **render policy**, and the render policy is not the window's
size: `--resolution WxH` and `--fullscreen` choose the window's shape, and
the flag changes neither the window's size nor the surface size nor the
mapping. It is off by default, and a run that does not ask for it draws the
way the software renderer draws.

The choice is a knowing divergence from the reference, and it is recorded as
one: the reference's software renderer point-samples every magnification
(`reference/krkr2/cpp/core/visual/RenderManager.cpp:2902`, `:3191-3226`), and
it has the bilinear mode — `stFastLinear`
(`reference/krkr2/cpp/core/visual/LayerBitmapIntf.h:63-65`) — as a
render-manager parameter it does not default to. The default path keeps the
point filter, so the reference stays the oracle for every run that does not
ask for the flag. The policy and its place in the architecture are in
[`../../arch/l2.md`](../../arch/l2.md) §4.

The flag is a launch argument only: the configuration file cannot set it, and
an unknown key is a typed error. The run names the choice in its startup
records, so a log says whether the picture was filtered.

---

## 2. The game path

`resolve_game` mirrors the reference's discovery from its first command-line
argument:

* a **positional directory** becomes the mount root;
* a **positional file** is mounted as an archive;
* `--base-dir` and a positional directory are mutually exclusive;
* the mount root is `--base-dir` (or the positional directory) when given,
  otherwise the parent of the first archive, otherwise the working
  directory;
* every relative archive path resolves against the mount root, so
  `--base-dir /game --archive data.xp3` mounts `/game/data.xp3`;
* no game input at all is not an error: it is the empty-scene run.

The existence of the base directory is not checked here — a missing one is
reported when the storage mounts.

Loading rules — the explicit list's authority, the directory scan, the
precedence — are in [`../../migration/09-game-loading.md`](../../../docs.old/migration/09-game-loading.md);
the reference behaviour behind the positional argument is in the
[evidence trail](../../../docs.old/migration/09-game-loading.md).

---

## 3. Configuration and logging

`load_desktop_config` reads the `key=value` file named by the
`KRKR_CONFIG` environment variable. An unset variable means no
configuration; a configured file that cannot be read is a **typed error**,
never an empty result.

The merge precedence is **launch argument > configuration file > built-in
default**, applied field by field, and it is implemented in
`resolve_runtime_config`. Both sources have already been validated, so the
merge cannot fail. The value types and the file grammar are in
[config.md](config.md).

`init_logger` installs the process logger — the one place in the tree where
a `log` backend is installed — with `RUST_LOG` selecting the filter and
`info` as the default. A second installation is a no-op, so an embedding
launcher may install its own logger first.

The **slow-frame reports** come in two forms. The summary is one `warn`
line per slow frame. The **detailed report** is logged at `debug` level, so
it appears only when `RUST_LOG` selects `debug`, and it is emitted beside
the summary rather than instead of it. It lists every measured cost of the
frame **ranked largest first**, each entry labelled with the section it
belongs to (`phases`, `vm`, `pool`, `host`, `gpu`, `wait`), and it adds the
residuals and the decode pool's job count. The ranking holds the parts that
do not overlap each other, and the totals they add up to are in the report's
own totals line, so the largest entry names a cause rather than a sum. Its first line names the frame,
the gap and the dominant cost; the ranked entries below it are the detail
behind that line. Both forms are produced within the report budget
(`--slow-report-ms`).

---

## 4. `runner` — the entry point

```rust
pub struct Launch {
    pub runtime: RuntimeConfig,          // the validated configuration
    pub backend: BackendKind,            // the render backend to create
    pub game: Option<GameInput>,         // the game to open, or None
    pub mode: RunMode,                   // offscreen or windowed
    pub resolution: Resolution,          // the resolution the run is created at
    pub policy: RenderPolicy,            // the rendering policy: how the device draws
}

pub enum RunMode {
    Headless { frames: u32 },
    Windowed,
}

pub enum Resolution {                    // `platform::Resolution`, re-exported
    Default,                             // the platform's own resizable window
    Size(u32, u32),                      // a declared resolution, in device pixels
    Fullscreen,                          // the display's own resolution
}

pub struct GameInput {
    pub base_dir: PathBuf,               // the app path
    pub archives: Vec<PathBuf>,          // in mount order, later wins
}

pub struct RunReport { pub last_frame: Option<StepReport> }

pub fn run(launch: Launch) -> Result<RunReport, RunError>
```

`run` dispatches on the mode and returns after the headless loop finishes or
after the windowed event loop exits.

### The headless loop

`run_headless` builds a headless `HostConfig` (the requested backend, the
requested overlay and cache policy, and the resolution the run is created at,
which is the size of the offscreen target), opens a `GameHost` when the launch
names a game, and steps `frames` frames.
The step waits for the frame's deadline, so the frames are as far apart as
the limit says, and the engine's clock — which advances by the time the loop
measured between the frames — is the wall clock: the script's tick count
measures the same time here as it does in a windowed run.
It prints the counters the backend reports — the null backend reports them, a
GPU backend reports none — and the scene summary — the window count, the
layer count, the bitmap count and, per window, how many layers the window's
root list can actually reach. That last number is the reachability the
reference's draw path uses, so a scene with many unreached layers renders as
an empty window and the summary says so.

### The frame-time distribution

Both loops print the run's **frame-time distribution** as its last line:
the headless loop after the scene summary, and the windowed loop when the
event loop exits. The line names the number of frames, the fastest and the
slowest frame, the percentiles, and the 1% low — the rate the slowest 1% of
the frames run at. Its shape is in [`../../perf.md`](../../perf.md) §4.3.

The runner measures every gap — the headless loop from its own clock, the
windowed loop from the platform's measurement — and keeps the recent ones in
a bounded ring, so a run of any length reports the distribution of its most
recent frames. A run that is launched with `frame_report` off measures no
history at all.

### The windowed loop

`run_windowed` builds a `PlatformConfig` (the window title `krkr-rs`, the
frame interval from the frame-rate limit, and the resolution the run is
created at) and hands a `DesktopApp` to `platform::run`. The platform loop
owns the window and the ticker; the handler owns the host.

`DesktopApp` is the bridge from L1 platform events to the L2 host:

| callback | what it does |
|---|---|
| `resumed` | takes the window and display handles — a missing handle is fatal, never replaced by a synthetic one — creates the `Host` at the window's **real physical size**, logs the resolution the run was created at, and opens the `GameHost` when a game is present |
| `event` | `Resized` queues the newest surface size and the VM's `onResize` (the reference posts `onResize`); the size is applied once at the next frame boundary, before the frame body draws, in every mode; `ScaleFactorChanged` needs no action, because winit follows it with a `Resized`; input and focus events are queued and delivered with the next frame; `CloseRequested` exits |
| `frame` | delivers the queued events, steps the host, applies a script-requested window resize, and logs the heartbeat |

An **OS window resize** is applied at the frame boundary, with the newest size
the event loop reported. A compositor can report several sizes for one layout
change (a fractional-scale tiling rounds the height back and forth); applying
them as they arrive rebuilds a swapchain for each, and a rebuild nothing is
presented from cannot show the frame. One size per frame is applied instead.

The size reaches the surface in every mode. The framebuffer is the window's
extent — the picture is drawn into the swapchain, and the swapchain's extent
is the window's extent — so the compositor owns it, and a run that kept a
resolution of its own and ignored the report would leave the engine's
surface size describing a framebuffer the picture is not drawn at. The click
the script receives is mapped back through the presentation mapping, which
is fitted to that surface size, so the picture and the click agree. The
event is handed to the VM as well, so the script learns that the window was
resized, the way the reference posts `onResize`.

A size the compositor reports before the host exists is kept, not dropped:
the host is created at the window's current size, and the reported size is
applied at the first frame.

The **script-requested window resize** is the other direction of the resize
story: a script's `Window.setInnerSize` makes the engine produce a request,
`Host::take_window_resize` takes it (so a size that does not change again
is applied once), and the handler asks the OS window to adopt it. A backend
that applies the size synchronously emits no `Resized`, so the surface is
resized from the size the backend reports back. When the compositor keeps the
final say on the size (a tiled window), the size it settles on arrives as the
following `Resized` event, which the next frame applies. A window that keeps
a resolution of its own adopts no client size: the handler takes the request
and reports it instead of applying it.

The **heartbeat** is one `debug` line per second of delivered frames, with
the frame index, the frame gap, whether the frame reached the surface and how
many of the frames changed the scene. The platform loop logs the ticker
deadline for the same second, so an alive loop that presents nothing is
distinguishable from one that stopped delivering frames.

The **scene-change count** is a measurement, not a decision. A frame that
moves neither the scene's structure revision nor its content revision drew
the same image as the frame before it, so the count says how many frames
*could* have been skipped. It is conservative: a frame that only refreshed
the debug overlay, or that wrote to a bitmap no layer shows, still counts as
an unchanged scene.

---

## 5. Related documents

| topic | document |
|---|---|
| the crate overview and the wiring | [README.md](README.md) |
| the game host, the mount and the startup sequence | [game.md](game.md) |
| the configuration types and the file grammar | [config.md](config.md) |
| the platform loop and the event contract | [`../arch/l1.md`](../../arch/l1.md) |
| game loading and startup discovery | [`../../migration/09-game-loading.md`](../../../docs.old/migration/09-game-loading.md) |
| the runtime configuration contract | [`../../runtime-config.md`](../../../docs.old/runtime-config.md) |
