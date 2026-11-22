# `saveStruct` — `Scripts.toStructString` behavioural specification

A clean-room specification for the one `saveStruct` member the engine is
missing. It is written for an implementer who will **not** see the upstream
source. It contains behaviour and exact text only — no upstream code.

---

## 0. Provenance and citation convention

| what | where |
|---|---|
| upstream plugin repository | `https://github.com/wtnbgo/saveStruct` |
| upstream commit read | `f03a81a46018d89d20d1e4614006fc502fd12d2c` (2026-10-19) |
| plugin source file | `Main.cpp` (436 lines) at the root of that repository |
| the plugin's text writer | a **second** upstream repository, `https://github.com/wtnbgo/json`, commit `331577f704d1d64c648646f2a4fdfd4b816f9387` (2026-07-02), file `Writer.hpp`; the plugin includes it as `../json/Writer.hpp` (`Main.cpp:4`) |
| upstream manual | `manual.tjs` (pseudo-code manual, 79 lines) |
| upstream readme | `readme.txt` (62 lines, Japanese) |

Citation forms used below:

* `Main.cpp:NNN` — the upstream plugin at the commit above.
* `json/Writer.hpp:NNN` — the upstream writer repository at its commit above.
* `manual.tjs:NNN`, `readme.txt:NNN` — the upstream documentation.
* `VM crates/tjs/cpp/tjs2/NNN` — the **engine's own** vendored TJS2 VM. The
  plugin delegates every *value text* to the VM it runs on, so the VM is the
  oracle for those texts; these citations are to the engine's tree, not to the
  upstream.

The plugin is built for a TJS2 host (KirikiriZ / KiriKiri2 lineage). The
engine's compat layer runs the vendored TJS2 VM, so the VM citations below
describe the behaviour the plugin will actually produce inside this engine.

---

## 1. The member

### 1.1 Script-visible signature

`Scripts.toStructString(target [, newline [, option ]])` → **String**

* It hangs on the **`Scripts`** class object as a **static** member: the call
  is `Scripts.toStructString(...)`; `this` inside the member is the `Scripts`
  class object; the member is not copied onto instances
  (`Main.cpp:381-383`).
* The member name is exactly `toStructString` (`Main.cpp:382`).
* There is one overload. The arguments are positional; there are no named
  arguments and no keyword arguments.

| # | name | direction | required | default | meaning |
|---|---|---|---|---|---|
| 0 | `target` | in | **yes** | — | the value to serialize |
| 1 | `newline` | in | no | `1` | newline selector: `1` → LF, **every other value** → CRLF |
| 2 | `option` | in | no | `0` | formatting flag bitmask (see §3) |

* The return value is a String holding the serialized text
  (`Main.cpp:375`).
* `newline` and `option` are read as **integers** (`Main.cpp:372`,
  `Main.cpp:374`): the argument is converted with the VM's integer
  conversion (VM `crates/tjs/cpp/tjs2/tjsVariant.h:864-884`), so
  * an integer is itself;
  * a real is truncated toward zero;
  * a string is parsed by the VM's number parser — a leading numeric literal
    is used (a real literal truncated), otherwise the value is `0` (VM
    `crates/tjs/cpp/tjs2/tjsVariantString.cpp:457-466`);
  * `void` is `0`;
  * an **object** or an **octet** raises the VM's variant conversion error
    (§5.1).
* The default of `newline` is `1` when the argument is **absent**
  (`Main.cpp:372`); an explicitly passed `0` selects CRLF.
* The default of `option` is `0` when the argument is **absent**
  (`Main.cpp:374`).

> **Documented default vs implemented default.** The upstream manual declares
> the member as `toStructString(target, newline=1, option=ssoIndent|ssoSort)`
> (`manual.tjs:57`) and the readme repeats that this member's option default
> differs from the `Array`/`Dictionary` ones (`readme.txt:54-55`). The
> **implementation** passes `0` when the option argument is absent
> (`Main.cpp:374`), i.e. the *same* default the `Array`/`Dictionary` members
> use (`Main.cpp:283`, `Main.cpp:339`). The code is the oracle: **the
> implemented default is `0`** (no indent, no sort). The upstream's own
> documentation is wrong here; do not implement the documented default.

### 1.2 What the member does

The member serializes `target` into the plugin's *saveStruct* text form and
returns that text (`Main.cpp:370-377`). The text form is described in §2; the
formatting flags in §3.

