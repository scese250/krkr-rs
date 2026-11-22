# `layerExImage` — the writer's report

## The compile fix (round 2)

The integrator's build named one error: `ctx.state::<Random>()` does not
exist. The per-binding state cell is `CompatServices::state::<T>()`
(`crates/compat/src/ctx.rs:245`, inside `impl CompatServices`, which spans
`ctx.rs:87-291`), and the context reaches the services through the unsafe
accessor `CompatCtx::services()` (`crates/compat/src/ctx.rs:327`).

**What I changed** — in `crates/compat/src/natives/plugins/layer_ex_image/noise.rs`,
both members that draw now go through the services, in the shape the tree
uses (`crates/compat/src/natives/async_trigger.rs:370-373`):

```rust
    // SAFETY: the caller resolved `ctx` from the installed binding.
    let services = unsafe { ctx.services() };
    let mut random = services.state::<Random>();
```

`noise.rs` also gained the file-level `#![allow(unsafe_code)]` it now needs
(the compat crate root denies unsafe code, `crates/compat/src/lib.rs:53`).

**The same mistake elsewhere: none.** `ctx.state::<…>` appeared only in those
two places; `mod.rs`, `colorize.rs` and `modulate.rs` never touch the state.

**Then I re-read every foreign API my port calls** and checked it against
its declaration — the list is at the end of this report. Two things that
check confirmed rather than changed:

* `layer_ex::bound` returns the **tuple** `(EngineBinding, CompatCtx)`
  (`crates/compat/src/natives/plugins/layer_ex/mod.rs:64`), not the binding
  alone — `receiver` destructures it as `let (resolved, ctx) = bound(engine)…`
  and takes the engine from `resolved.engine()`
  (`crates/compat/src/natives/system.rs:418`) ✓ (already correct in the
  delivered patch);
* the per-binding state is `CompatServices::state` (ctx.rs:245) and the
  context's accessor to it is `CompatCtx::services` (ctx.rs:327) — the one
  call that was wrong and is now fixed.

The patch has been regenerated with the fix.

## The patch

`/tmp/pi-patches/write-layerExImage/patch.diff` — five new files, no other
file touched. It applies with `git apply --check` / `git apply --3way`
(verified in a scratch clone: the five files appear, nothing else changes).

## The files I wrote

| file | what is in it |
|---|---|
| `crates/compat/src/natives/plugins/layer_ex_image/mod.rs` (246 lines) | the module, the registration on the host `Layer` class, and the shared helpers every member uses: the receiver lookup, the pixel window (`with_clip`), the per-pixel walk (`for_each_pixel`), and the two argument conversions |
| `crates/compat/src/natives/plugins/layer_ex_image/colorize.rs` (147) | `Layer.colorize(hue, sat, blend)`: the byte-packed lightness, the single-precision HSL → RGB step and the blend mix |
| `crates/compat/src/natives/plugins/layer_ex_image/modulate.rs` (198) | `Layer.modulate(hue, saturation, luminance)`: the double-precision HSL step, the hue fold and the byte store |
| `crates/compat/src/natives/plugins/layer_ex_image/noise.rs` (159) | `Layer.noise(level)` and `Layer.generateWhiteNoise()`, and the generator they draw from |
| `crates/compat/tests/layer_ex_image_effects.rs` (586) | the behaviour tests |

**The test file name.** The conventional name for this plugin's tests is
`crates/compat/tests/layer_ex_image.rs`, but that file already exists and
holds the `layerExDraw` family's `GdiPlus.Image` / `Layer.drawImage*` tests
(541 lines) — it is not mine and I did not touch it (its mtime is unchanged;
`git status` shows only the new file as added). The new tests therefore live
in `crates/compat/tests/layer_ex_image_effects.rs`. The integrator may
rename the older file if the tree wants the conventional name back.

The scope tool still named the old path, so the file was written under
`/tmp/pi-patches/write-layerExImage/layer_ex_image_effects.rs` and copied
into `crates/compat/tests/` from there.

## The wiring I need from the integrator

In `crates/compat/src/natives/plugins/mod.rs`:

```rust
pub mod layer_ex_image;            // next to `pub mod layer_ex;`
```

and one row in `BUILTIN_PLUGINS` (after the `layerExDraw.dll` row):

```rust
    BuiltinPluginDef {
        name: "layerExImage.dll",
        aliases: &[],
        dependencies: &[],
        register: layer_ex_image::register,
    },
```

