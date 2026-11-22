# `layerExLongExposure` — behavioural specification

A clean-room specification of the five members the upstream plugin
`layerExLongExposure` adds to the engine's own `Layer` class.

## 0. Provenance and citation convention

* **Upstream project:** <https://github.com/wtnbgo/layerExLongExposure>
* **Commit read:** `68f0df7fc6007a6cdd4b379e266cfdb4aa20a435`
  ("update for linux: src/plugins/layerExLongExposure", 2026-03-31).
* **Upstream source file:** `Main.cpp` at the repository root — the whole
  plugin is one file.
* **Citation form used below:** `upstream Main.cpp:<line>` — the line
  numbers of that file at that commit. `upstream manual.tjs:<line>` and
  `upstream startup.tjs:<line>` are the upstream's own pseudo-code manual
  and demo script.
* **Engine-side citations** are written `reference/krkr2/...` (the engine's
  read-only reference checkout) or `crates/...` (the engine's own tree).
  The upstream is the oracle for the five members; the reference checkout
  is the oracle for the engine facilities the upstream builds on (the
  `Layer` properties, the 32-bit pixel layout, the ncbind calling
  convention).

Note on the plugin table: the reference's own plugin table
(`reference/krkr2/docs/KRKR_PLUGINS.md`) lists the other `wtnbgo` layer
plugins (`layerExDraw`, `layerExImage`, `layerExSave`, `layerExAreaAverage`,
`layerExBTOA`, `layerExRaster`) but **not** `layerExLongExposure`. The
upstream URL used here was given with the task and follows the same
organisation and naming pattern as the table's entries.

## 1. What the plugin publishes

The upstream is a **ncbind plugin** that attaches to the engine's existing
`Layer` class; it publishes **no class of its own**, no global, no constant.
Linking it adds exactly five members to `Layer`:

| member | kind | arity |
|---|---|---|
| `initExposure` | instance method | 0 |
| `snapExposure` | instance method | 0 |
| `statExposure` | instance method | 0 (returns a dictionary) |
| `copyExposure` | instance method | 1 (required) |
| `termExposure` | instance method | 0 |

* The five members are registered as instance methods on the `Layer` class
  object, so every `Layer` (and every subclass of `Layer`) has them, and
  they only exist after the plugin is linked
  (`upstream Main.cpp:229-236`).
* The registration is an *attach* to an existing class: the upstream's
  internal class name (`LongExposure`, which is also the prefix of every
  error message) is never visible to a script
  (`upstream Main.cpp:229-236`; the attach mechanism is
  `reference/krkr2/cpp/core/plugin/ncbind.hpp:2235-2237`).
* The upstream's own manual documents exactly these five members and no
  other (`upstream manual.tjs:8-45`).
* The plugin's DLL name is the build target name `layerExLongExposure`
  (`upstream CMakeLists.txt:5-8`); the upstream's demo script links
  `bin/layerExLongExposure-d.dll` first and falls back to
  `bin/layerExLongExposure.dll` (`upstream startup.tjs:4-8`). The engine's
  built-in plugin table spells DLL names in the `*.dll` form
  (`layerExDraw.dll`), so the name to register is
  **`layerExLongExposure.dll`**.
* Until the plugin is linked, a script that calls one of the five members
  gets the engine's ordinary "no such member" error — the members are
  simply absent from the class.

## 2. The state a `Layer` carries

The plugin keeps **one accumulation buffer per `Layer` object** (not per
class, not global):

* It is created lazily, on the first call of any of the five members, and
  it belongs to that `Layer` object only
  (`upstream Main.cpp:219-228`).
* It holds, for every pixel of the layer's image, **four 32-bit unsigned
  counters** — one per colour channel and one for the alpha. In memory
  they are laid out per pixel in the order *blue, green, red, alpha*
  (`upstream Main.cpp:19-21`, `upstream Main.cpp:37-46`,
  `upstream Main.cpp:147-156`).
* The buffer also records the **image width and height** the buffer was
  created for (`upstream Main.cpp:13`, `upstream Main.cpp:180-182`).
* It is released when the `Layer` object is destroyed, without the script
  having to call `termExposure` (`upstream Main.cpp:12`,
  `upstream manual.tjs:39-44`).

Everything the five members read or write is the layer's **main image**
(the image of `imageWidth` × `imageHeight` pixels). The province image is
never read and never written.

## 3. Pixel format and coordinate spaces

* **The image** is the layer's main image: `imageWidth` × `imageHeight`
  pixels, 32 bits per pixel, 8 bits per channel. The plugin never looks at
  the layer's visible rectangle, at `imageLeft`/`imageTop`, or at the
  device; everything it does is in the main image's own pixel space, with
  the origin at the image's top-left corner.
* **The channel order in the upstream's buffer** is *blue, green, red,
  alpha* in memory: the upstream reads the four bytes of a pixel in that
  order and adds them to the four counters in that order
  (`upstream Main.cpp:41-44`), and it labels those counters blue, green,
  red, alpha when it computes the extremes and when it writes the result
  back (`upstream Main.cpp:79-82`, `upstream Main.cpp:151-154`). That is
  the engine's 32-bit ARGB layout: the bytes of a pixel in memory are
  `B, G, R, A` and the packed word is `0xAARRGGBB`
  (`reference/krkr2/cpp/core/visual/argb.h:23-40`).
* **The alpha is straight (not premultiplied)**: the upstream adds the
  stored alpha byte to the alpha counter like any other channel and never
  touches the colour channels when the alpha is small.
* **The row pitch** is the layer's `mainImageBufferPitch` — the number of
  bytes from one row of the image to the next
  (`upstream Main.cpp:39`, `upstream Main.cpp:149`,
  `reference/krkr2/cpp/core/visual/LayerIntf.cpp:3020-3024`). The upstream
  reads row *y* at *address + pitch × y*, so a row-padded image is handled
  correctly. The pitch is a **signed** value in the upstream; a negative
  pitch would make it walk the rows backwards. The engine's pitch is
  `imageWidth × 4` (`crates/engine/src/scene.rs:2088-2095`).
* **The engine's own buffer has the opposite byte order.** The engine's
  layer image is straight-alpha **RGBA8** (`crates/engine/src/scene.rs:386-394`):
  byte 0 is red, byte 1 green, byte 2 blue, byte 3 alpha. A port must map
  the channels by name, not by position: the engine's byte 0 feeds the
  *red* counter, byte 1 the *green*, byte 2 the *blue*, byte 3 the
  *alpha*. The counter order inside the port's own buffer is the port's
  choice; only the *labelling* is observable (through `statExposure`'s keys).

## 4. The normalisation arithmetic

This is the arithmetic `copyExposure` uses, and it is shared by the
upstream's four per-channel helpers (`upstream Main.cpp:59-73`).

Let `n` be the accumulated counter of one channel of one pixel, and let
`(lo, hi)` be that channel's pair of extremes. `n`, `lo` and `hi` are
**unsigned 32-bit** values. The output byte of that channel is:

* **255** when `lo >= hi`;
* otherwise: take `n' = min(max(n, lo), hi)` — the counter is *clamped*
  into `[lo, hi]` — and the output byte is
  `floor((n' - lo) × 255 / (hi - lo))`, computed in **unsigned 32-bit**
  arithmetic with the division truncating toward zero.

The product `(n' - lo) × 255` is evaluated in 32-bit unsigned arithmetic
and therefore **wraps modulo 2^32**; the quotient is then truncated to 8
bits. For every accumulation below 2^32 the formula above is the exact
result; above it, the upstream's result is the wrapped one. The port must
decide whether to reproduce the wrap or to compute in a wider type (see
§8).

The `lo >= hi` case is not an error and not a black image: it yields
**255** for every channel, i.e. an opaque white image. The upstream's
manual states this rule (`upstream manual.tjs:31-36`).

The same pair `(lo, hi)` is used for **all four channels independently**:
the red channel is stretched by the red extremes, the green by the green
ones, and so on.

## 5. The members

### 5.1 `Layer.initExposure()`

**Signature:** an instance method of `Layer`; no arguments, no return
value. Registered at `upstream Main.cpp:231`; implemented at
`upstream Main.cpp:15-22`.

**Semantics:**

1. Any accumulation state the layer already carries is discarded first: the
   buffer is released and the recorded size becomes 0 × 0
   (`upstream Main.cpp:16`).
2. The layer's `hasImage`, `imageWidth` and `imageHeight` are read
   (`upstream Main.cpp:17`, `upstream Main.cpp:171-183`). If `hasImage` is
   false, or `imageWidth <= 0`, or `imageHeight <= 0`, the member raises
   **`LongExposure.init: invalid layer image`** (`upstream Main.cpp:17-18`).
   The state is already gone at that point: the release of step 1 happens
   before the size is read, so a failed initialisation leaves the layer
   with no accumulation at all.
3. Otherwise a zeroed accumulation buffer is created for
   `imageWidth × imageHeight` pixels, holding four 32-bit unsigned counters
   per pixel, and the width and height are recorded
   (`upstream Main.cpp:19-21`).

**Edge cases:**

* Calling it twice is legal and **restarts** the accumulation: the second
  call throws away the first buffer and re-reads the size
  (`upstream Main.cpp:16`).
* The size that is recorded is the **image** size, not the visible
  rectangle's size.
* A layer with no image, or with an image of zero width or height, is
  refused (the error above).
* The buffer's counters start at zero, so a freshly initialised layer
  reports `min = max = 0` for every channel.

### 5.2 `Layer.snapExposure()`

**Signature:** an instance method of `Layer`; no arguments, no return
value. Registered at `upstream Main.cpp:232`; implemented at
`upstream Main.cpp:24-47`.

**Semantics:** the layer's current image is **added** to the accumulation.

1. If the layer carries no accumulation buffer, the member raises
   **`LongExposure.snap: not initialized`** (`upstream Main.cpp:25-26`).
2. `hasImage`, `imageWidth` and `imageHeight` are read again. If the layer
   has no image, or its image size is not positive, the member raises
   **`LongExposure.snap: invalid layer image`** (`upstream Main.cpp:28-29`).
3. If the current image size differs from the recorded one, the member
   raises **`LongExposure.snap: invalid layer size`** (`upstream Main.cpp:30-31`).
4. The layer's `mainImageBufferPitch` and `mainImageBuffer` are read
   (`upstream Main.cpp:33-35`, `upstream Main.cpp:184-194`).
5. For every pixel of the image and for every channel, the channel's byte
   value (0…255) is **added** to that pixel's counter for that channel
   (`upstream Main.cpp:37-46`). The counters are unsigned 32-bit and wrap
   modulo 2^32.

In formula form: with `A_c(x, y)` the counter of channel `c` at pixel
`(x, y)` and `P_c(x, y)` the channel's byte value in the image, the counter
becomes `A_c(x, y) + P_c(x, y)`, for every channel `c` and every pixel.

**Edge cases:**

* A snap after the layer's image was **resized** raises "invalid layer
  size"; the accumulation is left untouched (the size check precedes the
  addition).
* A snap after the layer's image was **released** raises "invalid layer
  image".
* The **province image is ignored**; a fully transparent main image
  contributes zeros to all four counters.
* There is no limit on the number of snaps. The counters wrap after about
  16.8 million snaps of a channel at 255 (§4, §8).
* The upstream does **not** check whether the image address it obtained is
  usable before it reads through it (`upstream Main.cpp:35` — the result of
  the read is discarded). See §8.

### 5.3 `Layer.statExposure()`

**Signature:** an instance method of `Layer`; no arguments; **returns a
dictionary**. Registered at `upstream Main.cpp:233`; implemented at
`upstream Main.cpp:87-109`.

**Semantics:**

1. If the layer carries no accumulation buffer, the member raises
   **`LongExposure.stat: not initialized`** (`upstream Main.cpp:88-89`).
2. The minimum and the maximum of each of the four counters are taken over
   **every pixel of the accumulation buffer** (`upstream Main.cpp:75-86`).
3. The result is a dictionary with **eight integer members**
   (`upstream Main.cpp:95-107`):

   | key | meaning |
   |---|---|
   | `min_r` | the smallest accumulated red value over all pixels |
   | `min_g` | the smallest accumulated green value over all pixels |
   | `min_b` | the smallest accumulated blue value over all pixels |
   | `min_a` | the smallest accumulated alpha value over all pixels |
   | `max_r` | the largest accumulated red value over all pixels |
   | `max_g` | the largest accumulated green value over all pixels |
   | `max_b` | the largest accumulated blue value over all pixels |
   | `max_a` | the largest accumulated alpha value over all pixels |

**Edge cases:**

* The values are the **accumulated sums**, not the pixel values: after *N*
  snaps of one unchanged image they are *N* times the pixel values.
* The values are **non-negative integers** (the counters are unsigned; they
  are handed to the dictionary as 64-bit integers).
* A layer that was initialised but never snapped reports `min = max = 0`
  for all four channels.
* The layer's **current** size is not consulted: the member answers from
  the buffer even if the layer's image was resized after the
  initialisation.
* The upstream's own manual lists the members in the order
  `min_r, max_r, min_g, max_g, min_b, max_b, min_a, max_a`
  (`upstream manual.tjs:22-29`); the dictionary itself has no order.

### 5.4 `Layer.copyExposure(stat)`

**Signature:** an instance method of `Layer`; **exactly one argument is
required**; no return value. Registered at `upstream Main.cpp:234`;
implemented at `upstream Main.cpp:111-157`.

**Arity:** the upstream's function takes one parameter, so the calling
convention counts one argument: a call with **no** argument is rejected
before the member runs, with the engine's parameter-count error — the
reference renders it as "Invalid argument count"
(`reference/krkr2/cpp/core/tjs2/tjsError.h:268`), and the engine's own
native members answer a typed error naming the member and the count it
needs (`crates/compat/src/natives/plugins/layer_ex/geom.rs:94`). Extra
arguments beyond the first are ignored. The count check lives in the
calling convention, not in the plugin
(`reference/krkr2/cpp/core/plugin/ncbind.hpp:1176-1178`; the argument
count is the parameter count of the function type,
`reference/krkr2/cpp/core/plugin/ncb_invoke.hpp:157`).

**Semantics:** the accumulation is turned into an image and written into
the layer's **own main image**, replacing it.

1. If the layer carries no accumulation buffer, the member raises
   **`LongExposure.copy: not initialized`** (`upstream Main.cpp:112-113`).
2. `hasImage`, `imageWidth` and `imageHeight` are read. If the layer has
   no image, or its image size is not positive, the member raises
   **`LongExposure.copy: invalid layer image`** (`upstream Main.cpp:114-116`).
3. If the current image size differs from the recorded one, the member
   raises **`LongExposure.copy: invalid layer size`**
   (`upstream Main.cpp:117-118`).
4. The layer's `mainImageBufferPitch` and `mainImageBufferForWrite` are
   read. If either of them is zero — no image, or no writable buffer — the
   member raises **`LongExposure.copy: invalid layer image`**
   (`upstream Main.cpp:120-123`, `upstream Main.cpp:195-205`).
5. The per-channel `(lo, hi)` pairs are chosen from the argument
   (`upstream Main.cpp:125-146`):
   * **the argument is `void`** — the statistics are computed from the
     accumulation itself and used as the pairs
     (`upstream Main.cpp:127-130`); this is the "implicit `statExposure`"
     the upstream's manual describes (`upstream manual.tjs:31-36`);
   * **the argument is an object** — the eight members `min_r`, `min_g`,
     `min_b`, `min_a`, `max_r`, `max_g`, `max_b`, `max_a` are read from it
     (`upstream Main.cpp:131-145`). Each member is read as an integer; a
     member the object does not carry reads as **0**
     (`reference/krkr2/cpp/core/plugin/ncbind.hpp:689-695`,
     `reference/krkr2/cpp/core/plugin/ncbind.hpp:728-733`). The values are
     used as **unsigned 32-bit** numbers: a negative value becomes the
     corresponding large unsigned value (e.g. `-1` is 4294967295), and a
     value beyond 2^32 is truncated to its low 32 bits;
   * **the argument is anything else** — an integer, a real, a string, an
     octet — or an object with no usable members (this includes `null`):
     **no pair is taken from the argument at all**, and the pairs stay at
     their built-in default, which is `lo = 4294967295` and `hi = 0` for
     every channel (`upstream Main.cpp:54`, `upstream Main.cpp:126-146`).
     Because `lo >= hi`, every output byte is then **255** and the layer
     becomes an opaque white image.
6. Every pixel of the layer's main image is replaced by the normalised
   value of its four counters, using the arithmetic of §4: the blue byte
   from the blue counter, the green byte from the green counter, the red
   byte from the red counter, the alpha byte from the alpha counter
   (`upstream Main.cpp:147-156`).

**Edge cases:**

* The write goes into the **main image**, never into the province image,
  and the layer's image size is unchanged.
* Reading the writable buffer marks the image as modified upstream
  (`reference/krkr2/cpp/core/visual/LayerIntf.cpp:3011-3017`).
* A dictionary whose members are all zero, or whose `min` is not smaller
  than its `max`, produces an **all-255** image (§4).
* A dictionary that carries only some of the eight members: the missing
  ones read as 0, so the corresponding channel usually ends up all-255.
* A member whose value is an object or an octet cannot be converted to an
  integer and **raises the engine's variant-conversion error** at the
  moment it is read (`reference/krkr2/cpp/core/tjs2/tjsVariant.h:862-878`).
* The argument may be any object — a dictionary, a plain object with those
  eight members, or an array (an array has no such named members, so all
  eight read as 0).

### 5.5 `Layer.termExposure()`

**Signature:** an instance method of `Layer`; no arguments, no return
value. Registered at `upstream Main.cpp:235`; implemented at
`upstream Main.cpp:159-163`.

**Semantics:** the accumulation buffer is released and the recorded size
is forgotten. Calling it when there is nothing to release is **not** an
error.

After it, `snapExposure`, `statExposure` and `copyExposure` raise their
"not initialized" error, and `initExposure` starts a fresh accumulation.
The buffer is also released when the `Layer` object is destroyed, without
the script calling this member (`upstream Main.cpp:12`,
`upstream manual.tjs:39-44`).

## 6. The error catalogue

Every error below is a script exception whose message is the quoted text,
raised with the engine's "throw exception message" facility
(`reference/krkr2/cpp/core/base/MsgIntf.h:42`). The messages name the
plugin's internal class, not a script-visible class.

| member | condition | message |
|---|---|---|
| `initExposure` | the layer has no image, or its image size is not positive | `LongExposure.init: invalid layer image` |
| `snapExposure` | no accumulation buffer | `LongExposure.snap: not initialized` |
| `snapExposure` | the layer has no image, or its image size is not positive | `LongExposure.snap: invalid layer image` |
| `snapExposure` | the image size changed since the initialisation | `LongExposure.snap: invalid layer size` |
| `statExposure` | no accumulation buffer | `LongExposure.stat: not initialized` |
| `copyExposure` | no accumulation buffer | `LongExposure.copy: not initialized` |
| `copyExposure` | the layer has no image, or its image size is not positive | `LongExposure.copy: invalid layer image` |
| `copyExposure` | the image size changed since the initialisation | `LongExposure.copy: invalid layer size` |
| `copyExposure` | no writable image buffer | `LongExposure.copy: invalid layer image` |
| `copyExposure` | called with no argument | the engine's parameter-count error (not the plugin's) — the reference renders it "Invalid argument count" |

