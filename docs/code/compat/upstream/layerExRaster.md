# `layerExRaster` — behavioural specification for `Layer.copyRaster`

A clean-room specification written from the upstream source only. It contains no
upstream code: everything below is prose, arithmetic and behaviour. Every claim
carries the upstream `file:line` that supports it, so the port's behaviour test
can cite the upstream as its oracle.

---

## 0. Provenance

| what | where |
|---|---|
| upstream repository | `https://github.com/wtnbgo/layerExRaster` |
| commit read | `a4c9be0c7510cca552e27c0c3e931b2eaee399af` ("update for linux: src/plugins/layerExRaster", 2026-03-31) |
| files in that repository | `main.cpp` (111 lines), `manual.tjs` (20), `readme.txt` (14), `CMakeLists.txt` (8), `layerExBase.hpp` (120) |
| shared base class | `https://github.com/wtnbgo/layerExImage`, commit `3aff625dba5c545ba722bb43b0a5ca1ad4f4336d`, file `layerExBase.hpp` |
| binding library | `ncbind` (`https://github.com/wamsoft/ncbind`, commit `256502b817e6627c1491b916938448470df395fd`) — supplies the argument-count rule only |

Two repositories are cited below, and they are not interchangeable:

* **`layerExRaster/main.cpp`** — the plugin's own source. It holds the raster
  operation and the registration of the `Layer` member.
* **`layerExImage/layerExBase.hpp`** — the shared base class the plugin
  inherits from. `main.cpp:7` includes `"../layerExImage/layerExBase.hpp"`, i.e.
  a *sibling directory*, not a file inside this repository. It holds the
  clip-rectangle reads, the per-call refresh of the cached layer state and the
  redraw request.

**The `layerExBase.hpp` that sits in the `layerExRaster` repository is dead
code.** It is an older copy of the base class: it has no clip members at all
(so the plugin's own source could not compile against it) and its redraw
request uses the image origin instead of the clip rectangle
(`layerExRaster/layerExBase.hpp:96-100`). It is *not* included by the build.
Everything this specification says about the clip rectangle and the redraw
comes from the `layerExImage` copy, which is the one the build actually uses.

The plugin's own manual (`layerExRaster/manual.tjs:18`) states the member as
`copyRaster(src, maxh, lines, cycle, time)` on `class Layer`.

---

## 1. What the plugin adds to the script surface

Exactly one member, and no new TJS class.

* The member is an **instance method on the engine's own `Layer` class**: the
  plugin registers it by *attaching* to the existing `Layer` class
  (`layerExRaster/main.cpp:109-111`). There is no `layerExRaster` class, no
  namespace and no global.
* The member name is **`copyRaster`** (`layerExRaster/main.cpp:110`,
  `layerExRaster/manual.tjs:18`).
* The plugin adds **nothing else**: no property, no constant, no callback, no
  event. `Layer.copyRaster` is the whole surface.
* The plugin does not register the `Layer` class itself; it only decorates it,
  so it may be loaded only after the host's `Layer` class exists.

The upstream's DLL/module name is `layerExRaster` — it is the CMake target name
and therefore the built file name (`layerExRaster/CMakeLists.txt:5-8`).

---

## 2. `Layer.copyRaster` — the signature

    Layer.copyRaster(src, maxh, lines, cycle, time) -> void

| # | name | script type | direction | default | meaning |
|---|---|---|---|---|---|
| 1 | `src` | object (a `Layer`) | in | **none — mandatory** | the layer whose main image is copied |
| 2 | `maxh` | integer | in | **none — mandatory** | maximum horizontal displacement, in pixels |
| 3 | `lines` | integer | in | **none — mandatory** | number of image rows in one wave period |
| 4 | `cycle` | integer (milliseconds) | in | **none — mandatory** | the animation period, in milliseconds |
| 5 | `time` | integer (milliseconds, 64-bit) | in | **none — mandatory** | "now", in milliseconds |

* **There are no optional arguments and no default values.** All five
  arguments are required.
* The member returns nothing (`void`); the upstream method's return type is
  `void` (`layerExRaster/main.cpp:26`).
* The member is an instance method: it is called on the destination layer, and
  the source layer is the first argument.

### 2.1 Argument types and coercion

The upstream's C++ signature is `(tTJSVariant, int, int, int, tjs_int64)`
(`layerExRaster/main.cpp:26`). The binding library converts each argument to
its declared type:

* `maxh`, `lines`, `cycle` are 32-bit integers; `time` is a 64-bit integer.
* The numeric coercion is TJS's integer coercion: `void` → `0`, an integer →
  itself, a real → truncated toward zero, a string → parsed as an integer (a
  non-numeric string → `0`). An **object or an octet argument raises** the
  VM's variant-conversion error ("Cannot convert the variable type (%1 to
  %2)"). A 64-bit value passed where a 32-bit integer is expected is
  **narrowed** to 32 bits (the low 32 bits, reinterpreted as signed).
