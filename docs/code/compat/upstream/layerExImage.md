# `layerExImage` — `Layer.colorize`, `Layer.modulate`, `Layer.noise`, `Layer.generateWhiteNoise`

A behavioural specification for a clean-room port. It contains no upstream code:
every rule below is stated as behaviour, as an exact formula, or as a worked
result, and every rule carries the upstream `file:line` it was read from.

---

## 0. Provenance and scope

| item | value |
|---|---|
| upstream repository | `https://github.com/wtnbgo/layerExImage` |
| upstream commit read | `3aff625dba5c545ba722bb43b0a5ca1ad4f4336d` (2026-10-13) |
| upstream's public entry point | the shared plugin `layerExImage.dll` (ncbind's `V2Link` / `V2Unlink`) |
| binding library (a build dependency, not part of the plugin) | `ncbind`, mirror `wtnbgo/ncbind` @ `256502b817e6627c1491b916938448470df395fd` |
| host header stub (a build dependency) | `tp_stub`, mirror `wtnbgo/tp_stub` @ `81eb0b2509b9b1e3437df4ab28be754d6564a6bc` |

Files in the upstream tree that matter here:

| file | what it carries |
|---|---|
| `Main.cpp` | the registration: which class the members hang on, and their names |
| `LayerExImage.h` | the declaration of each member, with its parameter types |
| `LayerExImage.cpp` | the behaviour of every member |
| `layerExBase.hpp` | the pixel window every member shares (which pixels, which buffer, the repaint) |
| `manual.tjs` | the upstream's own manual (pseudo-code) — the parameter ranges it documents |

The four members in scope are `Layer.colorize`, `Layer.modulate`, `Layer.noise`
and `Layer.generateWhiteNoise`. The upstream's plugin registers six members on
`Layer`; the other two (`light`, `gaussianBlur`) are out of scope here and are
mentioned only in §8.

**The upstream is the oracle.** The engine's own reference checkout does not
carry these members, so a port's behaviour test cites the upstream file and
line given with each rule below.

---

## 1. How the members are registered

* The plugin attaches its native class to the **host's `Layer` class**, so the
  four members are *instance* members of `Layer`: a script calls
  `someLayer.colorize(...)`, `someLayer.modulate(...)`, `someLayer.noise(...)`
  and `someLayer.generateWhiteNoise()` (`Main.cpp:30-37`).
* The script-visible names are the upstream's C++ member names, spelled exactly
  `colorize`, `modulate`, `noise`, `generateWhiteNoise` (`Main.cpp:32-35`).
* The upstream's native class is an implementation detail: it is not a script
  class, it is not a global, and no script can name it (`Main.cpp:11-27`). It
  holds no state that the members read; the plugin refreshes its view of the
  layer at the start of every call (`Main.cpp:16-22`).
* **There are no overloads, no optional arguments and no defaults.** Every
  parameter is required; each member takes exactly the arguments listed in
  §4–§7 and no more (a call with *extra* arguments is accepted and the extra
  arguments are ignored — see §3).
* Every member returns **void** (`LayerExImage.h:36`, `:44`, `:50`, `:55`).
* The plugin reads **eight** members of the host's `Layer` class — `imageWidth`,
  `imageHeight`, `mainImageBufferForWrite`, `mainImageBufferPitch`,
  `clipLeft`, `clipTop`, `clipWidth`, `clipHeight` — and calls a **ninth**,
  `update` (`layerExBase.hpp:78-86`, `:99-103`). A host that lacks any of the
  eight read members cannot run the plugin at all (§3, "host requirements").

---

## 2. What the members operate on

* **The layer's own MainImage pixels.** The province (mask) plane is never
  read and never written by any of the four members.
* **The region is the layer's clip rectangle.** The plugin offsets the buffer
  by `clipTop` rows and `clipLeft` pixels and then walks `clipWidth` columns
  by `clipHeight` rows (`LayerExImage.cpp:22-30`). Nothing outside the clip
  rectangle is touched, and nothing inside it is skipped.
* **The row stride is the buffer's own pitch**, in bytes, as the host reports
  it (`mainImageBufferPitch`): the plugin advances to the next row by the
  pitch, so it never assumes the stride is four bytes per pixel
  (`layerExBase.hpp:81`, `:112`; `LayerExImage.cpp:222`, `:338`, `:380`).
* **The pixel layout in the host's buffer is four bytes per pixel, in the
  order B, G, R, A** (`LayerExImage.cpp:196-198` reads the blue byte first,
  the green byte second and the red byte third; `:217-220` writes them back in
  the same order; `:329-336` and `:376-378` do the same for the other two
  members).
