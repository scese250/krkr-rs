# `layerExLongExposure` — writer's report

**Patch:** `/tmp/pi-patches/write-layerExLongExposure/patch.diff`
(4 new files; `git apply --check --reverse` against the working tree passes,
so the patch is exactly what is in the tree).

---

## 1. What I wrote

| file | what is in it |
|---|---|
| `crates/compat/src/natives/plugins/layer_ex_long_exposure/mod.rs` | the module: the documentation, `register`, the shared helpers (`bound`, `scene_mut`, `scene_ref`, `receiver_id`, `image_size`), the per-layer state (`ExposureState`, `exposure_state`, `release_gone`), the accumulation (`Accumulation`), the normalisation (`Ranges`), and two unit tests for the arithmetic |
| `crates/compat/src/natives/plugins/layer_ex_long_exposure/accumulate.rs` | the members that build the accumulation: `initExposure`, `snapExposure`, `termExposure` |
| `crates/compat/src/natives/plugins/layer_ex_long_exposure/report.rs` | the members that report the accumulation: `statExposure`, `copyExposure`, plus the argument reading (the eight members, the integer conversion, the conversion error) |
| `crates/compat/tests/layer_ex_long_exposure.rs` | 17 behaviour tests driving the real script surface |

Nothing else was touched. `crates/compat/src/natives/plugins/mod.rs` and
`crates/compat/src/natives/visual/layer/` are untouched, as instructed.

### The shape

The plugin publishes **no class of its own**: the five members are
registered as instance methods on the engine's existing `Layer` class, the
same shape the sibling `layerEx` family uses
(`crates/compat/src/natives/plugins/layer_ex/mod.rs:574`).

The per-layer accumulation is a `Vec<u32>` of four counters per pixel of the
layer's main image (red, green, blue, alpha — the engine's own byte order,
`crates/engine/src/scene.rs:386-394`), plus the image size it was created
for. The normalisation is one pure function (`Ranges::channel`), so the
arithmetic is unit-testable without a VM.

---

## 2. The open items, answered

### 2.1 The 32-bit wrap — **reproduced, not widened**

The counters are `u32` and the scaling product is a `u32`:
`Accumulation::add` uses `wrapping_add`, `Ranges::channel` uses
`wrapping_mul` and truncates the quotient to a byte.

**Why:** the upstream's arithmetic is 32-bit unsigned, so reproducing it
keeps the port bit-exact with the upstream for every accumulation — there is
no divergence to record under D36. A 64-bit counter would agree below 2^32
and differ above it, which would be a knowingly-left divergence and would
need an approximation note. The counters wrap after ~16.8 million snaps of a
255 channel, exactly as upstream.

The wrap is observable in the *scaling* product long before the counters
wrap (the product overflows once `n - lo > 16 843 009`), and the unit test
`the_scaling_product_wraps` pins it: with the range `(0, 0xffffffff)` the
value `16_843_009` writes `1` and `16_843_010` writes `0`.

### 2.2 The zero image address — **a typed error**

The upstream reads through the raw address without checking it
(spec §8.3). In this engine the address is 0 when the image declares more
pixels than its buffer holds
(`crates/engine/src/scene.rs:2113-2141`, `main_image_pixel_buffer` and
`main_image_pixel_buffer_for_write` return 0 when the bitmap does not hold
its pixels).

The port never takes the address: it reads and writes through the safe
accessors (`crates/engine/src/scene.rs:2171-2192`,
`with_main_image_pixels` / `with_main_image_pixels_mut`) and checks the
buffer length itself. A buffer that does not hold the pixels its size names
raises the plugin's own message for an unusable image
(`LongExposure.snap: invalid layer image` / `LongExposure.copy: invalid
layer image`) and logs the reason (`compat.layer: Layer.snapExposure: …`).
The script-visible catalogue is therefore exactly the upstream's.

### 2.3 `VAL_NULL` — **routed to the defaults branch explicitly**

The engine's VM gives `null` a type of its own
(`crates/tjs/src/lib.rs:60`, `VAL_NULL`), and the ABI carries it as such
(`crates/tjs/cpp/tjs2_abi.cpp:526-533`: a null argument arrives as
`TJS2_VAL_NULL`, not as an object).

