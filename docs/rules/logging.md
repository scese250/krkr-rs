# Logging — one format for the whole project

Status: **authoritative** for the log output. This file states the shape of
every log record the engine writes. Where it disagrees with
[`AGENTS.md`](../../AGENTS.md), `AGENTS.md` wins.

The engine's log was, before this file existed, a mixture: 106 distinct
leading tags over 374 sites, 81 of them with no tag at all, 26 of them
printing a whole message held in a variable. A reader could not tell which
subsystem a line came from, and the slow frame was reported twice - once as
one flat line and once as a ranked breakdown - in two different shapes.

Related documents:

| topic | document |
|---|---|
| the reviewer checklist | [`code_styles.md`](code_styles.md) |
| the gate that enforces this file | [`gates.md`](gates.md) |
| the layers the subsystems follow | [`arch_rules.md`](arch_rules.md) |

---

## 1. The record

Every record is one line:

```
<subsystem>: <message>
```

**The subsystem** is a lowercase dotted path that says where the record came
from. It is the crate, and the module inside it:

| subsystem | what it covers |
|---|---|
| `app.audio` | the audio device, its underruns and its playback cap |
| `app.frame` | the frame body, its phases and the slow-frame report |
| `app.runner` | the run: the mount, the window and the report it prints |
| `app.vm` | the VM's own analysis of the script compilation |
| `app.startup` | the open sequence and its phases |
| `android.host` | the Android composition root |
| `android.input` | the Android input queue and the touch mapping |
| `compat.audio` | the sound natives and the mixer |
| `compat.debug` | the Debug native, its log file and its logging handlers |
| `compat.event` | the event adapter |
| `compat.font` | the font natives |
| `compat.image` | the image natives and the pixel operations |
| `compat.kag` | the KAG parser and the scenario |
| `compat.layer` | the Layer natives, their focus and their transitions |
| `compat.menu` | the menu natives |
| `compat.plugin` | the plugin link and the plugin classes |
| `compat.psb` | the PSB plugin |
| `compat.scripts` | the script natives and the storage scripts |
| `compat.sound` | the sound buffer natives |
| `compat.storage` | the storage natives |
| `compat.system` | the system natives |
| `compat.timer` | the timer natives |
| `compat.video` | the video natives and the movie playback |
| `engine.audio` | the engine mixer |
| `engine.frame` | the frame clock and the engine frame |
| `engine.jobpool` | the decode pool |
| `engine.projection` | the projection of the scene onto the screen |
| `engine.scene` | the layer tree, the layers and their properties |
| `engine.storage` | the archives, the entries and the reads |
| `engine.text` | the fonts and the text measurement |
| `engine.timer` | the engine timers |
| `platform.fs` | the file system |
| `platform.linux` | the desktop window, the audio device and the clock |
| `platform.media` | the decoders |
| `platform.render` | the renderer and the backends |
| `vm.tjs` | the script VM host |

A subsystem that is not in this list is added to it, with the module it
covers. The list is the whole vocabulary.

**The message** is prose. A value a tool would want to read is written as a
`key=value` pair, and the pair's name says what it is:

```
engine.storage: opened bgimage.xp3 entries=108
app.frame: slow frame=7 presented=false gap_ms=520.0 dominant=phases.frame_tail dominant_ms=480.0
```

**A record that spans lines** is one record: the first line carries the
header, and every following line is indented two spaces and starts with a
keyword that names the kind of line:

```
app.frame: slow frame=7 presented=false gap_ms=520.0 dominant=phases.frame_tail dominant_ms=480.0
  cost phases.frame_tail=480.0
  cost pool.elapsed=12.0
  residual tick_residual_ms=4.0 host_residual_ms=0.3 unaccounted_ms=3.5 pool_jobs=2
  total body_ms=480.0 tick_ms=487.5 host_sync_ms=0.5 host_total_ms=11.0 total_ms=501.5 platform_wait_ms=15.0
```

The keywords are `cost`, `residual`, `total` and `front_end`. A record that
spans lines is one log entry, not several.

---

## 2. The levels

| level | what it means |
|---|---|
| `error` | the engine cannot continue: the game is over |
| `warn` | the engine continues, and something the reader should know about happened |
| `info` | a lifecycle event: a scenario loaded, a plugin linked, a device opened |
| `debug` | a decision the engine made, or a measurement of it |
| `trace` | per-item detail: every layer, every entry, every call |

A failure is a `warn` when the engine recovered from it and an `error` when
it did not. A measurement is `debug`, unless it is the slow-frame report:
that one is a `warn`, because it is the record of a frame the reader will
want to find again.

---

## 3. What a record never carries

* **A path into the game's content.** A storage name is fine; the path of the
  file a storage resolved to is not, because the game's directory holds the
  reader's material.
* **A transcript.** The scenario's text is the game's content.
* **A URI.** A `content://` URI carries a path and a grant.

---

## 4. The slow frame

The slow frame is reported **once**, as one record, in the shape of §1: the
header names the frame, the gap and the dominant cost; the costs follow
ranked largest first and labelled with the section they belong to; the
residuals and the totals close it.

The threshold is [`DEFAULT_SLOW_FRAME_MS`](../../crates/app/src/game.rs) (250 ms)
by default, and a caller that measures a frame budget sets its own: the
budget of a 30 fps target is 33.3 ms, and the default threshold hides
exactly the frames such a budget is about.

The costs are the parts that do not overlap, so they add up to the frame's
own time: the five polls, the input segments, the tick's residual, the
decode pool's work, the engine frame's parts, the GPU calls, the input
dispatch, and the gap the frame and the platform's wait leave unaccounted.
The totals name the sums those parts add up to.

---

## 5. The check

`krkr-gate log-format` reads the workspace's `log::` call sites and fails
when a record does not start with a subsystem from §1. The sites that
predate this file are in the check's baseline and are a burn-down: a site
that is added must comply, and a site that is touched must be brought into
compliance with it.