* **The alpha byte is never part of any computation and is never written.**
  Every member preserves the alpha of every pixel bit-exactly
  (`LayerExImage.cpp:220`, `:336`, `:360`, `:378`). The colour channels of a
  fully transparent pixel are transformed exactly like those of an opaque one:
  no member skips a transparent pixel.
* **After the write the plugin asks the host to repaint the clip rectangle**:
  it calls `Layer.update(clipLeft, clipTop, clipWidth, clipHeight)` with the
  four values it read at the start of the call (`layerExBase.hpp:99-103`).
* **The image is marked modified on every call**, even when the call changed
  no pixel, because the plugin asks the host for the *write* buffer before it
  does anything (`layerExBase.hpp:80`; `mainImageBufferForWrite` is the
  write-marking accessor).

### 2.1 The same four members in the engine's tree

The port writes into the engine's own MainImage, whose in-memory layout is
**straight-alpha RGBA8, `r, g, b, a` per pixel**
(`docs/code/engine/layer_ops.md:10-12`). The channel mapping a port must
apply is therefore:

| upstream buffer byte | meaning | engine buffer byte |
|---|---|---|
| 0 | blue | 2 |
| 1 | green | 1 |
| 2 | red | 0 |
| 3 | alpha | 3 (untouched) |

Alpha is straight (not premultiplied) on both sides, and since no member
reads or writes the alpha byte, the premultiplication question does not
arise: the four members are pure colour-channel transforms.

The engine's scene keeps the clip rectangle as `clipLeft`/`clipTop`/
`clipWidth`/`clipHeight` and the pitch as `width * 4` bytes
(`crates/engine/src/scene.rs:2091-2096`); the clip rectangle of a layer is
the rectangle the engine's own layer operations use
(`engine_api::layer_ops::clip_rect`).

---

## 3. Argument marshalling and host requirements

* **Argument count.** A call with fewer arguments than the member's arity is
  an error: the binding layer refuses the call with the VM's
  bad-parameter-count error (`TJS_E_BADPARAMCOUNT`, code `-1004`;
  `tp_stub.h:734`; the check is `ncbind.hpp:1281`). A call with *more*
  arguments than the arity is accepted and the surplus arguments are ignored
  (the same check is a "fewer than" test).
* **Numeric coercion.** Each parameter is a 32-bit integer or a 64-bit real
  in the upstream's signature (`LayerExImage.h:36`, `:44`, `:50`), and the
  binding layer converts the script value with the TJS2 numeric conversion
  before the narrowing (`ncbind.hpp:352-354`, `:445-458`):
  * an integer argument keeps its value; a real argument is truncated toward
    zero for an integer parameter and kept for a real parameter;
  * a string argument that reads as a number becomes that number (truncated
    toward zero for an integer parameter); a string that reads as nothing
    becomes 0;
  * `void` becomes 0;
  * an object or a byte string has no numeric conversion. The upstream's
    variant conversion is the host's own business; in the engine's tree such
    a value has no numeric conversion and the conversion raises. The port
    keeps the tree's rule and must not silently invent a value.
  * The integer parameters are 32-bit and wrap on overflow.
* **Host requirements.** The plugin reads the eight `Layer` members listed in
  §1 through the host's own property lookup. When the host does not have one
  of them, the plugin raises a script error whose message is
  `FAILED: get property object :` followed by the member's name
  (`layerExBase.hpp:26-33`, `:49-55`). This can only happen on a host that
  lacks `Layer.imageWidth`, `Layer.imageHeight`,
  `Layer.mainImageBufferForWrite`, `Layer.mainImageBufferPitch`,
  `Layer.clipLeft`, `Layer.clipTop`, `Layer.clipWidth` or `Layer.clipHeight`;
  the engine's `Layer` class has all eight, so a port cannot hit it.

---

## 4. `Layer.colorize(hue, sat, blend)`

### 4.1 Signature

| parameter | direction | type | meaning |
|---|---|---|---|
| `hue` | in | integer | the target hue, 0..255 where 0 is ≈ 0° and 255 is ≈ 360°. The value is stored in a byte, so any integer is taken **modulo 256**. |
| `sat` | in | integer | the target saturation, 0..255, also stored in a byte, so modulo 256. |
| `blend` | in | real | 0..1: 0 leaves the pixel alone, 1 replaces it with the fully re-hued colour. |

`hue` and `sat` reach the plugin as byte-sized fields of a four-byte colour
record (`tp_stub.h:102-107`; the assignment is `LayerExImage.cpp:201-202`,
`:206-207`), which is what makes the modulo-256 truncation observable. The
upstream's own manual documents the three parameters without giving ranges
for `hue` and `sat`, and gives `blend` the range 0 (no effect) to 1 (full
effect) (`manual.tjs:14-20`).

