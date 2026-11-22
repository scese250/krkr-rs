# `Layer.stretchCopyAA` — writer's report

**Patch:** `/tmp/pi-patches/write-layerExAreaAverage/patch.diff`
(three new files; `git apply` verified with a dry run against an empty tree)

**Oracle:** the `wtnbgo/layerExAreaAverage` module, commit
`b04fbd29548e43d76293625e1d06a3141f0853d8` — `main.cpp` (the member) and
`manual.tjs` (the signature). The engine's own checkout carries no build of
this module, so every citation in the tests points into that tree. The
engine's own `Layer` members are cited from the checkout only where their
meaning matters.

---

## 1. Files written

| file | what is in it |
|---|---|
| `crates/compat/src/natives/plugins/layer_ex_area_average/mod.rs` | the plugin module: the `register` entry point, the native callback `layer_stretch_copy_aa`, the argument marshalling (the nine arguments, the integer coercion, the source resolution), the error texts, and the side effects (image modified, pending paint). |
| `crates/compat/src/natives/plugins/layer_ex_area_average/area.rs` | the kernel: the fixed-point representation, the four clipping steps, the two truncated scale steps, the covered region, the visited source range with its two clamps, the overlap areas, the accumulators, the alpha-weighted colour, the transparent fallback and the written pixel. |
| `crates/compat/tests/layer_ex_area_average.rs` | 16 behaviour tests driving the VM and the member. |

Nothing else was touched. `crates/compat/src/natives/plugins/mod.rs` and
`crates/compat/src/natives/visual/layer/` are untouched, as instructed.

## 2. The script-visible surface

Exactly one member is registered, on the engine's own `Layer` class:

* `Layer.stretchCopyAA(dleft, dtop, dwidth, dheight, src, sleft, stop,
  swidth, sheight)` — an instance method, nine required arguments, no
  defaults, returns nothing (`void`).

No property, no new class, no new namespace. The member is absent from
`Layer` until the module is linked (`Plugins.link("layerExAreaAverage.dll")`),
as the module attaches on link.

Registration shape (`mod.rs:37-52`), the same shape the sibling
`shrinkCopy` port uses:

```rust
ctx.register_instance_members(&NativeInstanceMembers {
    class_name: "Layer",
    methods: vec![NativeObjThisMethodDef {
        name: "stretchCopyAA",
        f: layer_stretch_copy_aa,
    }],
    properties: Vec::new(),
})?;
```

## 3. The plugin-table entry the integrator has to add

`crates/compat/src/natives/plugins/mod.rs`:

```rust
pub mod layer_ex_area_average;
```

and, in `BUILTIN_PLUGINS` (alphabetically just before the `layerExDraw.dll`
entry):

```rust
    BuiltinPluginDef {
        name: "layerExAreaAverage.dll",
        aliases: &[],
        dependencies: &[],
        register: layer_ex_area_average::register,
    },
```

* `aliases`: none — the module has one name (`CMakeLists.txt:1-8` builds one
  module, `layerExAreaAverage`).
* `dependencies`: none — the member needs nothing but the engine's `Layer`
  class.
* The name is the DLL name the script passes to `Plugins.link`; the registry
  lower-cases the reported names, so `Plugins.getList()` reports
  `layerexareaverage.dll` (the test pins that).

## 4. The eight questions, answered

### 4.1 The magnification error's wording