Two more messages exist in the upstream but cannot be reached: the
"invalid layer image" of `statExposure` and the "stat failed" of
`copyExposure` sit behind a scan that always succeeds
(`upstream Main.cpp:75-86`, `upstream Main.cpp:92-93`,
`upstream Main.cpp:128-129`).

**Nothing is silently ignored except** the argument's *type* in
`copyExposure` (a non-object argument is accepted and leaves the ranges at
their defaults, §5.4) and the **extra arguments** of a `copyExposure`
call with more than one argument.

## 7. Edge cases, collected

| situation | result |
|---|---|
| `initExposure` on a layer with no image | raises "LongExposure.init: invalid layer image" |
| `initExposure` on a layer whose image is 0 pixels wide or high | raises "LongExposure.init: invalid layer image" |
| `initExposure` twice | the second call discards the accumulation and re-reads the image size |
| `snapExposure` / `statExposure` / `copyExposure` before `initExposure` | raises "…: not initialized" |
| `snapExposure` after the image was resized | raises "LongExposure.snap: invalid layer size"; the accumulation is untouched |
| `copyExposure` after the image was resized | raises "LongExposure.copy: invalid layer size" |
| the image was released after the initialisation | `snapExposure` / `copyExposure` raise "…: invalid layer image" |
| `copyExposure` with no argument | the engine's parameter-count error, before any check of the member's own state |
| `copyExposure(void)` | the statistics are computed internally and used as the ranges |
| `copyExposure(<dictionary with all eight members>)` | the given ranges are used, as unsigned 32-bit numbers |
| `copyExposure(<dictionary missing some members>)` | the missing members read as 0 |
| `copyExposure(<dictionary with min >= max>)` | every byte is 255 — an opaque white image |
| `copyExposure(<integer, real, string or octet>)` | every byte is 255 — an opaque white image |
| `copyExposure(null)` | every byte is 255 — an opaque white image |
| `copyExposure(<dictionary whose member is an object or an octet>)` | raises the engine's variant-conversion error |
| `termExposure` twice, or without a prior `initExposure` | no error |
| `termExposure` then `statExposure` | raises "LongExposure.stat: not initialized" |
| a **fully transparent** source image | the counters stay at 0; with one snap the ranges are `0…0`, so `lo >= hi` and the output is all-255 |
| a **zero-size** source image | impossible to reach: the initialisation refuses an image of zero width or height |
| a source image with a **uniform** colour in one channel | that channel's range is `lo == hi`, so the channel is written as 255 |
| the layer's **province** image | never read, never written |