* The first argument is taken as an *object* and is never coerced; the plugin
  reads four members from it.

### 2.2 Arity

The member is registered through the binding library's plain-method
registration, which has a fixed arity of five:

* **Fewer than five arguments → the call raises.** The binding library's
  arity gate returns its "bad parameter count" error (`ncbind/ncbind.hpp:1281`),
  which the VM raises as the script error **"Invalid argument count"**.
* **Six or more arguments → the extra arguments are ignored.** Only the first
  five are read.

---

## 3. What the call reads

The upstream keeps a small cache of the destination layer's members and
refreshes it **before every invocation** (`layerExRaster/main.cpp:90-101`),
so a change to the destination's size or clip between two calls is picked up.

### 3.1 From the destination layer (the receiver)

Read through the shared base class (`layerExImage/layerExBase.hpp:76-89`
declares the member list, `:108-117` reads them):

| member | meaning | used for |
|---|---|---|
| `imageWidth` | the width of the layer's **image** (its main image buffer), in pixels | the size guard and the wave's vertical scale |
| `imageHeight` | the height of the layer's **image**, in pixels | the size guard and the wave's vertical scale |
| `mainImageBufferForWrite` | the address of the first pixel of the main image | the destination pixels; asking for it *for write* is what marks the image as modified |
| `mainImageBufferPitch` | the row pitch of the main image, **in bytes** | the row stride |
| `clipLeft` | the left edge of the clip rectangle, in **image** coordinates | the horizontal origin of the copy |
| `clipTop` | the top edge of the clip rectangle, in **image** coordinates | the vertical origin of the copy and the wave's phase |
| `clipWidth` | the width of the clip rectangle, in pixels | how much of each row is copied |
| `clipHeight` | the height of the clip rectangle, in pixels | how many rows are copied |

The four clip members are the layer's *effective* clip rectangle: when the
layer has no clip set, they describe the whole image.

### 3.2 From the source layer (argument 1)

Read directly by the member (`layerExRaster/main.cpp:32-42`):

| member | meaning |
|---|---|
| `imageWidth` | the width of the source's image, in pixels |
| `imageHeight` | the height of the source's image, in pixels |
| `mainImageBuffer` | the address of the first pixel of the source's main image (read-only) |
| `mainImageBufferPitch` | the source's row pitch, in bytes |

**The source's own clip rectangle is never read.** The destination's clip
rectangle is applied to the source as well: the source is read at exactly the
same image coordinates the destination is written at.

### 3.3 Coordinate spaces

* Everything is in **image coordinates** of the two layers: row 0 is the first
  row of the main image, column 0 is the first pixel of the main image. The
  layer's *visible* size (`Layer.width` / `Layer.height`) and the image's
  origin offset (`imageLeft` / `imageTop`) play **no role** in this member.
* The source and the destination are read and written at the same image
  coordinates; the two layers need not share an origin, only a size.

---

## 4. The behaviour, step by step

### 4.1 The size guard

If the source's image size differs from the destination's image size — in
either dimension — the call **does nothing at all**: no pixel is touched, no
redraw is requested, no error is raised, and the member returns normally
(`layerExRaster/main.cpp:44-46`).