The upstream's message is Japanese; the engine's messages are English. The
tree's convention is `Layer.<member>: <what went wrong>` — the sibling
plugin's refusal reads `Layer.shrinkCopy: the source rectangle must cover the
destination rectangle`
(`crates/compat/src/natives/plugins/shrink_copy/area.rs:178-184`), and every
native member in `crates/compat/src/natives/visual/layer/` prefixes its
message with `Layer.<method>:`.

The port raises (`mod.rs:192-198`):

```
Layer.stretchCopyAA: cannot magnify; the destination rectangle must not be wider or taller than the source rectangle
```

Same meaning as the upstream's "stretchCopyAA cannot perform magnification".

### 4.2 A missing main image

The upstream dereferences a null address. The port raises a typed error.

The tree's own answer for this case is `main_bitmap`
(`crates/compat/src/natives/visual/layer/util.rs:192-206`), which raises
`Layer.<method>: this layer has no main image` when the layer has no
`MainImage`. The plugin has the same rule in its own helper
(`mod.rs:116-127`):

* the destination without an image → `Layer.stretchCopyAA: the destination
  layer has no main image`;
* the source without an image → `Layer.stretchCopyAA: the source layer has
  no main image`.

The check runs **after** the magnification check, because the upstream reads
the (null) addresses without failing and then refuses the magnification; the
error a script sees for "no image *and* a magnification" is therefore the
magnification error, as upstream.

### 4.3 Negative or out-of-range origins

Two different cases, two different answers:

* **A negative origin** (`dleft`, `dtop`, `sleft`, `stop` below zero) is
  **refused** with a typed error (`mod.rs:199-207`):
  `Layer.stretchCopyAA: the destination origin is negative` /
  `… the source origin is negative`. The upstream addresses memory outside
  the image there (undefined); clamping would silently move the copy to a
  place the caller did not ask for, and dropping the pixels would quietly
  change the result, so the port refuses and names the problem.
* **An origin beyond the image** is *not* an error: the upstream's own
  clipping (§7 of the specification) makes the rectangle's size negative and
  the copy writes nothing. That defined behaviour is reproduced exactly
  (`area.rs:108-143`), and the loop is additionally clamped to the image
  (`area.rs:175-184`) as a guard that can never fire.

### 4.4 Bit-exact fixed-point arithmetic

Reproduced bit for bit, in `area.rs`:

* 12 fractional bits, one pixel = 4096 units (`area.rs:21-24`);
* stored lengths are converted back with an arithmetic right shift (a floor
  division), `area.rs:234-237`;
* the two scale steps are `trunc(4096 * srcWidth / destWidth)` — the
  multiplication first, then the division, truncated toward zero
  (`area.rs:167-168`);
* the four clipping steps compute their partner's new size from the
  partner's **old** size, in **double precision**, truncated toward zero
  (`area.rs:98-101`, `area.rs:108-143`). The double-precision detail is
  observable: `swidth * ((new) / (old))` is not the same number as the
  exact rational `swidth * new / old` (e.g. `3 * (1/3)` truncates to 0 in
  double precision and to 1 in exact arithmetic), so the port uses `f64`
  exactly where the specification says the arithmetic is done in double
  precision;
* the colour weight is `(overlapArea * alpha) >> 8` — the shift is 8, not 12
  (`area.rs:253-257`);
* the destination alpha is `trunc(alphaSum / totalArea)` and the colour is
  `trunc(colourSum / colourWeight)`, with the transparent fallback and the
  zero-colour case (`area.rs:275-292`).

**Nothing was reused from the engine.** `crates/engine/src/layer_ops.rs`'s
`stretch_blit` / `stretch_blit_on` (lines 2169, 4488) are a different
sampler (the engine's own resampling with stretch types), and the sibling
`Layer.shrinkCopy` port (`crates/compat/src/natives/plugins/shrink_copy/area.rs`)
is the *span-table* average at a 1/256-pixel resolution — a different
algorithm with different results. Neither can express this member's
semantics, so the kernel is new.

**Where the kernel lives, and why.** Under the layer rules a per-pixel
bitmap operation would normally be an L2 one, and this kernel is generic
(no TVP vocabulary). It is nevertheless implemented in the plugin module,
for two reasons: (a) my write scope excludes `crates/engine/`, and (b) the
engine has no operation with these semantics to extend — the closest sibling
(`shrinkCopy`) also keeps its kernel in L3, in
`crates/compat/src/natives/plugins/shrink_copy/area.rs`. **This is a
follow-up for the integrator**: moving `area.rs` into
`crates/engine/src/layer_ops.rs` as a plain bitmap operation (and re-exporting
it through `crates/compat/src/engine_api.rs`) would be the shape the layer
rules prefer. It is a pure move — the kernel names no TVP concept.

### 4.5 The dropped edge column and row

Reproduced exactly, and documented in the module doc so that nobody "fixes"
it later.

The visited range's end is the column that holds the covered region's right
edge, the range itself stops **before** it, and the end is pulled back to
the source image's last index (`area.rs:234-237`):

```rust
let end_col = ((region_right + UNIT - 1) >> FRACTION).min(src_image.0 - 1);
```

so the source image's last column and last row are never sampled, and a
copy of the same size leaves the destination's last column and last row
untouched. The three tests that pin it are
`a_copy_of_the_same_size_leaves_the_last_column_and_row_untouched`,
`the_source_images_last_column_is_never_sampled` and
`every_destination_pixel_of_a_three_by_three_copy_is_written` (the last one
pins the *other* side of the quirk: a 3x3 copy out of a 3x3 source rectangle
writes all nine pixels, because the covered range of the last pixel is
non-empty and inside the image).

### 4.6 The repaint against the L2 dirty-region mechanism

Both halves of the engine's mechanism are used, because the engine's
`Layer.update()` and the L2 dirty region are two different things:

* the **upload region** is the L2 dirty region: `BitmapState::mark_dirty_rect`
  (`crates/engine/src/scene.rs:444-466`) with the *post-clipping*
  destination rectangle (`dleft`, `dtop`, `destWidth`, `destHeight`) — the
  rectangle the upstream's `update(x, y, w, h)` marks
  (`area.rs:205-214`);
* the **repaint** is the engine's `Layer.update()`: the layer's pending-paint
  flag, which the frame's paint dispatch turns into the layer's `onPaint`
  (`crates/compat/src/natives/visual/layer/mod.rs:1418-1447`,
  `Scene::set_layer_pending_paint` at `crates/engine/src/scene.rs:1999-2005`).

Both happen after the copy, unconditionally — the upstream's update call
happens even when nothing was written. The write also marks the layer's
image modified (`Scene::mark_layer_image_modified`,
`crates/engine/src/scene.rs:2007-2017`), because the upstream reaches the
destination through the *write* address, which marks the image modified.

### 4.7 The upstream's log line

**Not reproduced.** The upstream writes one success diagnostic per call
(`stretch copy by area average:… , time = N(ms)`). The engine's log
vocabulary (`docs/rules/logging.md` §1) is a set of subsystem records for
events worth a reader's attention, and the sibling plugin ports write no log
line for a successful copy (`crates/compat/src/natives/plugins/shrink_copy/`
writes none). A per-call success line would also flood the log of a game
that copies every frame. Nothing script-visible is lost: the line is a log
entry, not a value. If the integrator wants it, the engine's place would be
a `compat.plugin` record — but it is not part of the member's behaviour.

### 4.8 The plugin-table entry

See §3 above.

## 5. The tests

`crates/compat/tests/layer_ex_area_average.rs` — 16 tests, all driving the
real script surface through the VM (`Plugins.link`, `Env::run`, `Env::eval`,
`Env::error`). Every behaviour assertion cites the upstream line that
defines it.

| test | what it pins | citation |
|---|---|---|
| `the_link_resolves_to_the_builtin` | the member is absent before the link, present after it, and the linked module is the DLL name | `main.cpp:207-210` |
| `the_copy_needs_nine_arguments` | fewer than nine arguments raises | `main.cpp:16-21` |
| `the_magnification_is_refused` | a wider or taller destination raises; the same size is allowed | `main.cpp:65-70` |
| `a_copy_of_the_same_size_leaves_the_last_column_and_row_untouched` | the dropped edge column/row: nine pixels written, seven untouched | `main.cpp:116-121,136,163` |
| `the_source_images_last_column_is_never_sampled` | the green last column never mixes into a 2x2 downscale | `main.cpp:116-121,136` |
| `the_colour_is_weighted_by_the_area_and_the_alpha` | `0xBFA90055` — the area × alpha colour weight and the area-weighted alpha | `main.cpp:146-158,166,179-181` |
| `a_transparent_source_falls_back_to_its_own_colours` | `0x0019232D` — the transparent fallback | `main.cpp:153-158,167-176` |
| `the_destination_rectangle_is_clipped_to_the_destination_image` | the destination rectangle is cut back and the source scaled by the same ratio | `main.cpp:73-78` |
| `the_source_rectangle_is_clipped_to_the_source_image` | the source rectangle is cut back and the destination scaled by the same ratio | `main.cpp:79-84` |
| `the_covered_region_is_anchored_at_the_source_rectangles_origin` | the covered region starts at the source rectangle's origin, not at the image's | `main.cpp:98-99,110-113` |
| `every_destination_pixel_of_a_three_by_three_copy_is_written` | all nine pixels written; the 4x4 version of the same call is a magnification | `main.cpp:100-107,110-121`, `main.cpp:65-70` |
| `the_copy_marks_the_written_rectangle_and_the_layer` | the dirty rectangle, the pending paint and the modified image | `main.cpp:189-199` |
| `the_member_returns_nothing` | the member returns `void` | `main.cpp:203` |
| `a_layer_without_an_image_raises` | a destination or source without an image raises | `main.cpp:35-36,55-56` |
| `a_negative_origin_raises` | a negative origin raises | `main.cpp:105-106,128-129` |
| `the_source_must_be_a_layer` | a non-object source and a non-layer object raise | `main.cpp:48` |
| `the_numbers_are_read_with_the_integer_conversion` | an object argument raises the conversion error; a real is truncated toward zero | `main.cpp:40-43,60-63` |
| `the_source_and_the_destination_may_be_the_same_layer` | a layer may be its own source | `main.cpp:103-107` |

The six worked examples of the specification are all covered: (a) and (f) by
the first and the eleventh test, (b) by the second, (c) by the sixth, (d) by
the seventh, (e) by the eighth.

**The arithmetic was checked against the specification before the code was
written.** A line-by-line model of the kernel (the clipping's double
precision, the truncated steps, the exclusive range, the two clamps, the
overlaps, the `>> 8` weight, the fallback) reproduces all six worked
examples and the two clipping cases; the model is at
`/tmp/pi-patches/write-layerExAreaAverage/check_examples.py` and prints
`all examples agree`. The Rust code is the same algorithm; it has not been
compiled here (no cargo in a worker's scope).

## 6. Divergences from the upstream, and why

1. **A layer without an image raises** instead of dereferencing a null
   address (upstream: undefined, in practice a crash). §4.2.
2. **A negative origin raises** instead of writing outside the image
   (upstream: undefined). §4.3.
3. **The source must be a `Layer`.** The upstream reads four members from
   whatever object it is given and never checks the type; the port resolves
   the source through the VM's native-instance lookup and raises
   `the source is not a Layer` otherwise. In the upstream's own tree those
   four members (`imageWidth`, `imageHeight`, `mainImageBuffer`,
   `mainImageBufferPitch`) are `Layer` members, so the upstream's source is
   a layer in practice; the port's rule is the same restriction with a
   typed error instead of a "member not found".
4. **The arity error's wording.** `Layer.stretchCopyAA requires 9 arguments`
   instead of the VM's "Invalid argument count" text — the tree's convention
   (`crates/compat/src/natives/plugins/shrink_copy/mod.rs:165-169`). The
   behaviour (raise, nothing read or written) is the same.
5. **The conversion error's wording.** The tree's own text
   (`value is an object, not an integer`, from
   `crates/compat/src/natives/system.rs:453-465`) instead of the VM's
   conversion error text. The behaviour (raise) is the same.
6. **A self-copy reads the original source pixels.** The upstream writes as
   it reads, so a copy of a layer into itself would read the pixels it had
   already written. The engine's `Scene::with_two_bitmaps`
   (`crates/engine/src/scene.rs:5265-5280`) snapshots the source when it is
   the destination bitmap, so the reads see the original pixels. The
   specification's own wording sanctions this ("the destination is not read,
   so source and destination may be the same layer").
7. **The update is the engine's update.** The upstream calls the
   script-visible `update(x, y, w, h)`; the engine's `Layer.update()` takes
   no rectangle, so the port marks the L2 dirty region with the rectangle and
   sets the pending-paint flag. §4.6.
8. **No log line.** §4.7.

## 7. Risks

* **The surface lock will fail.** `Layer.stretchCopyAA` is a new member on
  the `Layer` class, and `docs/surface/runtime_surface.json` is the
  committed baseline that `krkr-gate surface-lock` compares against the live
  dump (the dump links every built-in plugin —
  `crates/compat/tests/runtime_surface.rs:207-212`). The integrator must run
  `uv run --project scripts krkr-gate surface-lock --update` after the
  build and commit the new baseline as a reviewed diff.
* **The plugin-parity report** (`docs/plugin_parity.md`) will list
  `Layer.stretchCopyAA` as a plugin extra once it is regenerated. Extras do
  not fail the check, and the gate runs it with `--no-doc`, so the report is
  a deliberate regeneration, not a gate blocker.
* **The tests fail until the plugin table is wired.** Every test in the file
  needs the `BuiltinPluginDef` entry; without it `Plugins.link` warns,
  registers nothing and the tests fail (they do not pass quietly).
* The kernel is in L3 although it is generic; see §4.4. Moving it to L2 is a
  pure move but touches `crates/engine/` and `engine_api.rs`, which was
  outside this scope.
* The cost of the copy is the upstream's: one destination pixel reads every
  source pixel it covers. A large downscale is expensive in both.

## 8. What was left alone

* `crates/compat/src/natives/plugins/mod.rs` — the module declaration and
  the table entry are the integrator's (§3).
* `crates/compat/src/natives/visual/layer/` — the member is registered onto
  the host class from the plugin module; nothing in the `Layer` natives was
  touched.
* `crates/engine/` — the kernel is in the plugin module (§4.4).
* `docs/` — the plugin's own page
  (`docs/code/compat/plugins/layer_ex_area_average.md`) does not exist and
  was not written: the specification asks for the dropped-edge quirk to be
  noted there, and that page is the integrator's to write. The quirk is
  documented in the module doc in the meantime.
* The other in-flight plugin modules in the tree (`layer_ex_btoa`,
  `layer_ex_long_exposure`) — untouched; their gate failures are theirs.

## 9. What the integrator has to do

1. Add the wiring of §3 to `crates/compat/src/natives/plugins/mod.rs`.
2. Build and run the suites:
   * `uv run --project scripts krkr-gate gate`
   * `uv run --project scripts krkr-gate test`
   * the plugin's own suite: `cargo test -p compat --test layer_ex_area_average`
3. Accept the new surface: `uv run --project scripts krkr-gate surface-lock
   --update` after the build, and commit the baseline diff.
4. Optionally write `docs/code/compat/plugins/layer_ex_area_average.md` and
   regenerate `docs/plugin_parity.md`.

**Gate state of this change (compiler-free checks only — no cargo was run
here):** `stateless`, `error-handling`, `house-style`, `layering`,
`callback-objthis`, `void-getter`, `native-construction`, `lifecycle-order`,
`storage-boundary` and `doc-lists` are clean for these files.
`native-arity` and `storage-media` cannot run without the reference
checkout. The `log-format` failure in the tree comes from another worker's
`layer_ex_long_exposure` files, not from these.