`target` may be any value: a dictionary, an array, another object, or a
primitive (`int` / `real` / `string` / `octet` / `null` / `void`)
(`readme.txt:52-53`, `manual.tjs:51`).

---

## 2. The text form

The member writes into a text buffer. The buffer starts empty and the
newline string is fixed for the whole call (`Main.cpp:372`).

Two primitives of the buffer matter:

* **newline**: the buffer's newline string is LF when the newline selector is
  exactly `1`, and CRLF for every other selector value — including `0`, `2`
  and negative values (`json/Writer.hpp:12-23`).
* **indent level**: a counter, initially `0`. Writing a newline emits the
  newline string followed by as many **space** characters as the current
  indent level (`json/Writer.hpp:30-35`). Opening a container's first entry
  raises the level by one and then writes a newline; closing the container
  lowers the level by one and then writes a newline
  (`json/Writer.hpp:37-45`).

### 2.1 A value's text

Every value is written as one of the following (`Main.cpp:184-208`).

**Object values** (`Main.cpp:189-199`):

| value | text |
|---|---|
| `null` | the four characters `null` (`Main.cpp:192-193`) |
| an `Array` instance | the *array form* (§2.3) |
| any other object | the *dictionary form* (§2.2) |

The array/dictionary decision is made by asking the VM whether the object is
an instance of `Array` (`Main.cpp:194`).

**Primitive values** (`Main.cpp:200-207`): first a *type prefix*, then the
value's *expression text*.

* The type prefix is `int ` for an integer and `real ` for a real
  (`Main.cpp:203-204`). It is **omitted** when the const flag (§3) or the
  undocumented no-prefix flag (§3) is set (`Main.cpp:202`). A `string`, an
  `octet` and `void` never get a type prefix.
* The expression text is the VM's own "this value as an expression" text
  (`Main.cpp:206`). The VM defines it as
  (VM `crates/tjs/cpp/tjs2/tjsUtils.cpp:174-232`):

| value | expression text |
|---|---|
| `void` | `void` |
| integer | the decimal digits, with `-` for a negative value |
| real | the IEEE-754 hex form, then ` /* `, then the decimal text, then ` */` |
| string | `"`, the C-escaped content, `"` |
| octet | `<% `, the bytes in uppercase hex separated by single spaces, ` %>` |

  * The **real's hex form** (VM `crates/tjs/cpp/tjs2/tjsVariant.cpp:252-297`)
    is: an optional `-`, then `0x1.`, then the 52-bit significand written as
    exactly **13 uppercase hex digits**, then `p`, then the *unbiased*
    exponent as a signed decimal integer (the stored exponent field minus
    1023). So `1.5` is `0x1.8000000000000p0` and `0.5` is
    `0x1.0000000000000p-1`. The five special values are **not** written in
    hex: `NaN`, `+Infinity`, `-Infinity`, `+0.0`, `-0.0`
    (VM `crates/tjs/cpp/tjs2/tjsVariant.cpp:219-238`).
  * The **decimal text** after `/*` is the VM's ordinary real-to-text
    conversion: 15 significant digits, plain form when the exponent is in
    `[-4, 15)`, otherwise the exponent form; `+0.0` / `-0.0` / `NaN` /
    `+Infinity` / `-Infinity` for the special values
    (VM `crates/tjs/cpp/tjs2/tjsVariant.cpp:241-249`).
    So the whole text of the real `1.5` is
    `real 0x1.8000000000000p0 /* 1.5 */`.
  * The **string escaping** (VM `crates/tjs/cpp/tjs2/tjsString.cpp:164-231`)
    is C-style: BEL → `\a`, BS → `\b`, FF → `\f`, LF → `\n`, CR → `\r`,
    TAB → `\t`, VT → `\v`, backslash → `\\`, single quote → `\'`, double
    quote → `\"`; any other character below `0x20` → `\xNN` with **lower**
    hex digits; every other character is copied unchanged. Because a `\xNN`
    escape would otherwise swallow a following hex digit, a hex digit
    (`0`-`9`, `a`-`f`, `A`-`F`) that directly follows a `\xNN` escape is
    itself written as `\xNN` (so the string `"\x01" + "a"` becomes
    `"\x01\x61"`).
  * The **octet's bytes** (VM `crates/tjs/cpp/tjs2/tjsVariant.cpp:114-140`)
    are uppercase hex, two digits per byte, separated by a single space. An
    empty octet contributes nothing, so its text is `<%  %>` (two spaces).