The member returns void (`LayerExImage.h:36`).

### 4.2 Semantics in one sentence

For every pixel of the clip rectangle: convert the pixel's colour to the
plugin's byte-packed HSL, **replace the hue and the saturation** with the
requested ones and **keep the lightness**, convert that back to RGB with the
plugin's floating-point HSL step, and then either use that colour as it is
(`blend` above 0.999) or mix it with the original colour by the `blend`
weight. The alpha byte is untouched.

### 4.3 The exact arithmetic

Let the pixel's colour channels be `R`, `G`, `B`, each 0..255.

**Step 1 — colour to byte-packed HSL** (`LayerExImage.cpp:78-119`). All the
divisions in this step are integer divisions truncating toward zero.

* `cMax` = the largest of R, G, B; `cMin` = the smallest.
* `L` = ((cMax + cMin) × 255 + 255) / 510.
* If `cMax` = `cMin` (a grey pixel): `S` = 0 and `H` = 170 (the plugin's
  undefined-hue constant, 255 × 2 / 3).
* Otherwise:
  * `S` = ((cMax − cMin) × 255 + (cMax + cMin) / 2) / (cMax + cMin) when
    `L` ≤ 127, and ((cMax − cMin) × 255 + (510 − cMax − cMin) / 2) /
    (510 − cMax − cMin) when `L` > 127. (The divisor of the second form is
    never zero, because `cMax` ≠ `cMin` excludes `cMax` = `cMin` = 255.)
  * `Rdelta` = ((cMax − R) × 42 + (cMax − cMin) / 2) / (cMax − cMin);
    `Gdelta` and `Bdelta` are the same with G and B. (42 is 255 / 6,
    truncated.)
  * `H` = (Bdelta − Gdelta) when R = `cMax`; (85 + Rdelta − Bdelta) when
    G = `cMax`; (170 + Gdelta − Rdelta) when B = `cMax`.
  * **The hue is stored in a byte, so a negative hue difference wraps.** When
    R is the largest channel, `Bdelta − Gdelta` lies in −42..42, and the byte
    store turns a negative value into 214..255 (it does **not** clamp it to
    0 and does **not** add 255 to it in a way that a signed implementation
    would differ on — the value is the low eight bits of the difference).
    In the other two branches the value is 43..127 and 128..212, so it never
    wraps.
  * The plugin's "if the hue is above the maximum, subtract the maximum"
    step never fires, because the hue was already stored in a byte
    (`LayerExImage.cpp:115`). A port that keeps the hue in a signed integer
    and *then* stores it in a byte reproduces the upstream exactly; a port
    that clamps the signed hue to 0..255 does not.

**Step 2 — apply the requested hue and saturation** (`LayerExImage.cpp:199-208`).
`H` becomes `hue` modulo 256, `S` becomes `sat` modulo 256, `L` is unchanged.

**Step 3 — HSL back to RGB** (`LayerExImage.cpp:121-171`). This step is
computed in **32-bit floating point**, and the plugin's own expression order
matters at the last bit; a port should use single precision here.

* `h` = H × 360 / 255; `s` = S / 255; `l` = L / 255 (all single precision).
* `m2` = l × (1 + s) when l ≤ 0.5, and l + s − l × s otherwise;
  `m1` = 2 × l − m2.
* If `s` = 0: the three channels are all (byte)(l × 255).
* Otherwise, with the hue-to-colour helper of `LayerExImage.cpp:121-142`,
  which first folds the hue into 0..360 (once: 360 more becomes 360 less, less
  than 0 becomes 360 more) and then answers
  `n1 + (n2 − n1) × hue / 60` for a hue below 60, `n2` for a hue below 180,
  `n1 + (n2 − n1) × (240 − hue) / 60` for a hue below 240, and `n1` otherwise:
  * red = (byte)(helper(m1, m2, h + 120) × 255)
  * green = (byte)(helper(m1, m2, h) × 255)
  * blue = (byte)(helper(m1, m2, h − 120) × 255)
* The byte store truncates toward zero.

**Step 4 — the blend** (`LayerExImage.cpp:182-188`, `:213-215`).

* `blend` is first clamped into 0..1.
* `a0` = (int)(256 × blend) — truncated toward zero — and `a1` = 256 − `a0`.
* When `blend` > 0.999 the result of step 3 *is* the new pixel.
* Otherwise each channel is (new × `a0` + old × `a1`) >> 8.