## 8. What a port has to decide for itself

These are the points where the upstream's behaviour depends on the engine
it was built for, or where the upstream has a defect the engine should not
copy. The specification above states the upstream's behaviour; each item
below is a decision the writer has to make and to record.

1. **Channel order.** The upstream's image bytes are `B, G, R, A`
   (`reference/krkr2/cpp/core/visual/argb.h:23-40`); the engine's layer
   image bytes are `R, G, B, A` (`crates/engine/src/scene.rs:386-394`).
   The port must map the channels *by name*. Only the labelling of the
   `statExposure` keys is observable; the image the plugin writes is
   correct either way as long as each byte is normalised with its own
   channel's range.
2. **Reading the pixels.** The upstream goes through the raw address of
   the layer's main image (`mainImageBuffer` / `mainImageBufferForWrite`).
   The engine offers the same pixels through safe accessors
   (`crates/engine/src/scene.rs:2171-2192`,
   `Scene::with_main_image_pixels` and `Scene::with_main_image_pixels_mut`),
   which also mark the image dirty on write — the behaviour the upstream
   gets from asking for the writable buffer.
3. **A zero image address.** The upstream does not check the result of
   reading the image address in `snapExposure` (`upstream Main.cpp:35`), so
   a layer whose address is 0 would be dereferenced. In the engine the
   address is 0 when the image declares more pixels than its buffer holds
   (`crates/engine/src/scene.rs:2113-2141`). The port should raise rather
   than read through a null address, and should say so.