### 2.2 The dictionary form

An object that is not an `Array` is written as a dictionary (`Main.cpp:196-198`):

* If the const flag is set, `(const)` is written first, with **no space**
  before `%[` (`Main.cpp:150`).
* `%[` is written (`Main.cpp:152`), then the entries, then `]`
  (`Main.cpp:159`).
* An **entry** is: the member name in double quotes, C-escaped by the rules
  of §2.1, then `=>`, then the member's value text
  (`Main.cpp:121-127`).
* The entries are separated by `,` **followed by the newline** and the
  current indentation (`Main.cpp:118-119`). The **first** entry is *not*
  preceded by a newline unless the indent flag is active
  (`Main.cpp:114-116`).
* A **hidden** member (the VM's hidden-member flag on the enumeration
  callback) is skipped unless the hidden flag is set (`Main.cpp:98`).
  Hidden members are those the VM marks hidden when a class declares them
  hidden (VM `crates/tjs/cpp/tjs2/tjsObject.cpp:1276-1282`); a plain
  dictionary has none.
* With the sort flag the entries are written in **ascending name order**
  (§3); without it they are written in the VM's member-enumeration order.
* The members are read from the object by the VM's member enumeration; the
  status of that enumeration is **not** checked (`Main.cpp:156`), so an
  object whose enumeration fails (or yields nothing) produces `%[]` rather
  than an error (`readme.txt:39-40` says such objects "cannot be saved"; in
  practice the upstream produces an empty dictionary form).

### 2.3 The array form

An `Array` instance is written as an array (`Main.cpp:162-182`):

* If the const flag is set, `(const)` is written first, with **no space**
  before `[` (`Main.cpp:164`).
* `[` is written (`Main.cpp:166`), then the elements, then `]`
  (`Main.cpp:181`).
* The element count is the array's `count` member (`Main.cpp:167`).
* The elements are separated by `,` (`Main.cpp:171-174`); **no newline** is
  inserted between them unless the indent flag is active *for this array*.
* The indent flag applies to an array only when the array is **not empty**
  (`Main.cpp:168`).
* An element that cannot be read is skipped (the read's status is checked,
  `Main.cpp:176`).

### 2.4 Nesting

A container's members and elements are written with the **same** option value
as the container (`Main.cpp:127`, `Main.cpp:177`), so every flag applies at
every depth.

---

## 3. The option flags

The `option` argument is a bitmask. Four bits are script-visible: the
upstream registers them as **global** constants when the plugin is loaded and
removes them when it is unloaded (`Main.cpp:389-406`, `Main.cpp:411-421`;
`manual.tjs:64-79`).

| bit | value | global name | effect |
|---|---|---|---|
| 0 | `1` | `ssoIndent` | **Indent.** A non-empty container's first entry starts on a new line, one space deeper per nesting level, and the container's closing bracket is alone on a line at the parent's indentation. The container's entries stay separated by `,` + newline. |
| 1 | `2` | `ssoConst` | **Const.** Every container is prefixed with `(const)` (no space), and the `int ` / `real ` type prefixes are dropped everywhere, at every depth (`Main.cpp:202`). |
| 2 | `4` | `ssoSort` | **Sort.** A dictionary's entries are written in ascending name order; an array keeps its element order. |
| 3 | `8` | `ssoHidden` | **Hidden.** The hidden members are written too (`Main.cpp:98`). |

Two further bits exist in the upstream but are **not** exposed as globals
(`Main.cpp:12-13`):

| bit | value | effect |
|---|---|---|
| 10 | `1024` | The `int ` / `real ` type prefixes are dropped, without the const behaviour (`Main.cpp:202`). |
| 11 | `2048` | An array ignores the indent flag (`Main.cpp:168`). |

Details of the **indent** flag:

* Opening a container's first entry raises the indent level by one and then
  writes the newline + the spaces of the new level; closing the container
  lowers the level by one and then writes the newline + the spaces of the
  parent level (`json/Writer.hpp:37-45`, `Main.cpp:116`, `Main.cpp:144`,
  `Main.cpp:169`, `Main.cpp:180`).
* One space per level (`json/Writer.hpp:32-34`).
* An **empty** container is never broken across lines: an empty dictionary is
  `%[]` and an empty array is `[]` — the closing indentation is only written
  when at least one entry was written (`Main.cpp:144`, `Main.cpp:168`).