Because `a0` + `a1` = 256 and the shift divides by 256, `blend` = 0 leaves the
pixel **exactly** unchanged and `blend` = 1 would give the step-3 colour
exactly — but 1 is caught by the "full blend" rule above, and the two paths
agree there anyway.

### 4.4 Edge cases

* **An empty clip rectangle** (a clip width or height of 0): nothing is
  written; the member still marks the image modified and still asks for the
  repaint. No error.
* **A layer with no image**: the upstream has no buffer address and writes
  through it — undefined behaviour, in practice a crash. The port must not
  crash; see §8.
* **`blend` outside 0..1** is clamped, not rejected.
* **`hue` and `sat` outside 0..255** are taken modulo 256, not rejected.
  `hue` = 255 and `hue` = −1 are the same hue (360° ≡ 0°); `sat` = 256 is
  the same as `sat` = 0 (fully desaturated).
* **A transparent pixel** is transformed exactly like an opaque one; only the
  alpha byte distinguishes them.
* **A grey pixel** (R = G = B) keeps its lightness and gets the requested hue
  and saturation; with a non-zero saturation a grey therefore becomes
  coloured.
* **The colour round trip is lossy.** Even at `blend` = 1 the result is not
  exactly the input: the plugin's HSL round trip moves a channel by up to one
  step (see the worked examples in §4.6). A port that "fixes" the rounding
  diverges from the upstream.

### 4.5 Errors

* Too few arguments → the bad-parameter-count error (§3).
* Any other condition → **no error**: the member never raises for an
  in-range call, and it never validates `hue`, `sat` or `blend`.

### 4.6 Worked examples

The plugin's own arithmetic, applied to single pixels (the alpha byte is
unchanged in every row):

| input R, G, B | `colorize(0, 255, 1.0)` | `colorize(120, 255, 1.0)` | `colorize(255, 255, 1.0)` | `colorize(0, 0, 1.0)` | `colorize(0, 255, 0.5)` | `colorize(0, 255, 0.0)` |
|---|---|---|---|---|---|---|
| 255, 0, 0 | 254, 1, 1 | 1, 254, 210 | 254, 1, 1 | 128, 128, 128 | 254, 0, 0 | 255, 0, 0 |
| 0, 255, 0 | 254, 1, 1 | 1, 254, 210 | 254, 1, 1 | 128, 128, 128 | 127, 128, 0 | 0, 255, 0 |
| 0, 0, 255 | 254, 1, 1 | 1, 254, 210 | 254, 1, 1 | 128, 128, 128 | 127, 0, 128 | 0, 0, 255 |
| 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 255, 255, 255 |
| 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 |
| 128, 64, 32 | 160, 0, 0 | 0, 160, 131 | 160, 0, 0 | 80, 80, 80 | 144, 32, 16 | 128, 64, 32 |
| 10, 200, 90 | 210, 0, 0 | 0, 210, 172 | 210, 0, 0 | 105, 105, 105 | 110, 100, 45 | 10, 200, 90 |

Reading of the table: `colorize(0, 255, 1.0)` forces every pixel to the pure
red hue (the 254/1 pair is the round-trip loss); `colorize(120, 255, 1.0)`
forces a hue of 120 × 360 / 255 ≈ 169° (a spring green); `colorize(255, …)`
is the same hue as `colorize(0, …)`; `colorize(0, 0, 1.0)` is a full
desaturation to the pixel's own lightness.

---

## 5. `Layer.modulate(hue, saturation, luminance)`

### 5.1 Signature

| parameter | direction | type | meaning |
|---|---|---|---|
| `hue` | in | integer | a hue rotation in degrees; the upstream's manual documents −180..180 (`manual.tjs:21-28`). Any integer is accepted; it is used as `hue / 360` of a turn. |
| `saturation` | in | integer | a saturation change in percent; the manual documents −100..100 (`manual.tjs:21-28`). |
| `luminance` | in | integer | a lightness change in percent; the manual documents −100..100 (`manual.tjs:21-28`). |

The member returns void (`LayerExImage.h:44`).

**This member uses a different colour model from `colorize`**: a
double-precision HSL with the hue as a fraction of a turn (0..1), not the
byte-packed one of §4. The two members are *not* two spellings of the same
transform; a port must implement both separately
(`LayerExImage.cpp:244-309` against `LayerExImage.cpp:78-171`).

### 5.2 Semantics in one sentence

For every pixel of the clip rectangle: convert the colour to a
double-precision HSL, rotate the hue by `hue` degrees, push the saturation
and the lightness by the given percentages, and convert back to RGB. The
alpha byte is untouched.

### 5.3 The exact arithmetic