The argument match in `report.rs` is therefore
`VAL_VOID => …`, `VAL_OBJECT => …`, `_ => Ranges::default()` — a null (and
every other non-object) keeps the built-in pair `lo = 4294967295`, `hi = 0`,
so every output byte is 255.

### 2.4 Where the per-object state lives — **the compat services' per-family state, keyed by layer id**

`crates/compat/src/ctx.rs:245` — `CompatServices::state::<T>()` is "the
extension point that keeps native-family state out of process globals". The
sibling family uses it the same way
(`crates/compat/src/natives/plugins/layer_ex/draw.rs:182-196`, `DrawState`
keyed by layer id).

The key is the layer's scene id. Ids are never reused
(`crates/engine/src/scene.rs:1400-1425`, `add_detached_layer` counts them
up), so a stale entry can never be mistaken for a new layer's accumulation.

**The release.** The upstream frees the buffer when the `Layer` *object* is
destroyed. In this tree that hook is
`crates/compat/src/natives/visual/layer/mod.rs:405,447,475` →
`layer_ex::forget_layer`, which is outside my scope. The port therefore
drops the accumulation of a layer whose scene record is gone on the next
member call (`release_gone`). The receiver's own entry is kept, so a call on
a layer the engine has torn down still answers with the image error rather
than "not initialized".

### 2.5 The parameter-count error — **"Invalid argument count"**

The upstream's check lives in the calling convention
(`reference/krkr2/cpp/core/plugin/ncbind.hpp:1176-1178`,
`if (_numparams < SelectorT::ArgsCount) return TJS_E_BADPARAMCOUNT;`) and
the count is the function's parameter count
(`reference/krkr2/cpp/core/plugin/ncb_invoke.hpp:157`). The engine renders
that error as "Invalid argument count"
(`reference/krkr2/cpp/core/tjs2/tjsError.h:268`), so that is the text the
upstream's script sees, and the text the port raises.