* The closing bracket of an indented container is preceded by a newline and
  the parent's indentation; the closing bracket of a **non**-indented
  container directly follows the last entry (`Main.cpp:118-119`).

Details of the **sort** flag:

* The names are collected during the member enumeration and the entries are
  written afterwards, in the collected order sorted ascending
  (`Main.cpp:99-101`, `Main.cpp:130-143`).
* The order is the VM's ordinary ascending comparison of the names, i.e. a
  comparison by UTF-16 code unit (VM
  `crates/tjs/cpp/tjs2/tjsArray.cpp:602-718` for the default sort order and
  `crates/tjs/cpp/tjs2/tjsArray.cpp:135-140` for its comparison, VM
  `crates/tjs/cpp/tjs2/tjsVariant.cpp:856-882` for the string comparison).
* The sort is **not** stable, but the names of a dictionary are distinct, so
  the resulting order is deterministic.

Unknown bits are ignored.

---

## 4. Exact results

Notation: `\n` is LF (`0x0A`), `\r\n` is CR LF (`0x0D 0x0A`). These are the
literal results of `Scripts.toStructString(target)` with the default
arguments (newline `1`, option `0`), unless stated otherwise.

| call | result |
|---|---|
| `Scripts.toStructString(%[])` | `%[]` |
| `Scripts.toStructString(%[a=>1,b=>"x"])` | `%["a"=>int 1,\n"b"=>"x"]` |
| `Scripts.toStructString(%[a=>1,b=>"x"], 1, ssoIndent)` | `%[\n "a"=>int 1,\n "b"=>"x"\n]` |
| `Scripts.toStructString(%[b=>1,a=>"x"], 1, ssoSort)` | `%["a"=>"x",\n"b"=>int 1]` |
| `Scripts.toStructString(%[a=>1], 1, ssoConst)` | `(const)%["a"=>1]` |
| `Scripts.toStructString([1,2])` | `[int 1,int 2]` |
| `Scripts.toStructString([1,2], 1, ssoIndent)` | `[\n int 1,\n int 2\n]` |
| `Scripts.toStructString([1,2], 1, ssoIndent\|2048)` | `[int 1,int 2]` (the array ignores the indent, §3) |
| `Scripts.toStructString([1,2], 1, 1024)` | `[1,2]` (the type prefixes are dropped, §3) |
| `Scripts.toStructString([])` | `[]` |
| `Scripts.toStructString(%[a=>%[b=>1]], 1, ssoIndent)` | `%[\n "a"=>%[\n  "b"=>int 1\n ]\n]` |
| `Scripts.toStructString(1.5)` | `real 0x1.8000000000000p0 /* 1.5 */` |
| `Scripts.toStructString(1.0)` | `real 0x1.0000000000000p0 /* 1 */` |
| `Scripts.toStructString("a\"b")` | `"a\"b"` |
| `Scripts.toStructString("a\nb")` | `"a\nb"` (the two characters `\` `n`, not a line break) |
| `Scripts.toStructString(<% 00 ff %>)` | `<% 00 FF %>` |
| `Scripts.toStructString(null)` | `null` |
| `Scripts.toStructString(void)` | `void` |
| `Scripts.toStructString(-42)` | `int -42` |
| `Scripts.toStructString(%[a=>1,b=>2], 0)` | `%["a"=>int 1,\r\n"b"=>int 2]` |

With `newline` `0`, every LF of the results above becomes CR LF; with any
newline value other than `1` the same happens (`json/Writer.hpp:15-22`).

---

## 5. Edge cases and error behaviour

### 5.1 What raises

* **`Scripts.toStructString()` with no argument** — the call fails: the
  upstream returns the VM's wrong-argument-count error before touching the
  target (`Main.cpp:370`). In this engine's terms: a script error the script
  can catch. The upstream's message is the VM's generic
  "Invalid argument count"; the engine's house style for a native member is
  a typed error whose message names the member (see
  `crates/compat/tests/json.rs` for the `Scripts.saveJSON` precedent) — the
  *observable* fact the test must pin is that **the call raises**.
* **An object or an octet as the `newline` or the `option` argument** — the
  VM's integer conversion raises its variant conversion error (VM
  `crates/tjs/cpp/tjs2/tjsVariant.h:864-884`), so the call fails. This is the
  only other condition that raises.
* **Nothing else raises.** In particular:
  * An object that does not support member enumeration → `%[]`, no error
    (`Main.cpp:156`).
  * A member whose value cannot be read → in the **sorted** path that member
    is skipped (`Main.cpp:137-138`); in the **unsorted** path the value is
    delivered by the enumeration itself, and the VM stops the enumeration
    when the value cannot be read (VM
    `crates/tjs/cpp/tjs2/tjsObject.cpp:1289-1299`), so the members after it
    are not written.
  * An array element that cannot be read → that element is skipped
    (`Main.cpp:176`).
  * An unknown option bit → ignored.
  * An out-of-range newline selector → not an error; every value other than
    `1` means CR LF.
  * A `target` that is a primitive → not an error; it is serialized as §2.1
    describes.

### 5.2 Silent behaviour worth pinning

* **No cycle detection.** A structure that contains itself is serialized
  recursively until the stack is exhausted; the upstream has no guard
  (`Main.cpp:127`, `Main.cpp:177` recurse without a depth limit).
* **The member's value is read at write time.** A member whose getter raises
  propagates that error to the script (the enumeration callback runs inside
  the member's own call).
* **The type prefix is dropped at every depth** when the const flag is set
  (`Main.cpp:202` is reached from every nesting level).
* **A hidden member is skipped** unless the hidden flag is set, and the
  skipped member does not affect the "was anything written" test that
  decides the closing indentation (`Main.cpp:114`, `Main.cpp:144`).
* **The sort flag changes only the entry order** of dictionaries; the
  enumeration order of the *unsorted* case is the VM's own member order and
  is not specified by the upstream.

---

## 6. The upstream's other members (summary, for the faithfulness check)

The upstream plugin attaches five members and four globals. Only
`Scripts.toStructString` is in scope for this specification; the rest are
listed so the writer can tell whether the engine's existing members match.

| member | kind | upstream behaviour | citation |
|---|---|---|---|
| `Array.save2(filename [, utf8 [, newline]])` | instance member of `Array` | Writes each element's **string** form followed by the newline to a file; a non-string element is an error (the VM's string conversion raises). The file is written in UTF-8 when `utf8` is non-zero, otherwise in the host's current code page. | `Main.cpp:227-246` |
| `Array.saveStruct2(filename [, utf8 [, newline [, option]]])` | instance member of `Array` | Writes the array in the plugin's structural text form (§2.3) to a file, with the same encoding and newline rules. | `Main.cpp:256-268` |
| `Array.toStructString([newline [, option]])` | instance member of `Array` | The same text as §2.3, returned as a String. Defaults: newline `1`, option `0`. | `Main.cpp:276-287` |
| `Dictionary.saveStruct2(filename [, utf8 [, newline [, option]]])` | **static** member of the `Dictionary` class | Writes the dictionary in the plugin's structural text form (§2.2) to a file. | `Main.cpp:312-324`, attached at `Main.cpp:346-349` |
| `Dictionary.toStructString([newline [, option]])` | **static** member of the `Dictionary` class | The same text as §2.2, returned as a String. Defaults: newline `1`, option `0`. | `Main.cpp:332-343`, attached at `Main.cpp:346-349` |
| `ssoIndent` / `ssoConst` / `ssoSort` / `ssoHidden` | globals | `1` / `2` / `4` / `8`; registered on the global object when the plugin loads, removed when it unloads. | `Main.cpp:389-406`, `Main.cpp:411-421`, `manual.tjs:64-79` |

### 6.1 How the engine's existing members compare

The engine already implements `Array.save2`, `Array.saveStruct2`,
`Array.toStructString`, `Dictionary.saveStruct2` and
`Dictionary.toStructString` (`crates/compat/src/natives/plugins/save_struct/`),
ported from a **different revision** of the same plugin — the one the
reference engine ships. That revision keeps the string escaping and the
octet formatting in the plugin file itself (the engine's own citations to
the reference's file point at those lines), while the upstream revision read
here delegates both to the VM and adds the formatting flags and the
`Scripts` member. The upstream revision is the later one: in the upstream's
own history, the rewrite "保存処理を大幅改訂" of 2016-12-31 introduced the
formatting flags and the `Scripts` member.

The upstream revision and the engine's existing port differ in these
script-observable ways:

| point | upstream revision | the engine's existing port |
|---|---|---|
| `newline` default of `Array`/`Dictionary.toStructString` | `1` → LF (`Main.cpp:281`, `Main.cpp:337`) | `0` → CR LF (the reference revision's default) |
| the `option` argument of `Array`/`Dictionary.toStructString` | present (`Main.cpp:283`, `Main.cpp:339`) | absent — the argument is ignored |
| the formatting flags (`ssoIndent`, `ssoConst`, `ssoSort`, `ssoHidden`) | present (§3) | absent |
| `(const)` prefix | present | absent |
| a `null` value | `null` (`Main.cpp:193`) | `nullptr` |
| a real value | `real 0x1.…p… /* decimal */` (`Main.cpp:206`) | `real <decimal>` |
| an octet value | `<% 00 FF %>` — uppercase hex (VM `tjsVariant.cpp:114-140`) | `<% 00 ff %>` — lowercase hex |
| a string value | full C escaping, control characters included (VM `tjsString.cpp:164-231`) | only `"` and `\` are escaped |
| the `sso*` globals | registered (`Main.cpp:389-406`) | not registered |
| the file members' encoding | UTF-8 or the current code page (`Main.cpp:232-235`) | UTF-16 LE (the reference revision's format) |