The three parameters are first turned into fractions
(`LayerExImage.cpp:321-323`): `h` = hue / 360, `s` = saturation / 100,
`l` = luminance / 100. All the arithmetic below is **double precision**.

Per pixel (`LayerExImage.cpp:244-309`):

1. Normalise: `red` = R / 255, `green` = G / 255, `blue` = B / 255.
2. `cMax` = the largest, `cMin` = the smallest, `delta` = cMax − cMin,
   `add` = cMax + cMin, `lum` = add / 2.
3. If `delta` = 0 (a grey pixel): the saturation is 0 and the hue fraction is
   0. Otherwise:
   * the saturation is `delta / add` when `lum` < 0.5, and
     `delta / (2 − add)` otherwise;
   * the hue fraction is (green − blue) / delta when red is the largest, 2 +
     (blue − red) / delta when green is the largest, and 4 + (red − green) /
     delta when blue is the largest; the result is then divided by 6, so the
     hue fraction is a fraction of a turn.
4. Rotate the hue: the hue fraction becomes the hue fraction + h; then, while
   it is below 0 add 1, and while it is above 1 subtract 1 (repeatedly, so a
   large rotation still lands in 0..1).
5. Push the saturation: if s > 0, saturation = saturation + (1 − saturation) × s;
   otherwise saturation = saturation + saturation × s. (So s = 0 leaves it
   alone, s = 1 makes it 1, s = −1 makes it 0, and s = −2 makes it the
   negative of itself — the upstream does not clamp.)
6. Push the lightness the same way: if l > 0, lum = lum + (1 − lum) × l;
   otherwise lum = lum + lum × l. (l = 1 makes the pixel white, l = −1 makes
   it black.)
7. Back to RGB:
   * If the saturation is exactly 0: all three channels are (int)(lum × 255).
   * Otherwise `m2` = lum × (1 + saturation) when lum ≤ 0.5, and lum +
     saturation − lum × saturation otherwise; `m1` = 2 × lum − m2; and each
     channel comes from the plugin's double-precision hue-to-colour helper
     (`LayerExImage.cpp:227-242`), which folds the hue into 0..1 once (below 0
     gains 1, above 1 loses 1) and then answers
     `n1 + (n2 − n1) × hue × 6` for a hue below 1/6, `n2` for a hue below 1/2,
     `n1 + (n2 − n1) × (2/3 − hue) × 6` for a hue below 2/3, and `n1`
     otherwise; the channel is (int)(that × 255), truncated toward zero.
   * The red channel uses the hue + 1/3, the green the hue itself and the
     blue the hue − 1/3 (`LayerExImage.cpp:305-307`).
8. The three channels are written back as bytes and the alpha byte is skipped
   (`LayerExImage.cpp:333-336`). A channel value outside 0..255 (only
   reachable with an out-of-range `saturation` or `luminance`) wraps modulo
   256 on the way into the byte.

### 5.4 Edge cases

* **An empty clip rectangle**: nothing is written; the image is still marked
  modified and the repaint is still requested. No error.
* **A layer with no image**: undefined behaviour upstream (§8).
* **An empty argument list**: too few arguments → error (§3).
* **`hue` = 0, `saturation` = 0, `luminance` = 0** is *nearly* the identity,
  but not exactly: the double-precision HSL round trip can move a channel by
  one step (10, 200, 90 becomes 9, 200, 89). A port must reproduce the round
  trip, not shortcut it.
* **`saturation` = −100** desaturates completely: every pixel becomes grey
  with its own lightness (255, 0, 0 becomes 127, 127, 127).
* **`luminance` = 100** makes every pixel white; **`luminance` = −100** makes
  every pixel black — including a pixel that was already black.
* **`hue` outside −180..180** is accepted: the rotation is normalized into
  0..1 by the loop in step 4, so `hue` = 360 is the same as `hue` = 0 and
  `hue` = 540 is the same as `hue` = 180.
* **An out-of-range `saturation` or `luminance`** (say −200) is not clamped
  and can produce a negative saturation or lightness; the resulting channel
  values wrap modulo 256.

### 5.5 Errors

* Too few arguments → the bad-parameter-count error (§3).
* Any other condition → **no error**; no argument is validated.

### 5.6 Worked examples