This is the upstream's manual's "the processing is not performed unless it is
the same size as itself" (`layerExRaster/manual.tjs:12`).

### 4.2 The wave

Three of the arguments describe a horizontal shear that varies from row to
row; the copy moves every row sideways by a different amount, which is what
gives the effect its "raster" look.

* **Angular step** — `2π / lines`, in radians per image row. One full wave
  therefore spans `lines` rows of the image
  (`layerExRaster/main.cpp:49`).
* **Amplitude** — the horizontal displacement never exceeds `maxh` pixels
  (`layerExRaster/main.cpp:53`). The upstream contains a commented-out
  alternative in which the amplitude would also be modulated by `time`
  (`layerExRaster/main.cpp:51-52`); **that modulation is not active** — in
  the shipped behaviour only the *phase* moves with time, the amplitude is
  always `maxh`.
* **Phase** — a starting angle that depends on `time` and `cycle`
  (`layerExRaster/main.cpp:56`), plus the clip's top row
  (`layerExRaster/main.cpp:59`).

The displacement of a row is therefore

    displacement(row) = trunc( maxh · sin( phase ) )

where `trunc` is truncation **toward zero** (not floor) and `phase` is the
row's angle, defined exactly in §5.

A **negative** `maxh` is meaningful: it inverts the wave, because the
displacement is the product of `maxh` and the sine.

### 4.3 The copy

For every row of the destination's clip rectangle, in increasing row order:

* Let `d` be that row's displacement (§4.2).
* If `d ≥ 0`: the row's content is copied **rightwards by `d` pixels** — the
  first `clipWidth − d` pixels of the source row are written to the
  destination row starting `d` pixels to the right of the clip's left edge.
  The last `d` pixels of the destination row are **not written** and keep
  whatever they held before.
* If `d < 0`: the row's content is copied **leftwards by `|d|` pixels** — the
  last `clipWidth − |d|` pixels of the source row are written to the
  destination row starting at the clip's left edge. The first `|d|` pixels of
  the destination row are **not written** and keep whatever they held before.
* If `|d| ≥ clipWidth`, **nothing** is copied for that row (the number of
  pixels to copy is zero or negative).
* If `clipWidth` is zero, nothing is copied for any row.

The pixels are moved **verbatim**: the whole 32-bit pixel word is copied, with
no blending, no alpha arithmetic, no colour conversion and no clamping. A
transparent source pixel overwrites an opaque destination pixel with a
transparent one.

The rows are processed in increasing order and each row is copied from the
source's own row — the source is never modified, and the copy of one row
never depends on another row.

### 4.4 The redraw

After the copy, the destination layer is asked to repaint itself, passing the
clip rectangle's four values (`clipLeft`, `clipTop`, `clipWidth`,
`clipHeight`) to the layer's `update` member
(`layerExImage/layerExBase.hpp:99-103`, invoked at
`layerExRaster/main.cpp:84`).

The redraw request happens **whenever the size guard passed**, even if the
clip rectangle is empty and not a single pixel was copied. It does **not**
happen when the size guard rejected the call (§4.1).

---

## 5. The exact arithmetic

Everything below is IEEE-754 double arithmetic unless stated otherwise.

**The inputs**

* `H` — the image height in pixels (the source's and the destination's are
  equal at this point, because of the size guard).
* `CL`, `CT`, `CW`, `CH` — the destination's clip rectangle: left, top, width,
  height, in image coordinates and in pixels.
* `A` — `maxh`, the amplitude in pixels (an integer).
* `L` — `lines`, the rows per wave period (an integer).
* `P` — `cycle`, the period in milliseconds (an integer).
* `T` — `time`, the current time in milliseconds (a 64-bit integer).