**Consequence for this port.** The engine's existing port does not implement
this member at all (it is the one member the engine's TODO lists as missing
for this plugin), so there is no existing behaviour to preserve for it — but
the *text* it must produce (§2, §4) is the upstream revision's text, which is
**not** the text the engine's `Array`/`Dictionary.toStructString` produce
today. The implementer must therefore decide, and the decision belongs to
the integrator:

* **Option A (member-local).** Implement `Scripts.toStructString` with the
  upstream's text and flags, as a separate path; the existing
  `Array`/`Dictionary.toStructString` keep the reference revision's text.
  The engine then holds two text forms of "saveStruct" side by side, and the
  two members of the same plugin disagree — which is *not* what the upstream
  does (upstream, `Scripts.toStructString(x)` and `x.toStructString()` agree
  byte for byte at the default arguments).
* **Option B (whole plugin moves to the upstream revision).** Re-base the
  plugin's text core on the upstream revision: the existing members' output
  changes (the table above), the existing tests that pin the reference
  revision's text change with them, and the surface baseline is updated.
  This is the faithful-to-the-upstream choice, but it changes members that
  are already implemented and tested.

§8 lists what each option needs from the tree.

---

## 7. Build system, file layout, and what a port must decide for itself

### 7.1 The upstream's build

* The upstream builds with **CMake** (`CMakeLists.txt`, 8 lines) against a
  stub of the host API: `CMakeLists.txt:1-2` points at a sibling `tp_stub`
  directory and includes `krkrz.cmake` from it, and `krkrz_plugin(saveStruct
  NCBIND SOURCES Main.cpp)` declares the plugin. The plugin is an
  **NCBIND** plugin (the `ncbind.hpp` C++ binding helper, `Main.cpp:1`), so
  its members are attached to host classes with the binding's attach macros.