4. **The 32-bit wrap.** Both the counters and the scaling product wrap
   modulo 2^32 upstream (§4). A port with 64-bit counters agrees with the
   upstream for every accumulation below 2^32 and differs above it. The
   writer should pick one and record it as an approximation if it differs
   (the engine's rule for a knowingly-left divergence is D36).
5. **Where the per-object state lives.** The upstream keeps it in a native
   instance attached to the `Layer` object, created lazily and released with
   the object (`upstream Main.cpp:219-228`). The engine has to find its own
   home for it (the compatibility layer's per-object state).
6. **The error text.** The messages in §6 are the upstream's, verbatim.
   The engine's own convention is to raise with the same text, so a script
   that catches the error sees the upstream's message.
7. **The DLL name and the plugin table.** `layerExLongExposure.dll` is the
   name to register (§1). The upstream's demo also tries
   `layerExLongExposure-d.dll` (`upstream startup.tjs:4-8`); a debug-suffixed
   name is a build artefact of the upstream's CMake helper and is not part
   of the surface.
8. **A `statExposure` result.** The upstream returns a dictionary with
   eight integer members (§5.3). The engine's dictionary type must carry
   them.
9. **What `copyExposure` accepts.** The upstream accepts *any* object and
   reads the eight names from it (`upstream Main.cpp:131-145`); a port that
   restricted the argument to a dictionary would raise where the upstream
   accepts an array or a plain object.
10. **How `null` is treated.** In the upstream's script engine `null` is an
    object variant with no object behind it
    (`reference/krkr2/cpp/core/tjs2/tjsLex.cpp:1551-1553`), so it takes
    the "an object with no members" path: the ranges stay at their
    defaults and the result is an all-255 image (§5.4). The engine's script
    engine gives `null` a type of its own
    (`crates/tjs/src/lib.rs:60`, `VAL_NULL`), so the port has to route it
    to the same branch explicitly.
11. **Which properties are read.** The upstream reads `hasImage`,
    `imageWidth`, `imageHeight`, `mainImageBuffer`, `mainImageBufferPitch`
    and `mainImageBufferForWrite` through the global `Layer` class object,
    which it resolves once and caches (`upstream Main.cpp:206-214`,
    `upstream Main.cpp:170`). The port should read the layer's own
    properties; the only observable difference would be a script that
    replaced the global `Layer`, which no game does.

## 9. The upstream's build system and file map

* **Build system:** CMake, using the `krkrz` helper of the KiriKiri Z
  tree: the project includes `../tp_stub/krkrz.cmake` and declares one
  ncbind plugin target, `layerExLongExposure`, whose only source is
  `Main.cpp` (`upstream CMakeLists.txt:1-8`). The plugin is compiled
  against the engine's `ncbind.hpp`; it needs no engine source, no
  fixture, no image file and no device feature.
* **Files:**

  | file | what it holds |
  |---|---|
  | `Main.cpp` | the whole plugin: the five members, the per-object state, the registration |
  | `CMakeLists.txt` | the build |
  | `manual.tjs` | the upstream's pseudo-code manual of the five members (`upstream manual.tjs:1-45`) |
  | `startup.tjs` | a runnable demo: link, `setImageSize`, `initExposure`, four `loadImages` + `snapExposure` rounds, `statExposure`, `copyExposure(stat)`, save the result (`upstream startup.tjs:1-35`) |
  | `readme.txt` | the technique in Japanese: the sum buffer is created at the layer's current size, each snap adds the current image, and the copy clips with min/max and rounds to 0…255 |

* **Where each member lives:**

  | member | upstream lines |
  |---|---|
  | `initExposure` | `Main.cpp:15-22`, registered at `Main.cpp:231` |
  | `snapExposure` | `Main.cpp:24-47`, registered at `Main.cpp:232` |
  | `statExposure` | `Main.cpp:87-109`, registered at `Main.cpp:233` |
  | `copyExposure` | `Main.cpp:111-157`, registered at `Main.cpp:234` |
  | `termExposure` | `Main.cpp:159-163`, registered at `Main.cpp:235` |
  | the registration block | `Main.cpp:229-236` |
  | the per-object state hook | `Main.cpp:219-228` |
  | the image-size probe | `Main.cpp:171-183` |
  | the read-only image access | `Main.cpp:184-194` |
  | the writable image access | `Main.cpp:195-205` |
  | the property read helper | `Main.cpp:206-214` |
  | the extremes holder and the normaliser | `Main.cpp:49-74` |
  | the extremes scan | `Main.cpp:75-86` |

## 10. What this specification does not settle

* **The engine's own conventions** — how a plugin module is laid out, how a
  member is registered on an existing class, how a native member reports an
  error — are the tree's business (`AGENTS.md`,
  `docs/rules/code_styles.md`, the existing ported plugins under
  `crates/compat/src/natives/plugins/`). This document only says what the
  five members *do*.
* **The image format of the result** is not the plugin's business: the
  plugin only writes pixels into the layer's main image.
* **Whether the plugin is a startup module or a link-time module**: the
  upstream is linked explicitly by the script
  (`upstream startup.tjs:4-8`); the engine's table decides where it sits.
* **The upstream's demo writes a BMP** (`upstream startup.tjs:35`); that
  is the engine's `saveLayerImage`, not part of this plugin.