Two follow-ups the gate will ask for:

* `krkr-gate surface-lock --update` — the four new `Layer` members change
  the runtime surface dump (`docs/surface/runtime_surface.json`).
* `docs/code/compat/plugins/layer_ex_image.md` and one row in the table of
  `docs/code/compat/plugin.md` — the tree's per-plugin doc convention. The
  gate does not check it, but the module map does.

## The script-visible members I register

Four **instance** members of the host's `Layer` class, registered with
`ctx.register_instance_members(&NativeInstanceMembers { class_name: "Layer",
… })` — the same shape the `layerExDraw` port uses
(`crates/compat/src/natives/plugins/layer_ex/mod.rs:574`):

| member | arity | returns |
|---|---|---|
| `colorize` | 3 (`hue`, `sat`, `blend`) | void |
| `modulate` | 3 (`hue`, `saturation`, `luminance`) | void |
| `noise` | 1 (`level`) | void |
| `generateWhiteNoise` | 0 | void |

No overloads, no optional arguments, no defaults; a call with *more*
arguments than the arity is accepted and the surplus is ignored. The
members exist only after `Plugins.link("layerExImage.dll")`, as upstream
(the plugin registers them when its DLL is linked).

## The questions the specification raises — my answers

### 1. The random source and the seed policy

**Answer: the port brings its own generator, and a run of the plugin is
reproducible.** A xorshift64\* whose state lives in the binding's own state
(`CompatCtx::state`, `crates/compat/src/ctx.rs:245`), seeded with a fixed
constant, so the same sequence of calls on a fresh binding produces the
same pixels. Successive calls continue the sequence, so a repeated call
does *not* reproduce the previous noise — the upstream's `rand()` behaves
the same way. The two members share one generator, as the upstream's
process-wide `rand()` is shared.

*Why:* the upstream's sequence is neither seeded nor reproducible, so
nothing can be matched; the tree's own answer to the same need is a seeded
generator with an explicit seed — the engine's L2 `noise` takes a `seed`
(`crates/engine/src/layer_ops.rs:1095`) and the tree's UUID generator is the
same xorshift64\* (`crates/compat/src/natives/system.rs:2558`). Keeping the
state in the binding's state (not a process global) is the tree's rule for
plugin state.

*What is kept from the upstream:* the offset is the truncation **toward
zero** of `(u − 0.5) × level`, the three channels draw independently, the
result is clamped into 0..255, the alpha byte is untouched, and the grey of
`generateWhiteNoise` is the draw divided by the integer constant
`2³¹−1 / 255 = 8 421 504` (so it is uniform over 0..=255). The draws are
taken in the buffer's own order — blue, green, red — which is the upstream's
order mapped onto the tree's channel layout.

### 2. A layer with no main image

**Answer: the call raises a typed error** — `Layer.<member>: this layer has
no main image` — and never writes through a null address.

*Why:* the upstream writes through a null buffer address (undefined
behaviour). The tree's rule is that a missing thing is named in an error,
never silently substituted, and the tree's own layer pixel operations raise
exactly this message for the same condition
(`crates/compat/src/natives/visual/layer/util.rs:192-197`).

### 3. The clip rectangle against the image bounds

**Answer: the rectangle is intersected with the image** (`engine_api::
layer_ops::intersect`, `crates/engine/src/layer_ops.rs:593`).

*Why:* the upstream trusts the rectangle; the tree intersects. The tree's
clip is stored already intersected with the image
(`crates/compat/src/natives/visual/layer/mod.rs:1900`), so the two agree in
practice — the port never writes past the image.

### 4. The repaint and the channel order