* The upstream's own build history also shows a static-link build
  (commit `2c2c9c4`, "static対応") and a renamed helper class to avoid a
  symbol clash when several plugins are linked together (commit `bb69a6e`).
* The plugin has **no** build-time dependency on this engine and **no**
  fixture, no data file, no device feature. It is pure text formatting.
* The upstream's writer (the second repository) is header-only
  (`Writer.hpp`): a text-buffer writer and a file writer. The file writer
  encodes the buffer as UTF-8 or in the host's current code page
  (`json/Writer.hpp:127-149`). The string writer used by `toStructString` is
  encoding-neutral (`json/Writer.hpp:51-90`).

### 7.2 Which file each member lives in

| member | upstream file | lines |
|---|---|---|
| `Scripts.toStructString` | `Main.cpp` (the class that attaches to `Scripts`) | `Main.cpp:354-383` |
| `Array.save2`, `Array.saveStruct2`, `Array.toStructString` | `Main.cpp` | `Main.cpp:215-294` |
| `Dictionary.saveStruct2`, `Dictionary.toStructString` | `Main.cpp` | `Main.cpp:299-349` |
| the members' attachment to their classes | `Main.cpp` | `Main.cpp:290-294`, `Main.cpp:346-349`, `Main.cpp:381-383` |
| the text core (value / array / dictionary text) | `Main.cpp` | `Main.cpp:72-208` |
| the `sso*` globals | `Main.cpp` | `Main.cpp:389-406`, `Main.cpp:411-421` |
| the newline / indent behaviour | `json/Writer.hpp` (second repository) | `json/Writer.hpp:12-45` |
| the value texts the plugin delegates to the VM | the TJS2 VM | VM `crates/tjs/cpp/tjs2/tjsUtils.cpp:174-232`, `tjsVariant.cpp:114-140`, `tjsVariant.cpp:219-297`, `tjsString.cpp:164-231` |