The tree has two conventions: the older native members name the member and
the count (`crates/compat/src/natives/plugins/layer_ex/geom.rs:108` — "PointF
requires 2 arguments"), while the sibling plugin family of this batch uses
the engine's own text
(`crates/compat/src/natives/plugins/layer_ex_btoa/province.rs:403` — "Invalid
argument count"). The port follows the sibling and the upstream: the
message is the engine's, because the failure is the engine's, not the
plugin's.

### 2.6 The DLL name — **`layerExLongExposure.dll`**

The upstream's build target is `layerExLongExposure`; the engine's built-in
table spells DLL names in the `*.dll` form
(`crates/compat/src/natives/plugins/mod.rs`, `BUILTIN_PLUGINS` — e.g.
`layerExDraw.dll`). The upstream's debug-suffixed name
(`layerExLongExposure-d.dll`) is a build artefact of its CMake helper and is
not part of the surface.

---

## 3. The wiring the integrator has to add

In `crates/compat/src/natives/plugins/mod.rs`:

```rust
pub mod layer_ex_long_exposure;
```

in the module list (alphabetically, after `pub mod layer_ex;`), and in
`BUILTIN_PLUGINS`:

```rust
    BuiltinPluginDef {
        name: "layerExLongExposure.dll",
        aliases: &[],
        dependencies: &[],
        register: layer_ex_long_exposure::register,
    },
```

No aliases (the upstream publishes one DLL name), no dependencies (the
plugin needs no other module).

The doc table `docs/code/compat/plugin.md` (the list of ported plugins, the
row after `layerExDraw.dll`) should gain a row for it — that file is
outside my scope.

**The surface lock is unaffected**: the runtime surface dump is taken from
`CompatHost::install`, and the plugin's members are published by the first
`Plugins.link`, not by the install (`crates/compat/tests/runtime_surface.rs`).

---

## 4. The script-visible members

Five instance methods on `Layer`, and nothing else — no class, no global,
no constant:

| member | arity | result |
|---|---|---|
| `initExposure` | 0 | void |
| `snapExposure` | 0 | void |
| `statExposure` | 0 | a dictionary with `min_r`, `min_g`, `min_b`, `min_a`, `max_r`, `max_g`, `max_b`, `max_a` |
| `copyExposure` | 1 (required) | void |
| `termExposure` | 0 | void |

Like every native-class member in this VM, they exist on the instances
created **after** the plugin is linked (the native class copies its members
onto every instance it creates —
`crates/tjs/cpp/tjs2/tjsNative.cpp:408-412`, `CopyClassInstanceMembers`).
A layer created before the link does not carry them, and neither does the
upstream.

---

## 5. The tests and what each pins

`crates/compat/tests/layer_ex_long_exposure.rs` — every test links the
plugin, creates a layer with a main image, and drives the members from a
script. Each assertion cites the upstream line that fixes it.

| test | what it pins |
|---|---|
| `the_members_appear_only_when_the_plugin_is_linked` | the members are absent before the link and present after it (Main.cpp:229-236) |
| `init_exposure_refuses_a_layer_without_an_image` | "LongExposure.init: invalid layer image" (Main.cpp:17-18) |
| `init_exposure_refuses_a_layer_without_an_accumulation` | the three "not initialized" messages (Main.cpp:25-26, 88-89, 112-113) |
| `the_statistics_are_the_accumulated_sums` | the statistics are the accumulated sums, per channel (Main.cpp:37-46, 95-107) |
| `a_freshly_initialised_layer_reports_zero` | the counters start at zero (Main.cpp:19-21) |
| `init_exposure_twice_restarts_the_accumulation` | the second initialisation discards the first buffer (Main.cpp:16) |
| `the_copy_stretches_the_accumulation_over_the_channel_range` | `copyExposure(void)` uses the accumulation's own extremes and stretches every channel over its own range (Main.cpp:127-130, 147-156) |
| `the_copy_uses_the_ranges_the_argument_carries` | the ranges the argument carries are the ones used; a pair with no spread writes 255 (Main.cpp:54, 79-82) |
| `a_missing_member_reads_as_zero` | a member the object does not carry reads as 0 — the red range becomes (100, 0), which has no spread, so the image is white (Main.cpp:131-145) |
| `an_argument_that_is_not_an_object_writes_255` | an integer, a real, a string and a null carry no pair, so every byte is 255 (Main.cpp:126-146) |
| `the_copy_needs_one_argument` | a call with no argument raises the engine's parameter-count error (Main.cpp:234) |
| `a_member_that_is_not_an_integer_is_refused` | a member that holds an object raises the engine's variant-conversion error (Main.cpp:131-145) |
| `a_snap_after_a_resize_is_refused` | "LongExposure.snap: invalid layer size", and the accumulation is left untouched (Main.cpp:30-31) |
| `a_copy_after_a_resize_is_refused` | "LongExposure.copy: invalid layer size" (Main.cpp:117-118) |
| `term_exposure_releases_the_accumulation` | the release, the release of nothing, and the "not initialized" that follows (Main.cpp:159-163) |
| `the_accumulation_belongs_to_one_layer` | the accumulation is per layer, not per class (Main.cpp:219-228) |
| `the_province_plane_is_never_touched` | the province plane keeps its byte through a copy (Main.cpp:147-156) |
| `the_copy_writes_the_main_image_only` | the copy marks the image modified and leaves the image size alone (Main.cpp:120-123) |

Two unit tests in `mod.rs` pin the arithmetic that no script can reach
cheaply: `the_scaling_product_wraps` (the 32-bit product) and
`an_empty_range_writes_255` (a pair with no spread).

---

## 6. The divergences, and why

1. **A null image address raises instead of dereferencing.** The upstream
   reads through the address it obtained without checking it (spec §8.3).
   The port raises "…: invalid layer image" and logs the reason. The
   script-visible catalogue stays exactly the upstream's.

2. **A member whose value is `null` reads as 0.** The upstream's integer
   conversion raises the variant-conversion error for an object variant, and
   `null` is an object variant there. The safe member read
   (`crates/tjs/src/lib.rs:2557-2607`, `member_read`) cannot tell a null
   from an absent member — both arrive as `MemberRead::Void` — so the port
   reads it as 0. Every other shape (an object, an octet) raises, as the
   upstream does.

3. **The accumulation of a layer that is gone is released on the next
   member call**, not exactly when the `Layer` object is destroyed. The
   destroy hook is outside my scope (§2.4). The observable difference is
   limited to a layer that was invalidated and then used from *another*
   layer's member call: the upstream would still hold the buffer, the port
   has dropped it and answers "not initialized".

4. **A buffer that cannot be allocated raises**
   ("LongExposure.init: the accumulation buffer could not be allocated").
   The upstream's allocation failure is a crash.

5. **The parameter-count message is the engine's** ("Invalid argument
   count"), not the tree's "… requires N arguments" (§2.5). This matches
   the upstream's script-visible text.

6. **The counter order inside the port's buffer is red, green, blue,
   alpha** (the engine's byte order), where the upstream's is blue, green,
   red, alpha. Only the labelling is observable, through `statExposure`'s
   keys, and the labels are the same (spec §8.1).

7. **The write path.** The upstream asks for the writable image address,
   which marks the image modified, and writes through it. The port writes
   through `with_main_image_pixels_mut` (which marks the bitmap dirty) and
   sets the layer's `image_modified` flag explicitly
   (`crates/engine/src/scene.rs:2013`, `mark_layer_image_modified`), so
   `Layer.imageModified` reads the same.

Nothing else was left out: every behaviour the specification describes is
implemented, including the quirks (`lo >= hi` yields 255, so a never-snapped
layer, a missing-member dictionary, a non-object argument and `null` all
produce an opaque white image; the statistics are the accumulated sums; the
initialisation releases the old state before it can fail).

---

## 7. Risks

* **The build is unverified.** The worker's scope forbids cargo, so the
  code has not been compiled. The borrow-checker-sensitive places are the
  ones where a `&mut Scene` and the `RefMut` of the plugin state coexist
  (`snap_exposure`, `copy_exposure`); they are different objects, and each
  `&mut Scene` is confined to its own block, but the integrator's build is
  the first real check.
* **The plugin is not wired yet**, so every test in the new file fails
  until the table entry lands (the link of an unknown DLL name only warns,
  so the failure is "Member … does not exist", not a panic in the link).
* **The memory footprint**: 16 bytes per pixel of the layer's image, per
  layer with an accumulation — the same as the upstream. A 1920×1080 layer
  costs ~33 MB.
* **`release_gone` sweeps on every member call.** It is O(layers with an
  accumulation), which is a handful; the sweep exists because the destroy
  hook is out of scope.
* The compiler-free gate checks pass on the new files (`house-style`,
  `layering`, `log-format`, `error-handling`, `stateless`,
  `callback-objthis`, `void-getter`, `native-instance-lookup`,
  `storage-boundary`, `lifecycle-order`); `native-arity` and
  `storage-media` need a reference checkout this host does not have.

---

## 8. What I left alone

* `crates/compat/src/natives/plugins/mod.rs` — the module declaration and
  the table entry (§3).
* `crates/compat/src/natives/visual/layer/` — the destroy hook that would
  release an accumulation exactly with its layer (§2.4).
* `crates/compat/tests/runtime_surface.rs` and the surface-lock baseline —
  unchanged, because the plugin's members are published by the link.
* `docs/code/compat/plugin.md` — the ported-plugin table (§3).
* The sibling plugins' files (the other workers' `layer_ex_btoa` etc.) —
  untouched.

---

## 9. Post-build audit (after the integrator's first build)

The build stopped in a sibling plugin, so my files were never fully checked.
I re-read every declaration my port calls. **One real error was found and
fixed**; everything else matched.

### 9.1 What I changed

**`crates/compat/tests/layer_ex_long_exposure.rs`** — the pixel helper read
the layer's name as an `Option<String>`:

```rust
.find(|layer| layer.name.as_deref() == Some(name))   // before
```

but `LayerState::name` is a plain `String`
(`crates/engine/src/scene.rs:210`), and `String` has no `as_deref` (that
method is on `Option`). It is now

```rust
.find(|layer| layer.name == name)
```

which is the form the sibling test uses
(`crates/compat/tests/layer_ex.rs:104`). Nothing else changed; the patch was
regenerated and re-verified (`git apply --check --reverse` passes).

### 9.2 The audit — every engine/compat API the port calls

**The compat context and its services**

| call | declaration |
|---|---|
| `ctx.services()` (unsafe) | `crates/compat/src/ctx.rs:327` — `pub unsafe fn services(&self) -> &CompatServices` |
| `services.state::<ExposureState>()` | `crates/compat/src/ctx.rs:245` — `pub fn state<T: Default + 'static>(&self) -> RefMut<'_, T>` (the shape of `crates/compat/src/natives/plugins/window_ex/host.rs:80`; **no `ctx.state(...)` anywhere in my port**) |
| `services.scene()` | `crates/compat/src/ctx.rs:117` — `pub fn scene(&self) -> Option<*mut Scene>` |
| `ctx.register_instance_members(&NativeInstanceMembers { .. })` | `crates/compat/src/ctx.rs:423` — `pub fn register_instance_members(&self, members: &NativeInstanceMembers<'_>) -> Result<(), CompatError>` |
| `CompatCtx::native_instance_id(host, id, &["Layer"])` | `crates/compat/src/natives/visual/common.rs:466` (the `impl CompatCtx` block opens at `:450`) — `pub(crate) fn native_instance_id(host: &Tjs2Engine, object: Tjs2ValueId, classes: &[&str]) -> Result<Option<u32>, String>` |
| `resolve(engine)` | `crates/compat/src/natives/system.rs:403` — `pub(crate) fn resolve(engine: *mut c_void) -> Option<EngineBinding>` |
| `EngineBinding::ctx()` | `crates/compat/src/natives/system.rs:413` — `pub(crate) fn ctx(&self) -> CompatCtx` |
| `EngineBinding::engine()` | `crates/compat/src/natives/system.rs:418` — `pub(crate) fn engine(&self) -> &'static tjs::Tjs2Engine` |
| `construct::dictionary_of(host, entries)` | `crates/compat/src/natives/construct.rs:67` — `pub(crate) fn dictionary_of(host: &Tjs2Engine, entries: &[(&str, TjsValue)]) -> Result<DetachedValue, String>` |
| `tjs_string_to_integer(&value)` | `crates/compat/src/natives/tjsnum.rs:17` (`pub(crate) fn tjs_string_to_integer(s: &str) -> i64`), re-exported at `crates/compat/src/natives/system.rs:469` |
| `callback_args(argv, argc)` | `crates/compat/src/native_util.rs:42` — `pub(crate) fn callback_args(argv: *const Value, argc: c_int) -> &'static [Value]` |
| `set_void(out)` | `crates/compat/src/native_util.rs:126` — `pub(crate) fn set_void(out: *mut Value)` |
| `report_error(out_error, &msg)` | `crates/compat/src/native_util.rs:188` — `pub(crate) fn report_error(out_error: *mut *mut c_char, msg: &str) -> c_int` |

**The L2 scene (`crate::engine_api::Scene` → `crates/engine/src/scene.rs`)**

| call | declaration |
|---|---|
| `scene.layer(id)` | `crates/engine/src/scene.rs:1796` — `pub fn layer(&self, id: u32) -> Option<&LayerState>` |
| `scene.bitmap(id)` | `crates/engine/src/scene.rs:4194` — `pub fn bitmap(&self, id: u32) -> Option<&BitmapState>` |
| `scene.resolve_bitmap(id)` | `crates/engine/src/scene.rs:3457` — `pub fn resolve_bitmap(&mut self, id: u32) -> bool` |
| `scene.with_main_image_pixels(id, f)` | `crates/engine/src/scene.rs:2171` — `pub fn with_main_image_pixels<R>(&self, layer_id: u32, f: impl FnOnce(&[u8]) -> R) -> Option<R>` |
| `scene.with_main_image_pixels_mut(id, f)` | `crates/engine/src/scene.rs:2182` — `pub fn with_main_image_pixels_mut<R>(&mut self, layer_id: u32, f: impl FnOnce(&mut [u8]) -> R) -> Option<R>` |
| `scene.mark_layer_image_modified(id)` | `crates/engine/src/scene.rs:2013` — `pub fn mark_layer_image_modified(&mut self, id: u32)` |
| `layer.bitmap` | `crates/engine/src/scene.rs:129` — `pub bitmap: Option<u32>` |
| `layer.image_width` / `layer.image_height` | `crates/engine/src/scene.rs:154` / `:155` — `pub image_width: u32` / `pub image_height: u32` |
| `bitmap.rgba` | `crates/engine/src/scene.rs:394` — `pub rgba: Vec<u8>` |

**The VM (`crates/tjs`)**

| call | declaration |
|---|---|
| `host.retain_object_detached(objthis)` | `crates/tjs/src/lib.rs:2301` — `pub fn retain_object_detached(&self, obj: impl Into<Tjs2ValueId>) -> Result<DetachedValue, String>` (`impl From<*mut c_void> for Tjs2ValueId` at `:749`) |
| `host.retain_object_arg(&value)` | `crates/tjs/src/lib.rs:2287` — `pub fn retain_object_arg(&self, v: &Value) -> Result<DetachedValue, String>` |
| `host.member_read(id, name)` | `crates/tjs/src/lib.rs:2557` — `pub fn member_read(&self, id: Tjs2ValueId, membername: &str) -> Result<MemberRead, String>` |
| `object.raw_id()` | `crates/tjs/src/lib.rs:863` — `pub fn raw_id(&self) -> Tjs2ValueId` |
| `Tjs2ValueId::as_ptr()` | `crates/tjs/src/lib.rs:770` — `pub const fn as_ptr(self) -> *mut c_void` |
| `MemberRead` / `TjsValue` shapes | `crates/tjs/src/lib.rs:919` / `:891` |
| `Value` (the FFI struct) and `VAL_*` | `crates/tjs/src/lib.rs:312` / `:40-60` |
| `NativeObjThisMethodFn` (the callback signature) | `crates/tjs/src/lib.rs:258` |
| `NativeObjThisMethodDef` / `NativeInstanceMembers` | `crates/tjs/src/lib.rs:1062` / `:1089` |

**The test file** — `common::Env::new/run/eval/eval_int/error/link_plugin`
(`crates/compat/tests/common/mod.rs:116/256/263/274/291/446`), the public
`scene` field (`crates/compat/tests/common/mod.rs:76`),
`Scene::layers` (`crates/engine/src/scene.rs:907`),
`LayerState::name` (`:210`), `LayerState::bitmap` (`:129`),
`Scene::bitmap` (`:4194`), `BitmapState::pixel_offset`
(`crates/engine/src/scene.rs:470`).

### 9.3 Two things I checked because they are the usual traps

* **The lint level.** `#![allow(unsafe_code)]` sits in `mod.rs`; the
  out-of-line submodules `accumulate.rs` and `report.rs` inherit it. The
  tree proves the inheritance: `crates/compat/src/natives/plugins/
  motion_player/player_members.rs` uses `unsafe` 28 times and carries no
  allow of its own, while `motion_player/mod.rs:33` carries it. (The crate
  root denies it — `crates/compat/src/lib.rs:53`.)
* **The state borrow.** `exposure_state(ctx)` returns a `RefMut` borrowed
  from `ctx`; the `&mut Scene` from `scene_mut(ctx)` is
  `Result<&'a mut Scene, String>` with an unconstrained `'a`, so it does not
  borrow `ctx` and the two can coexist. Every `&mut Scene` is confined to
  its own block, and the `RefMut` is taken and dropped before the next one.

### 9.4 Still unverified

The code has still never been compiled — the integrator's build is the
first check. The compiler-free gate checks pass on my files
(`house-style`, `layering`, `log-format`, `stateless`,
`callback-objthis`, `void-getter`, `native-instance-lookup`; the
`error-handling` failure is in `save_struct/scripts_member.rs:546`, a
sibling's file).