**Repaint:** after the write the port marks the layer for repaint
(`Scene::set_layer_pending_paint`, `crates/engine/src/scene.rs:1999`), marks
the changed rectangle of the bitmap dirty (`BitmapState::mark_dirty_rect`,
`crates/engine/src/scene.rs:451`) and sets the layer's `imageModified` flag
(`Scene::mark_layer_image_modified`, `crates/engine/src/scene.rs:2013`) —
the three things the upstream's `Layer.update(clipLeft, clipTop,
clipWidth, clipHeight)` plus its write-buffer request do in the tree.

**Channel order:** the tree's main image is straight-alpha RGBA8, `r, g, b,
a` per pixel (`crates/engine/src/scene.rs:386-395`), so the plugin's colour
channels are bytes 0, 1 and 2 and the alpha is byte 3. The upstream's
`B, G, R, A` buffer maps onto it as upstream-blue → tree-byte 2,
upstream-green → 1, upstream-red → 0, alpha → 3. The members never read or
write the alpha byte, so no premultiplication question arises.

### 5. Where the members live, and `modulate` / `generateWhiteNoise` in L2

**Answer: the four members live in the plugin module and are registered on
the host `Layer` class; `modulate` and `generateWhiteNoise` are *not* added
to L2, and the engine's L2 operations are not reused.**

* Why a plugin module and not the layer pixel module: the upstream
  publishes these members when its DLL is linked (`Main.cpp:30-37`), and the
  tree's rule for a plugin is that its members appear on the first
  `Plugins.link` (the `layerExDraw` port does exactly this,
  `crates/compat/src/natives/plugins/layer_ex/mod.rs:574`). Registering them
  on the `Layer` class from `crates/compat/src/natives/visual/layer/` would
  make them permanent class members, which is not what the plugin does.
* Why the engine's `colorize` was not reused: the engine's
  `layer_ops::colorize` (`crates/engine/src/layer_ops.rs:1029`) has
  **the same arithmetic** as the upstream's `colorize` — I transcribed the
  specification's 42 worked cells and checked them against a transcription
  of that function's arithmetic (including the single-precision rounding,
  which is what makes the 254/1/1 of a pure red come out), and all 42
  match. It is nevertheless *not* reusable from here: it is not on the L3
  seam (`crates/compat/src/engine_api.rs` does not re-export it, and the
  `l3-engine-api` rule forbids naming `engine::` outside that file), and
  the engine is outside my write scope.
* Why the engine's `noise` was not reused: it is **different** —
  `crates/engine/src/layer_ops.rs:1095` returns early for a level ≤ 0
  (the upstream darkens the pixels for a negative level) and takes a `seed`
  parameter. Reusing it would have silently changed the behaviour.
* `modulate` and `generateWhiteNoise` have no L2 counterpart at all.

**So the per-pixel work is in the plugin module (L3), which is a shape
deviation from the tree's rule that a per-pixel bitmap operation belongs in
L2.** It is forced by my write scope (the plugin module and the test file
only) and it is the option the brief allows ("implement the member in the
plugin, and say which you did and why"). **Follow-up for the integrator:**
move the four transforms into `crates/engine/src/layer_ops.rs` (the
`colorize` arithmetic is already there and matches) and re-export them
through `crates/compat/src/engine_api.rs`'s `layer_ops` list; the plugin
then becomes a thin shim and the duplicated arithmetic disappears. The
upstream's `noise` semantics (a signed level) would need the L2 operation
to grow a parameter or a second entry point.

## The tests, and what each one pins

`crates/compat/tests/layer_ex_image_effects.rs` — 16 tests, all driving the
real script surface (the members are called on a layer through the VM; the
pixels are read back from the layer's main image).

| test | what it pins |
|---|---|
| `the_members_exist_once_the_plugin_is_linked` | the members are absent before `Plugins.link` and present after (`Main.cpp:30-37`) |
| `colorize_takes_the_requested_hue_and_saturation` | the plugin's own 42-cell worked table (`LayerExImage.cpp:179-225`, `:78-119`, `:121-171`, `:182-188`) |
| `colorize_takes_the_hue_and_the_saturation_modulo_256` | the byte-sized hue and saturation, and the clamped blend (`:201-202`, `:182-188`) |
| `modulate_rotates_and_pushes_the_colour` | the plugin's own 49-cell worked table (`:318-341`, `:244-309`, `:227-242`) |
| `modulate_normalizes_the_hue_into_a_turn` | a rotation of 360 and of 540 degrees (`:244-309`) |
| `the_members_leave_the_alpha_byte_alone` | the alpha byte is never touched, and a transparent pixel is transformed like an opaque one (`:220`, `:336`, `:360`, `:378`) |
| `the_members_work_on_the_clip_rectangle_alone` | the clip rectangle bounds the work (`layerExBase.hpp:78-86`, `:99-103`, `LayerExImage.cpp:22-30`) |
| `noise_moves_the_colour_channels_and_keeps_the_alpha` | the offset stays within half the level and the alpha is untouched (`:347-365`) |
| `a_level_of_one_or_two_leaves_the_pixels_alone` | the smallest level that can move a pixel is 3 (`:347-365`) |
| `a_negative_level_mirrors_the_offsets_of_a_positive_one` | the offset is `(draw − 0.5) × level`, so a negative level mirrors the offsets (`:347-365`) |
| `the_noise_is_coloured` | the three channels draw independently (`:347-365`) |
| `the_noise_is_reproducible` | the port's own sequence: the same calls produce the same pixels |
| `generate_white_noise_gives_every_pixel_the_same_grey` | the three colour channels get the same value and the alpha is untouched (`:370-383`) |
| `the_grey_of_generate_white_noise_is_not_the_same_for_every_pixel` | the grey is drawn per pixel, not a constant (`:370-383`) |
| `the_members_refuse_a_call_with_too_few_arguments` | the arity (`LayerExImage.h:36`, `:44`, `:50`, `:55`) |
| `a_layer_without_a_main_image_reports_the_missing_image` | the typed error instead of a write through a null address (`layerExBase.hpp:80`) |

(The table-driven tests run the plugin's own table cell by cell; the
`modulate` table carries one deviation, see below.)

## The divergences I introduced, and why

1. **A layer with no main image raises** instead of writing through a null
   address (upstream: undefined behaviour).
2. **The clip rectangle is intersected with the image** (upstream: trusted
   as it is). The two agree in practice because the tree's clip is stored
   intersected with the image.
3. **The random sequence is the port's own** (upstream: the C library's
   process-wide, unseeded `rand()`). The distribution, the truncation and
   the clamping are identical; the sequence cannot be.
4. **A too-few-arguments call raises a message** (`Layer.colorize requires
   3 arguments`) rather than the VM's `TJS_E_BADPARAMCOUNT` code — the
   tree's convention for every native member
   (`crates/compat/src/natives/visual/layer/mod.rs:1682`).
5. **An object or an octet argument for a numeric parameter raises**
   instead of converting; the tree's rule
   (`crates/compat/src/natives/system.rs:457-470`), and the specification
   asks for it explicitly.
6. **A NaN `blend` leaves the pixels unchanged** (the multiplication and
   the truncation of a NaN are undefined upstream; the port's saturating
   float-to-int cast makes the weight zero).
7. **The specification's own table contradicts its own rule in two cells.**
   `modulate(0, -100, 0)` on a white pixel and on a black pixel: the table
   says `127, 127, 127` for both, while the rule the table's other rows
   follow — a desaturation leaves the pixel with *its own* lightness — gives
   `255, 255, 255` and `0, 0, 0`. The same table's lightness columns of
   those two rows (`modulate(0, 0, -100)` → black, `modulate(0, 0, 100)` →
   white) only hold with the grey's *own* lightness, so the two saturation
   cells are the odd ones out. **The port follows the rule**, and the two
   cells are pinned in the test with a comment that says so. If the
   integrator can check the upstream's behaviour for a grey pixel under a
   full desaturation, that is the one place where the port may differ from
   the table.
8. **The specification's §6.5 says a negative level "darkens the pixels".**
   Its own §6.3 formula (`offset = (int)((u − 0.5) × level)`) does not: a
   negative level *mirrors* the offsets of a positive one (a draw below the
   midpoint then brightens the channel). The port follows the formula, and
   the test pins the mirroring.

Nothing else diverges: the hue/saturation byte store, the blend weights and
the "full blend" threshold, the alpha handling, the empty-clip no-op, the
per-channel independence, the level's 32-bit wrap and the byte wrap of an
out-of-range channel all follow the specification.

## The risks

* **The per-pixel work sits in L3** (the plugin module) because the engine
  is outside my write scope. The integrator may want it in
  `crates/engine/src/layer_ops.rs` with the re-export through
  `crates/compat/src/engine_api.rs`; the `colorize` arithmetic is already
  in the engine and matches (I checked all 42 cells of the table against
  it), so that step would *remove* the duplication rather than add it.
* **The plugin reuses the `layerExDraw` port's helpers** (`bound`,
  `scene_mut`, `layer_from_objthis` — `pub(super)` in
  `crates/compat/src/natives/plugins/layer_ex/mod.rs`) instead of carrying a
  second copy of the pointer plumbing. The sibling plugin ports in this
  tree (`layer_ex_area_average`, `layer_ex_btoa`, …) each carry their own
  copies; the integrator may prefer to hoist the three helpers into
  `crates/compat/src/natives/plugins/mod.rs` and have every plugin use them.
* **The single-precision arithmetic must match.** The `colorize` step is
  sensitive to the order of the float operations (the 254/1/1 of a pure red
  comes from one rounding step in `l + s − l·s`). I verified the
  transcription against the specification's table in a script before
  writing the Rust; the build must confirm the Rust agrees.
* **`f32` in Rust is strictly IEEE-754 single precision** and is not
  contracted into fused multiply-adds, so the expressions keep the
  upstream's order.
* **The `Random` state is per binding**, and the binding is process-global
  and single — so the sequence is shared by every layer in the process, as
  the upstream's `rand()` is.

## What I left alone

* `crates/compat/src/natives/plugins/mod.rs` — the module declaration and
  the table row are reported above, not edited.
* `crates/compat/src/natives/visual/layer/` — untouched; the members are
  registered onto the host class from the plugin module.
* `crates/compat/tests/layer_ex_image.rs` — untouched (the draw family's
  tests).
* The engine, `crates/compat/src/engine_api.rs`, the docs, the surface
  baseline and the parity reports — untouched; the follow-ups are listed
  above.

## What I verified, and what I could not

Verified here (no cargo): `rustfmt --edition 2024 --check` is clean on all
five files; the compiler-free gate checks pass — `stateless`,
`error-handling`, `house-style`, `log-format`, `layering`,
`storage-boundary`, `native-construction`, `lifecycle-order`,
`callback-objthis`, `void-getter`, `native-instance-lookup`. Two checks
could not run here because the reference checkout is absent from the tree
(`storage-media`, `native-arity` — both fail with "no reference checkout
found", before looking at my files).

Not verified here (the integrator's job): the build, the clippy pass, the
test suites, `surface-lock` (the baseline needs the four new members) and
the parity reports.

## Every foreign API the port calls (for a compiler-less check)

Each line is the call as my code writes it, and the declaration it must
match. Nothing else in the port reaches outside its own module.

### `crates/compat/src/ctx.rs` (the context and the services)

| call | declaration |
|---|---|
| `ctx.services()` (unsafe) | `CompatCtx::services(&self) -> &CompatServices` — ctx.rs:327 |
| `services.state::<Random>()` | `CompatServices::state<T: Default + 'static>(&self) -> RefMut<'_, T>` — ctx.rs:245 (in `impl CompatServices`, ctx.rs:87-291) |
| `ctx.register_instance_members(&NativeInstanceMembers { class_name, methods, properties })` | `CompatCtx::register_instance_members(&self, &NativeInstanceMembers<'_>) -> Result<(), CompatError>` — ctx.rs:423 |

### `crates/compat/src/host.rs`

| call | declaration |
|---|---|
| (through `layer_ex::scene_mut`) `services.scene()` | `CompatServices::scene(&self) -> Option<*mut Scene>` — host.rs:117 |

### `crates/compat/src/natives/system.rs`

| call | declaration |
|---|---|
| (through `layer_ex::bound`) `resolve(engine)` | `resolve(engine: *mut c_void) -> Option<EngineBinding>` — system.rs:403 |
| (through `layer_ex::bound`) `resolved.ctx()` | `EngineBinding::ctx(&self) -> CompatCtx` — system.rs:413 |
| `resolved.engine()` | `EngineBinding::engine(&self) -> &'static tjs::Tjs2Engine` — system.rs:418 |
| `tjs_string_to_real(&str)` | `pub(crate) fn tjs_string_to_real(s: &str) -> f64` — system.rs:477 |
| `tjs_string_to_integer(&str)` | re-exported at system.rs:469; defined `pub(crate) fn tjs_string_to_integer(s: &str) -> i64` — crates/compat/src/natives/tjsnum.rs:17 |

### `crates/compat/src/native_util.rs`

| call | declaration |
|---|---|
| `callback_args(argv, argc)` | `pub(crate) fn callback_args(argv: *const Value, argc: c_int) -> &'static [Value]` — native_util.rs:42 |
| `value_as_string(&Value)` | `pub(crate) fn value_as_string(v: &Value) -> String` — native_util.rs:110 |
| `set_void(out)` | `pub(crate) fn set_void(out: *mut Value)` — native_util.rs:126 |
| `report_error(out_error, msg)` | `pub(crate) fn report_error(out_error: *mut *mut c_char, msg: &str) -> c_int` — native_util.rs:188 |

### `crates/compat/src/natives/plugins/layer_ex/mod.rs` (the sibling port's shared helpers)

| call | declaration |
|---|---|
| `bound(engine)` | `pub(super) fn bound(engine: *mut c_void) -> Result<(EngineBinding, CompatCtx), String>` — layer_ex/mod.rs:64 (returns the **tuple**) |
| `scene_mut(ctx)` | `pub(super) fn scene_mut<'a>(ctx: &CompatCtx) -> Result<&'a mut Scene, String>` — layer_ex/mod.rs:74 |
| `layer_from_objthis(host, objthis)` | `pub(super) fn layer_from_objthis(host: &Tjs2Engine, objthis: *mut c_void) -> Result<Option<u32>, String>` — layer_ex/mod.rs:108 |

### `crates/compat/src/engine_api.rs` (the L3 → L2 seam)

| call | declaration |
|---|---|
| `engine_api::BitmapState` | re-export — engine_api.rs:29-31 |
| `engine_api::layer_ops::RectI` | re-export — engine_api.rs:110-124 |
| `engine_api::layer_ops::clip_rect` | re-export — engine_api.rs:110-124 |
| `engine_api::layer_ops::intersect` | re-export — engine_api.rs:110-124 |

### `crates/engine/src/scene.rs`

| call | declaration |
|---|---|
| `scene.layer(id)` | `pub fn layer(&self, id: u32) -> Option<&LayerState>` — scene.rs:1796 |
| `scene.resolve_bitmap(id)` | `pub fn resolve_bitmap(&mut self, id: u32) -> bool` — scene.rs:3457 |
| `scene.bitmap(id)` | `pub fn bitmap(&self, id: u32) -> Option<&BitmapState>` — scene.rs:4194 |
| `scene.bitmap_mut(id)` | `pub fn bitmap_mut(&mut self, id: u32) -> Option<&mut BitmapState>` — scene.rs:4207 |
| `scene.mark_layer_image_modified(id)` | `pub fn mark_layer_image_modified(&mut self, id: u32)` — scene.rs:2013 |
| `scene.set_layer_pending_paint(id, true)` | `pub fn set_layer_pending_paint(&mut self, id: u32, pending: bool) -> bool` — scene.rs:1999 |
| `bitmap.mark_dirty_rect(region)` | `pub fn mark_dirty_rect(&mut self, rect: PixelRegion)` — scene.rs:451 |
| `bitmap.pixel_offset(x, y)` | `pub fn pixel_offset(&self, x: u32, y: u32) -> Option<usize>` — scene.rs:470 |
| `bitmap.rgba` / `bitmap.width` / `bitmap.height` | fields — scene.rs:394 / 390 / 392 |
| `layer.bitmap` / `layer.clip` | fields — scene.rs:129 / 195 |

### `crates/engine/src/layer_ops.rs`

| call | declaration |
|---|---|
| `layer_ops::intersect(bitmap, rect)` | `pub fn intersect(bitmap: &BitmapState, rect: RectI) -> Option<RectI>` — layer_ops.rs:593 |
| `layer_ops::clip_rect(clip)` | `pub fn clip_rect(clip: Option<Rect>) -> RectI` — layer_ops.rs:611 |

### `crates/platform/api/src/geometry.rs`

| call | declaration |
|---|---|
| `PixelRegion`, `PixelRegion::ZERO`, `PixelRegion::from_min_max`, `region.x/y/right()/bottom()` | `pub struct PixelRegion` — geometry.rs:128; `ZERO` 134; `from_min_max` 160; `right()` 170; `bottom()` 175 |

### `crates/tjs/src/lib.rs`

| call | declaration |
|---|---|
| `NativeInstanceMembers { class_name, methods, properties }` | tjs/src/lib.rs:1089 |
| `NativeObjThisMethodDef { name, f }` | tjs/src/lib.rs:1062 |
| `Value { ty, integer, real }`, `VAL_INTEGER/VAL_REAL/VAL_STRING/VAL_VOID` | the VM's value type |

### `crates/compat/tests/common/mod.rs` (the test harness)

| call | declaration |
|---|---|
| `Env::new()` | common/mod.rs (the `Env` struct at 72, `build` at ~110) |
| `env.run(script)` | `pub fn run(&self, script: &str)` — common/mod.rs:256 |
| `env.error(expression)` | `pub fn error(&self, expression: &str) -> String` — common/mod.rs:291 |
| `env.link_plugin(dll)` | `pub fn link_plugin(&self, dll: &str)` — common/mod.rs:446 |
| `env.scene` | `pub scene: Box<Scene>` — common/mod.rs:76 |