### 7.3 Decisions a port must make that this specification does not settle

1. **Which text core the new member uses** — Option A or Option B of §6.1.
   This decides whether the engine keeps two saveStruct text forms.
2. **The `sso*` globals.** The upstream registers them on the global object
   (`Main.cpp:389-406`); the engine registers none of them today. A script
   that writes `Scripts.toStructString(x, 1, ssoIndent|ssoSort)` needs them;
   the upstream's own manual uses them in the member's documented default
   (`manual.tjs:57`). The values are fixed (`1`, `2`, `4`, `8`), so this is a
   decision only about whether to expose them.
3. **The hidden-member flag (bit 3).** The upstream's hidden members are the
   members a class declares hidden; the engine's VM-side enumeration helper
   drops hidden members before the caller sees them
   (VM/engine `crates/tjs/cpp/tjs2_abi.cpp:1810-1837`), and the engine's
   existing serializer walks the members through the VM's own `Array.assign`,
   which drops them as well. Producing the hidden members therefore needs a
   new capability (an enumeration that reports the hidden flag), or the flag
   is accepted and has no effect — which would be a silent no-op and is
   forbidden by the engine's rules.
4. **The real's hex form.** The engine has no hexadecimal float formatter
   today (the VM's decimal real-to-text is the only real formatting in the
   tree), so the hex form of §2.1 has to be written from scratch. Its exact
   shape is fixed by the upstream's VM behaviour (§2.1): `-`, `0x1.`, 13
   uppercase hex digits of the significand, `p`, the unbiased exponent.
5. **The escaping.** The VM's C escaping (§2.1) is not exposed to the Rust
   side; the engine's existing serializer re-implements a *narrower* escape
   in its TJS helper script. The full set (`\a \b \f \n \r \t \v \\ \' \"`
   and `\xNN`, including the hex-digit-after-hex-escape rule) has to be
   implemented wherever the new member's text is produced.
6. **The octet's uppercase hex** — the same point: the engine's existing
   helper writes lowercase.
7. **The error message** for the missing `target` argument (§5.1): the
   upstream raises the VM's generic wrong-argument-count error; the engine's
   convention for a native member is a typed error naming the member.
8. **The surface baseline.** `Scripts.toStructString` is a new member on the
   `Scripts` class, so `docs/surface/runtime_surface.json` changes and its
   baseline is updated with the change (the engine's surface lock). The four
   `sso*` globals are integers, so they are not part of the class dump.

---

## 8. What each option of §6.1 needs from the tree

**Option A (member-local) needs:**

* a second text core for `Scripts.toStructString` that produces §2's text;
* the full C escaping, the uppercase octet hex and the real's hex form
  (§7.3 items 4-6);
* the sort flag (a name sort — the tree's own string comparison is by code
  unit for the VM's strings, so this is straightforward);
* the hidden flag, or an explicit decision not to support it (§7.3 item 3);
* the `sso*` globals, if scripts are to use them (§7.3 item 2);
* a surface-baseline update.

**Option B (the whole plugin moves to the upstream revision) needs** all of
the above, plus:

* the existing `Array`/`Dictionary.toStructString` and `saveStruct2` members
  re-pointed at the new text core;
* the existing `newline` default changed from `0` to `1`;
* the existing tests that pin the reference revision's text updated, with
  their citations re-pointed at the upstream revision;
* the file members' encoding revisited (the upstream writes UTF-8 or the
  current code page; the engine writes UTF-16 LE, which is the *reference
  revision's* behaviour and is what the existing tests pin).

---

## 9. Questions the writer would have to ask

1. Option A or Option B of §6.1? (The answer changes the existing members'
   observable output.)
2. Are the four `sso*` globals part of this change?
3. Is the hidden-member flag supported, or is the change blocked on a new
   enumeration capability?
4. Which error message does the missing-`target` call raise — the VM's
   generic one, or a typed error naming the member (the engine's convention)?
5. Does the sort use the VM's comparison of the *names* (code-unit order) or
   the tree's own byte order? (They differ for non-ASCII names.)