| input R, G, B | `modulate(0, 0, 0)` | `modulate(120, 0, 0)` | `modulate(180, 0, 0)` | `modulate(0, 100, 0)` | `modulate(0, -100, 0)` | `modulate(0, 0, 100)` | `modulate(0, 0, -100)` |
|---|---|---|---|---|---|---|---|
| 255, 0, 0 | 255, 0, 0 | 0, 255, 0 | 0, 254, 255 | 255, 0, 0 | 127, 127, 127 | 255, 255, 255 | 0, 0, 0 |
| 0, 255, 0 | 0, 255, 0 | 0, 0, 255 | 254, 0, 255 | 0, 255, 0 | 127, 127, 127 | 255, 255, 255 | 0, 0, 0 |
| 0, 0, 255 | 0, 0, 255 | 255, 0, 0 | 255, 254, 0 | 0, 0, 255 | 127, 127, 127 | 255, 255, 255 | 0, 0, 0 |
| 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 255, 255, 255 | 0, 0, 0 |
| 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 0, 0, 0 | 255, 255, 255 | 0, 0, 0 |
| 128, 64, 32 | 128, 64, 32 | 32, 128, 63 | 32, 95, 128 | 160, 53, 0 | 80, 80, 80 | 255, 255, 255 | 0, 0, 0 |
| 10, 200, 90 | 9, 200, 89 | 89, 9, 200 | 200, 9, 119 | 0, 210, 88 | 105, 105, 105 | 255, 255, 255 | 0, 0, 0 |

---

## 6. `Layer.noise(level)`

### 6.1 Signature

| parameter | direction | type | meaning |
|---|---|---|---|
| `level` | in | integer | the noise amplitude. The upstream's manual documents 0 (no noise) to 255 (a lot of noise) (`manual.tjs:30-34`). Any integer is accepted; nothing is clamped. |

The member returns void (`LayerExImage.h:50`).

### 6.2 Semantics in one sentence

For every pixel of the clip rectangle, add an independent random offset to
each of the three colour channels and clamp the result into 0..255; the alpha
byte is untouched.

### 6.3 The exact arithmetic

Per pixel (`LayerExImage.cpp:347-365`), for each of the three colour channels
in turn — the buffer's blue, then green, then red:

* The offset is (int)((u − 0.5) × level), where `u` is a uniform random
  number in 0..1 (the upstream draws it from the C library's global random
  generator: `rand() / RAND_MAX`, which is 0 when the draw is 0 and 1 when
  the draw is `RAND_MAX`). The multiplication and the cast are as written:
  the offset is truncated toward zero to an integer.
* The new channel value is the old value plus the offset, clamped into
  0..255.
* The three channels draw **independently** — three separate random numbers
  per pixel — so the noise is coloured, not monochrome.
* The alpha byte is skipped, not written.

### 6.4 The random source — what is and is not specified

The upstream draws from the C library's process-wide `rand()`
(`LayerExImage.cpp:354`, `:356`, `:358`). The plugin never seeds it, so the
sequence is the C library's default sequence and differs between C libraries
and between platforms; it is also shared with the rest of the process.

What **is** specified, and what a port must reproduce:

* the offset is the truncation **toward zero** of (u − 0.5) × level, so its
  sign follows the sign of (u − 0.5) — a draw below the midpoint darkens the
  channel, a draw above it brightens it — and its magnitude never exceeds
  floor(|level| / 2) (for `level` = 255 that is 127);
* the magnitude distribution, for a positive level L: the offset 0 with a
  probability of 2 / L (and with a probability of 1 when L is 1); the offsets
  ±k with 1 ≤ k < floor(L / 2) with a probability of 1 / L each; and the two
  extreme offsets ±floor(L / 2) with a probability of 1 / (2 × L) each when L
  is odd — when L is even the extreme offsets ±L / 2 are practically
  unreachable, because truncating toward zero reaches them only at the single
  extreme draw;
* **the smallest level that can change a pixel is 3**: at `level` = 1 the
  offset is always 0, and at `level` = 2 it is 0 for every draw but the two
  extreme ones;
* the clamping of the channel into 0..255;
* the alpha is untouched.

What is **not** specified, and cannot be: the actual sequence of offsets. A
port must choose its own generator and its own seed policy (§8).

### 6.5 Edge cases

* **An empty clip rectangle**: nothing is written; the image is still marked
  modified and the repaint is still requested. No error.
* **A layer with no image**: undefined behaviour upstream (§8).
* **`level` = 0**: every offset is 0, so the pixels do not change — but the
  member still marks the image modified and still asks for the repaint.
* **`level` = 1 or 2**: every offset is 0 as well (§6.4), so the pixels do
  not change.
* **A negative `level`**: the offset is negative, so the channels are
  *darkened* by up to |level| / 2. The upstream does not clamp the level and
  does not treat a negative level as "no noise".
* **A level above 255**: accepted; the offset range grows with it and the
  per-channel clamp into 0..255 does the rest.