**The derived values** (in the upstream's operation order)

    angular_step = 2π / L
    half_height  = H / 2                       (integer division: H halved, truncated)
    phase0       = -angular_step · T / P · half_height
    phase0      += angular_step · CT

Notes on the arithmetic:

* `half_height` is an **integer** division: for an odd `H` it is `⌊H/2⌋`.
* `phase0` is computed as `((-angular_step · T) / P) · half_height`: the
  division by `P` is a **real** division (the dividend is already a double),
  and `T` is converted to a double, so a `T` beyond 2⁵³ loses precision.
* `2π` is the double closest to 2π.
* The sine is the platform's double-precision `sin`.

**The per-row displacement**, for the clip row `n` (`0 ≤ n < CH`, whose
absolute image row is `r = CT + n`):

    phase(n) = phase0 + n · angular_step
    d(n)     = trunc( sin(phase(n)) · A )

* `trunc` truncates **toward zero**: `−4.7 → −4`, `4.7 → 4`. (Rounding to
  nearest, or flooring, is *not* the upstream's behaviour.)
* The upstream accumulates the phase by adding the angular step once per row
  rather than computing `phase0 + n · angular_step`; a bit-exact port should
  accumulate the same way.
* `d(n)` is an integer with `|d(n)| ≤ |A|`.
* The number of pixels to copy is computed in 32-bit signed arithmetic
  upstream; a port should compute it in a wider type and treat a
  non-positive count as "copy nothing".

**The copy ranges**, in image coordinates, for the clip row `n`:

| case | source pixels read | destination pixels written |
|---|---|---|
| `d ≥ 0` and `d < CW` | columns `CL … CL + (CW − d) − 1` of row `CT + n` | columns `CL + d … CL + CW − 1` of row `CT + n` |
| `d < 0` and `|d| < CW` | columns `CL + |d| … CL + CW − 1` of row `CT + n` | columns `CL … CL + CW + d − 1` of row `CT + n` |
| `|d| ≥ CW` | none | none |

Every destination pixel that is not in the "written" column range keeps its
previous content.

**The pixel addresses** — the copy moves whole 32-bit pixels, so the source
and the destination may have different pitches:

    the row r of a buffer whose first pixel is at address `b` and whose pitch
    is `p` starts at  b + (r − CT)·p + CL·4

and the pixel at column `c` of that row is at offset `+ 4·c` from the row
start. The destination's origin is the address of its main image advanced by
`pitch · CT + 4·CL`; the source's origin is the address of its main image
advanced by the same expression **with the source's own pitch**
(`layerExRaster/main.cpp:59-61`).

### 5.1 A worked example

Destination and source are both 64 × 32 images, the clip is
`(0, 0, 64, 32)`, `maxh = 8`, `lines = 16`, `cycle = 1000`, `time = 0`.

* `angular_step = 2π/16 = 0.3926990817`
* `half_height = 32/2 = 16`, `phase0 = −0.3926990817 · 0 / 1000 · 16 = 0`,
  and `phase0 += 0.3926990817 · 0 = 0`.

| row `n` | `phase(n)` | `sin·8` | `d` | pixels written | destination columns |
|---|---|---|---|---|---|
| 0 | 0.000000 | 0.000000 | 0 | 64 | 0 … 63 (the row is copied verbatim) |
| 1 | 0.392699 | 3.061467 | 3 | 61 | 3 … 63 (columns 61…63 keep their old content) |
| 2 | 0.785398 | 5.656854 | 5 | 59 | 5 … 63 |
| 3 | 1.178097 | 7.391036 | 7 | 57 | 7 … 63 |
| 4 | 1.570796 | 8.000000 | 8 | 56 | 8 … 63 |
| 8 | 3.141593 | 0.000000 | 0 | 64 | 0 … 63 |

Note row 8: `sin(π)` is a tiny positive number, and the truncation turns it
into `0`, so the row is copied verbatim.

With `maxh = −8` the same rows produce `d = 0`, `−3`, `−5`, `−7`, `−8`, and
the copy runs leftwards: for `d = −3` the source columns 3…63 are written to
the destination columns 0…60, and the destination columns 0…2 keep their old
content.

---

## 6. Pixel format

* The main image is a **32-bit-per-pixel** buffer with a row pitch given in
  bytes (`mainImageBufferPitch`); the upstream moves whole 32-bit words and
  assumes four bytes per pixel when it offsets into a row
  (`layerExRaster/main.cpp:60-61`, `:69-80`).
* The copy is **verbatim**: the destination pixel ends up with exactly the
  source pixel's colour *and* alpha, in whatever channel order and alpha
  convention the engine's main image uses. There is no premultiplication, no
  straight/premultiplied conversion and no blending. A port must therefore
  copy the four bytes of a pixel as they are, in the engine's own main-image
  representation, and must not reinterpret the channels.
* **The province plane (the hit-test mask) is not touched.** The member writes
  only the main image; the province keeps whatever it had, so after a call the
  province no longer matches the picture. This is the upstream's behaviour and
  a game that relies on hit testing has to live with it.
* The layer's *visible* size (`Layer.width` / `Layer.height`) is irrelevant:
  the member works on the **image**, which may be larger than the visible
  rectangle.
* The destination's image is marked as modified by the write (the upstream
  asks the host for the buffer "for write"), so the layer repaints.

---

## 7. Edge cases

| situation | upstream behaviour |
|---|---|
| source image size ≠ destination image size (either dimension) | **nothing happens**: no pixels written, no redraw requested, no error. The member returns normally. (`layerExRaster/main.cpp:44-46`) |
| source and destination have the same size but the clip is empty (`CH = 0` or `CW = 0`) | nothing is copied; the redraw is still requested. (`layerExRaster/main.cpp:65`, `:84`) |
| the destination has no image at all (image size 0 × 0) | the size guard only passes when the source is *also* 0 × 0; then the clip is empty and nothing is copied. The redraw is still requested. |
| the source is a `Layer` with no image (the host reports its image size as 0 × 0 — §10.3) | the size guard rejects the call (unless the destination is 0 × 0 too): nothing happens, no error. |
| the source is an object that is **not** a `Layer` (or lacks the four members) | the four member reads fail silently and yield `0`, so the size guard rejects the call: **nothing happens, no error**. (Only if the destination's image is 0 × 0 does the call proceed, and then it copies nothing.) |
| the source is **not an object** (a number, a string, `void`, …) | the upstream dereferences a null pointer: **undefined behaviour — a crash**. The port must not reproduce this; see §9. (`layerExRaster/main.cpp:32-34`) |
| `maxh = 0` | every row's displacement is `0`; every clip row is copied verbatim from the source. |
| `maxh < 0` | the wave is inverted: the rows are copied leftwards instead of rightwards (§4.2). |
| `|maxh| ≥ clipWidth` | for every row, `|d| ≥ CW`, so **nothing is copied**; the redraw is still requested. |
| `lines = 0` or `cycle = 0` | the angular step or the phase becomes infinite, the sine becomes NaN, and the conversion of NaN to an integer is undefined behaviour. In practice nothing is copied and the redraw is still requested, but the outcome is not defined by the upstream; see §9. |
| `time` is very large | the phase loses precision (the time is converted to a double); the wave's position becomes inaccurate but nothing breaks. |
| `maxh`, `lines` or `cycle` is outside the 32-bit range | the argument is narrowed to 32 bits before use. |
| the source and the destination are the same layer | the copy is well defined: each row is read from the source row and written to the same row, and within a row the destination index is never below the source index, so the copy does not disturb the pixels it has yet to read. |
| the source's pitch differs from the destination's pitch | handled: each buffer is walked with its own pitch. |

---

## 8. Error behaviour — a summary

| condition | upstream |
|---|---|
| fewer than five arguments | the VM raises **"Invalid argument count"** (the binding library's arity gate) |
| an object or octet passed where an integer is expected | the VM raises the variant-conversion error **"Cannot convert the variable type (%1 to %2)"** |
| the source argument is not an object | **undefined behaviour** (a null-pointer dereference) — no script error is raised |
| the source argument is an object that is not a `Layer` | **no error**: the call silently does nothing |
| the source image size differs from the destination's | **no error**: the call silently does nothing |
| anything else | **no error**: the member never raises on its own account |

The member itself raises **nothing**: it has no validation of its arguments, no
range checks and no error paths at all. Every failure it can meet is either a
silent no-op or undefined behaviour.

---

## 9. What the upstream leaves to the host (decisions the port must make)

The upstream is a thin C++ shim over the host engine and leaves several things
undetermined. A port must decide each of them; the choices below are
recommendations, not upstream facts.

1. **A source argument that is not an object.** The upstream crashes. The port
   should raise a typed script error instead (the tree's convention for the
   sibling member `Layer.perspectiveCopy` is the same: report a typed error
   where the reference crashes).
2. **A source argument that is an object but not a `Layer`.** The upstream does
   nothing, silently. The port may either mirror that (do nothing, no error)
   or raise a typed error. Mirroring it keeps the observable behaviour; raising
   is friendlier and matches the tree's "no silent fallback" rule. This is a
   genuine choice and it should be made deliberately and recorded.
3. **`lines ≤ 0` or `cycle ≤ 0`.** The upstream computes an infinite angular
   step or an infinite phase and then converts NaN to an integer — undefined
   behaviour. The port should reject these with a typed error rather than
   reproduce an undefined result.
4. **The channel order and the alpha convention of the main image.** The
   upstream is agnostic: it copies 32-bit words. The port must use the
   engine's own main-image representation and copy the four bytes of a pixel
   unchanged.
5. **The redraw request.** The upstream calls the layer's `update` with the
   clip rectangle. In this engine `Layer.update` ignores its arguments and
   marks the whole layer for repaint, so marking the layer dirty is the
   equivalent.
6. **The update is requested even when nothing was copied** (as long as the
   size guard passed). A port that skips the redraw when the copy was empty
   would differ from the upstream in an observable way (the layer's paint
   event).

---

## 10. Build system, file layout, and other open questions

### 10.1 The upstream's build system

* **CMake**, with the shared helper `krkrz.cmake` from the sibling
  `tp_stub` repository (`layerExRaster/CMakeLists.txt:1-8`; the helper is
  `tp_stub/krkrz.cmake`). The helper expects three sibling directories next
  to the plugin: `tp_stub`, `ncbind` and `simplebinder`.
* The plugin is built as a shared library (or as a static plugin when the host
  is built statically) from **one** source file, `main.cpp`, plus the shared
  `ncbind` runtime.
* The ncbind runtime and the tp_stub shim provide the host's ABI; the plugin
  links against the host executable's exported symbols at load time.
* The plugin has **no dependencies** beyond ncbind: no image library, no
  device feature, no fixture, no data file. Its only inputs are the two
  layers' pixel buffers.
* The plugin is compiled as C++ by the host's own CMake tree; the plugin's
  own `CMakeLists.txt` sets no standard of its own, and `tp_stub/krkrz.cmake`
  does not either — the standard comes from the host's tree.

### 10.2 Where each member lives

| member | upstream file |
|---|---|
| `Layer.copyRaster` | `layerExRaster/main.cpp:26-85` (the body), registered at `layerExRaster/main.cpp:109-111` |
| the clip rectangle, the per-call refresh of the destination's state | `layerExImage/layerExBase.hpp:76-89`, `:108-117` |
| the redraw request | `layerExImage/layerExBase.hpp:99-103` |

### 10.3 Things the specification does not settle

* **The host's `Layer` member semantics.** The plugin reads `imageWidth`,
  `imageHeight`, `mainImageBuffer`, `mainImageBufferForWrite`,
  `mainImageBufferPitch`, `clipLeft`, `clipTop`, `clipWidth`, `clipHeight` and
  `update` from the host's `Layer` class. Their exact definitions (what
  "image size" means for a layer without an image, how the clip rectangle is
  reported when none is set, whether the clip is in image or layer
  coordinates) are the **host engine's** contract, not this plugin's; the port
  must take them from the engine it is porting into.
* **The pixel layout of the main image** (channel order, straight versus
  premultiplied alpha) — the host engine's contract; see §9.4.
* **The `update` arguments' coordinate space.** The upstream passes image
  coordinates (the clip rectangle) to a member whose parameters are, in the
  host, layer coordinates. The upstream does not care; the port should use
  whatever its own `Layer.update` does.
* **The build artifacts.** The upstream ships no prebuilt DLL and no test
  fixture; the behaviour test must be written against the specification above
  and cite the upstream lines.