* **A transparent pixel** is noised exactly like an opaque one.

### 6.6 Errors

* Too few arguments → the bad-parameter-count error (§3).
* Any other condition → **no error**.

---

## 7. `Layer.generateWhiteNoise()`

### 7.1 Signature

No parameters (`LayerExImage.h:55`; `manual.tjs:36-39`). The member returns
void.

### 7.2 Semantics in one sentence

Replace the colour of every pixel of the clip rectangle with a uniform
grey — the three colour channels become the same value — and keep the alpha
byte; the original colour is ignored entirely.

### 7.3 The exact arithmetic

Per pixel (`LayerExImage.cpp:370-383`):

* One random number is drawn and mapped to a byte: the upstream divides the
  C library's draw by the **integer** constant `RAND_MAX / 255` (with
  `RAND_MAX` = 2147483647 that constant is 8421504) and truncates the
  quotient, so the value is uniform over 0..255.
* The blue, green and red bytes of the pixel are all set to that value
  (`LayerExImage.cpp:378`).
* The alpha byte is not written.

The value is *not* derived from the pixel's old colour: a pixel that was
opaque red and a pixel that was transparent black both become the same grey.

### 7.4 The random source

As in §6.4: the upstream draws from the C library's global `rand()`, so the
exact grey values are not reproducible across C libraries. What is specified
is that each pixel's value is uniform over 0..255 and that the three colour
channels of a pixel are equal.

### 7.5 Edge cases

* **An empty clip rectangle**: nothing is written; the image is still marked
  modified and the repaint is still requested. No error.
* **A layer with no image**: undefined behaviour upstream (§8).
* **No arguments**: the member takes none, so a call with an argument is
  accepted and the argument is ignored (§3).
* **A transparent pixel** keeps its alpha and gets the grey colour, so it
  stays invisible.

### 7.6 Errors

* Any other condition → **no error**. (There is no argument-count failure
  for this member: it takes no arguments.)

---

## 8. The upstream's build system, and what a port must decide for itself

### 8.1 The build

* The upstream builds with **CMake**, through the shared plugin macro of the
  host's build (`CMakeLists.txt:1-10`): the plugin is one target,
  `layerExImage`, built from `LayerExImage.cpp`, `Main.cpp` and
  `LicensesGen.cpp`, with the `NCBIND` option, which pulls in the binding
  library's own source (`tp_stub/krkrz.cmake:20-35`).
* The build needs three sibling checkouts next to the plugin: the host
  header stub (`tp_stub`), the binding library (`ncbind`) and — for the
  plugins that need it — the simple binder (`tp_stub/krkrz.cmake:3-5`).
* The plugin is a **shared** plugin whose only exported symbols are
  `V2Link` and `V2Unlink` (`tp_stub/krkrz.cmake:88-118`); it is loaded by the
  host's own plugin loader, so a script reaches it with
  `Plugins.link("layerExImage.dll")`.
* The plugin's own licence machinery registers the licence of the third-party
  component whose code the colour steps come from (`LicensesGen.cpp:1-72`,
  `licenses/manifest.json`). A port does not need it, but it does need to
  respect the licence of the algorithm it ports: the upstream's own licence
  file (`LICENSE.TXT`) and the note in `readme.txt` that the image-processing
  code is derived from a third-party component under a zlib-style licence.
  **A port that copies the upstream's arithmetic verbatim inherits that
  licence obligation**; a port that writes the behaviour from this
  specification does not copy any code, but the licence of the *algorithm's
  origin* is worth checking with the integrator.
* Where each member lives: `colorize`, `modulate`, `noise` and
  `generateWhiteNoise` are all implemented in `LayerExImage.cpp` and declared
  in `LayerExImage.h`; the registration is in `Main.cpp`; the shared pixel
  window is in `layerExBase.hpp`.

### 8.2 Decisions this specification does not settle

1. **The random source and the seed policy.** The upstream's `rand()` cannot
   be reproduced; the port must pick a generator and decide how it is seeded
   (per call, per layer, from a clock). The *distribution*, the truncation
   and the clamping are specified in §6.3 and §7.3; the sequence is not.
2. **A layer with no main image.** The upstream writes through a null buffer
   address — undefined behaviour. The port must choose: a no-op (the engine's
   own layer operations do this) or a typed error. This is a port decision,
   not an upstream behaviour, and it must be recorded as such.
3. **The clip rectangle against the image bounds.** The upstream trusts the
   clip rectangle and does not intersect it with the image; the engine's own
   operations intersect. The port should intersect (the engine's clip
   rectangle is always inside the image, so the two agree in practice).
4. **The repaint.** The upstream calls `Layer.update(clipLeft, clipTop,
   clipWidth, clipHeight)`; the engine's equivalent is marking the layer for
   repaint and marking the changed rectangle of the bitmap dirty.
5. **Where the members live in the tree.** The four members are `Layer`
   members; the tree's convention for a `Layer` pixel member is the layer
   pixel module, while the plugin's convention is a module under the
   built-in plugin directory. Either is defensible; the registered surface
   and its baseline change either way, and the DLL name `layerExImage.dll`
   joins the built-in plugin table.
6. **The other two members of the upstream plugin.** The upstream attaches
   six members to `Layer` (`Main.cpp:31-36`); two of them, `light` and
   `gaussianBlur`, shadow members the host's own `Layer` class already has.
   This specification covers only the four new names. A port that registers
   the plugin wholesale would override the two existing members, which the
   engine does not want.
7. **The engine's own L2 operations.** The engine already has in-place
   colour operations named after two of these members. A port must check
   them against §4 and §6 rather than assume they match. The known
   difference to check: the tree's noise operation treats a non-positive
   level as "do nothing", while the upstream **darkens** the pixels for a
   negative level (§6.5). The tree's `colorize` operation matches §4's
   arithmetic as far as this specification can tell, down to the single
   precision rounding; §4.6's worked values are the check.

---

## 9. The questions a writer would have to ask

1. Which generator and seed policy for `noise` and `generateWhiteNoise`, and
   must a repeated call with the same arguments give the same result?
2. A layer without a main image: no-op or a typed error (§8.2, item 2)?
3. Do the four members answer only after `Plugins.link("layerExImage.dll")`,
   or are they on the `Layer` class from the start? (The upstream registers
   them when the DLL is linked.)
4. Are the four members registered as `Layer` instance members in the layer
   pixel module, or in a plugin module under the built-in plugin directory?
5. Must `modulate` and `generateWhiteNoise` be added to the engine's L2
   operations, or may they live in the compat layer? (The engine's rule is
   that a per-pixel operation over the bitmap belongs in L2.)

---

## 10. Citation index

Every rule above carries its upstream `file:line` inline. The index below is
the quick map from a rule to its source.

| rule | upstream location |
|---|---|
| the members hang on `Layer`; the six registered names | `Main.cpp:30-37` |
| the native instance is refreshed at every call | `Main.cpp:11-27` |
| the members' declarations and parameter types | `LayerExImage.h:36`, `:44`, `:50`, `:55` |
| the upstream's own parameter documentation | `manual.tjs:14-20`, `:21-28`, `:30-34`, `:36-39` |
| the eight host `Layer` members the plugin reads and the ninth it calls | `layerExBase.hpp:78-86`, `:99-103` |
| the "FAILED: get property object" error | `layerExBase.hpp:26-33`, `:49-55` |
| the clip rectangle is the region; the buffer offset | `LayerExImage.cpp:22-30` |
| the B, G, R, A byte order | `LayerExImage.cpp:196-198`, `:217-220`, `:329-336`, `:376-378` |
| the alpha byte is untouched | `LayerExImage.cpp:220`, `:336`, `:360`, `:378` |
| the byte-packed colour → HSL step | `LayerExImage.cpp:78-119` |
| the undefined-hue constant 170 | `LayerExImage.cpp:75`, `:96` |
| the hue wrap into a byte | `LayerExImage.cpp:107-115` |
| the single-precision HSL → colour step | `LayerExImage.cpp:121-171` |
| `colorize` | `LayerExImage.cpp:179-225` |
| the blend weights `a0`/`a1` and the full-blend rule | `LayerExImage.cpp:182-188`, `:213-215` |
| the double-precision hue → colour step | `LayerExImage.cpp:227-242` |
| the double-precision colour → HSL step and the adjustments | `LayerExImage.cpp:244-309` |
| `modulate` | `LayerExImage.cpp:318-341` |
| `noise` | `LayerExImage.cpp:347-365` |
| `generateWhiteNoise` | `LayerExImage.cpp:370-383` |
| the argument-count rule | `ncbind.hpp:1281`; the error code `tp_stub.h:734` |
| the numeric coercion of the parameters | `ncbind.hpp:352-354`, `:445-458` |
| the byte-sized colour record the hue and saturation land in | `tp_stub.h:102-107` |
| the build | `CMakeLists.txt:1-10`; `tp_stub/krkrz.cmake:3-5`, `:20-35`, `:88-118` |
| the licence registration | `LicensesGen.cpp:1-72`; `licenses/manifest.json`; `LICENSE.TXT`; `readme.txt` |
