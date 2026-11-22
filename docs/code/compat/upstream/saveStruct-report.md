# `Scripts.toStructString` — report

## Patch

`/tmp/pi-patches/write-saveStruct/patch.diff` (1278 lines, applies with
`git apply --3way`; the new file is included as a `new file` hunk).

*Note*: the working tree is shared with other agents. Twice during this
task a concurrent `git checkout`/restore wiped my uncommitted files (I
rewrote them and now keep the patch in `/tmp` as the copy of record), and
once another agent edited one line of the new file
(`crates/compat/src/natives/plugins/save_struct/scripts_member.rs`, the
digit conversion in `decimal_exponent`). That edit is kept, the file is
`rustfmt`-clean, and the patch matches the working tree.

## Files I wrote

| file | what |
|---|---|
| `crates/compat/src/natives/plugins/save_struct/scripts_member.rs` | **new** — the `Scripts.toStructString` member: the traversal helper, the token-stream reader and the text writer. |
| `crates/compat/src/natives/plugins/save_struct/mod.rs` | **extended** — `mod scripts_member;`, the registration call, the member table row and the `# The two text forms` section. No existing behaviour, comment or baselined line changed. |
| `crates/compat/tests/save_struct.rs` | **extended** — the new tests appended at the end. No existing test touched, removed or rewritten; the module doc gained one paragraph. |

I chose to **extend the existing test file** rather than add a second one: the
plugin's tests belong together, and the file's `Env` harness (a bound VM with
a mounted storage) is exactly what the new member needs. The file grows from
601 to ~880 lines.

## The three settled questions

### 1. The error text for the missing target

The **engine's typed error that names the member**:
`"Scripts.toStructString requires 1 argument"`
(`crates/compat/src/natives/plugins/save_struct/scripts_member.rs`, the
`scripts_to_struct_string` callback).

Why: that is what the tree's own native bridge uses for a wrong argument
count — `Scripts.evalJSON requires 1 argument`
(`crates/compat/src/natives/plugins/json/mod.rs:928`), `Array.save2 requires
1 argument` (`crates/compat/src/natives/plugins/save_struct/mod.rs`, the
`array_save2` callback). The upstream raises the VM's generic "Invalid
argument count"; the engine's bridge never produces that text, so the
observable fact the test pins is "the call raises", and the message names
the member like every other native member does.

### 2. The name sort

The **VM's code-unit order** (UTF-16), reproduced with a sort key built in
the module: `compare_names` compares `left.encode_utf16()` with
`right.encode_utf16()` (`scripts_member.rs`). The tree's own byte order
(UTF-8) differs for a name above the basic multilingual plane against a
name in the private use area, so the key is built rather than borrowed.

### 3. The four `sso*` globals

**Not part of this change.** The member reads the option bits *by value*
(`1`, `2`, `4`, `8`, plus the two undocumented `1024` and `2048`), so the
globals are not needed for the member to work. The upstream's globals are a
separate item: they are script-visible integers the upstream registers on
the global object when the plugin loads and removes when it unloads
(`Main.cpp:389-406`, `Main.cpp:411-421`), and this change registers no
global at all.

## The script-visible members I registered

Exactly one:

* **`Scripts.toStructString`** — a **static** member of the `Scripts` class
  object, registered with `native_util::register_builtin_static_members`
  (`scripts_member.rs`, `register`), so the class carries it and an instance
  never copies it.

Nothing else is script-visible: the traversal helper lives on a namespace
dictionary (`__krkr_save_struct_scripts_ns`) that is removed from the global
object right after it is built, and the test
`the_scripts_member_is_a_static_member_without_leaking_helpers` pins that
none of the helper names is a global.

**The surface baseline changes**: the `Scripts` class in
`docs/surface/runtime_surface.json` gains `"toStructString"` (the list is
sorted). The integrator must run `krkr-gate surface-lock --update` (or edit
the baseline by hand) — this is the only baseline change in the change.

## The tests

Thirteen new tests, appended to `crates/compat/tests/save_struct.rs`. Every
behaviour assertion cites the upstream's `Main.cpp:NNN` (the upstream is
the oracle for this member — the engine's own plugin revision does not have
it); the VM's texts cite the engine's vendored VM.

| test | what it pins |
|---|---|
| `scripts_to_struct_string_writes_the_dictionary_form` | `%[]` and `%["a"=>int 1,\n"b"=>"x"]` (`Main.cpp:150-159`, `:118-119`) |
| `scripts_to_struct_string_indents_the_containers` | the indent bit: `%[\n "a"=>int 1,\n "b"=>"x"\n]` and the nested `%[\n "a"=>%[\n  "b"=>int 1\n ]\n]` (`Main.cpp:116`, `:144`, `json/Writer.hpp:30-45`) |
| `scripts_to_struct_string_writes_the_array_form` | `[int 1,int 2]`, the indented array, the array-ignores-indent bit `2048`, and `[]` (`Main.cpp:162-182`) |
| `scripts_to_struct_string_writes_the_const_form` | `(const)%["a"=>1]` and the type-prefix bit `1024` (`Main.cpp:202`, `:150`, `:164`) |
| `scripts_to_struct_string_sorts_the_entries` | the sort bit `4` and the VM's own member order without it (`Main.cpp:99-101`, `:130-143`) |
| `scripts_to_struct_string_defaults_to_no_options` | the implemented option default `0` (not the manual's `ssoIndent\|ssoSort`) (`Main.cpp:374`, `manual.tjs:57`) |
| `scripts_to_struct_string_writes_the_value_texts` | `real 0x1.8000000000000p0 /* 1.5 */`, `real 0x1.0000000000000p0 /* 1 */`, `"a\"b"`, `"a\nb"`, `<% 00 FF %>`, `null`, `void`, `int -42` (`Main.cpp:184-208`) |
| `scripts_to_struct_string_selects_the_newline` | LF for the selector `1` (the default), CRLF for `0` and for `2` (`Main.cpp:372`, `json/Writer.hpp:12-23`) |
| `scripts_to_struct_string_requires_a_target` | the call without a target raises (`Main.cpp:370`) |
| `scripts_to_struct_string_rejects_a_non_numeric_argument` | an object as the newline or the option raises (the VM's integer conversion) (`Main.cpp:372`, `:374`) |
| `scripts_to_struct_string_rejects_the_hidden_option` | the hidden-member bit raises (`Main.cpp:98`) |
| `the_scripts_member_is_a_static_member_without_leaking_helpers` | the member is on the `Scripts` class object; no helper name is a global (`Main.cpp:381-383`, `:411-421`) |

The twenty exact result strings of the specification are all pinned by the
tests above (the empty dictionary, the two-member dictionary, the indented
dictionary, the sorted dictionary, the const dictionary, the array, the
indented array, the array with the indent bit, the prefix-bit array, the
empty array, the nested dictionary, the two reals, the quoted string, the
string with a newline, the octet, `null`, `void`, the negative integer and
the CRLF dictionary).

**One input spelling differs from the specification.** The specification
writes the dictionary literal `%[a=>1,b=>"x"]`; this engine's TJS grammar
spells an entry `%[a: 1]` (`=>` does not parse — it is read as an
expression, and the probe raises `Member "a" does not exist`). The tests use
the engine's spelling; the members named are the same.

## The two text forms, and where the traversal is shared

The module documentation (`crates/compat/src/natives/plugins/save_struct/mod.rs`,
section `# The two text forms`) states which member writes which form and
why: the plugin has two revisions, the engine carries the members of both,
and each member keeps the form of the revision it belongs to.

* The `Array`/`Dictionary` members keep the first revision's form — the
  existing TJS serializer script is untouched, and so are its tests.
* `Scripts.toStructString` writes the second revision's form.

The two forms **share the traversal**: both walk the value through the VM's
own member enumeration (the builtin `Array.assign` walk), so the member
order and the hidden-member filtering are the VM's in both. They **do not
share a formatter**, because they read differently (a `null` value, a real,
an octet and a string all have their own text in each) and lay out
differently (the newline default and the formatting flags). The first
form's formatter is the existing TJS script; the second form's formatter is
the Rust writer in `scripts_member.rs`.

The new member's traversal is a small TJS helper
(`HELPER_SCRIPT` in `scripts_member.rs`) that classifies every value and
pushes it into a flat token stream (a container's marker, its entry count,
and one entry per member or element). The Rust side turns the stream into
the text. The helper keeps the recursion inside the VM, so a structure that
contains itself runs into the VM's own limit instead of the Rust stack.

## Divergences I introduced, and why

1. **The hidden-member bit (`8`) raises.** The engine's VM-side member
   enumeration drops the hidden members, and the existing serializer reaches
   them through the same walk, so the bit cannot have an effect here. A bit
   that silently does nothing is forbidden, so the member raises a typed
   error that names the missing capability:
   *"Scripts.toStructString: the hidden-member option (8) is not supported:
   the engine's member enumeration does not report the hidden members"*.
   The specification's §7.3 item 3 lists the same gap.
2. **A member whose value cannot be read propagates the error.** The
   specification's §5.1 distinguishes two cases that the VM does not: the
   sorted path skips such a member and the unsorted path stops the
   enumeration, while §5.2 says a raising getter propagates to the script.
   The two are not separable across the bridge (both arrive as one failed
   read), so the member propagates the error — the §5.2 behaviour, and the
   one the house style wants (no swallowed failure).
3. **A TJS `property` member contributes its getter's result.** The
   upstream's enumeration delivers the value with the ordinary flags, so the
   getter runs; the helper reads the member the same way. The first form's
   port reads the raw value instead, so the two forms differ in that one
   point as well — deliberately, each following its own revision.
4. **The upstream's manual is wrong about the option default.** The manual
   declares `option=ssoIndent|ssoSort` (`manual.tjs:57`); the implementation
   passes `0` when the argument is absent (`Main.cpp:374`). The member
   implements the **implemented** default (`0`), and
   `scripts_to_struct_string_defaults_to_no_options` pins it.

## What I could not implement as specified

Nothing in the twenty result strings. The two things the tree cannot do
today are handled explicitly rather than silently: the hidden-member bit
raises (point 1 above), and the four `sso*` globals are not registered (a
separate item, see question 3).

## Risks

* **The Rust side is not compiled.** I may not run cargo. The code is
  formatted (`rustfmt --edition 2024`, clean) and passes every
  compiler-free gate check (`house-style`, `error-handling`, `layering`,
  `stateless`, `callback-objthis`, `void-getter`, `native-instance-lookup`,
  `log-format`, `native-construction`, `lifecycle-order`, `doc-lists`), but
  it needs the integrator's build.
* **The helper script is validated, the token-stream reader is not.** I ran
  the helper and a model of the formatter inside the engine (through the
  app's startup script) and checked the layout against the specification's
  table; the Rust reader of the token stream has not been executed.
* **The unsorted member order in two tests depends on the VM's hash
  order.** `%[a: 1, b: "x"]` enumerates as `a`, `b` and
  `%[b: 2, a: 1, c: 3]` as `c`, `a`, `b` — I verified both on the current
  build. The upstream does not specify that order (it is the VM's own), but
  the two tests pin it because the sort test needs an order that differs
  from the sorted one.
* **The specification's dictionary literal spelling does not parse here**;
  the tests use `%[a: 1]`.

## What I left alone

* The `Array`/`Dictionary` members and their tests — untouched.
* `crates/compat/src/natives/plugins/mod.rs` — untouched (the plugin is
  already registered).
* The `sso*` globals — a separate item.
* The surface baseline — the integrator's to update.

## What I need from the integrator

1. The build (`cargo build` / `cargo clippy --workspace --all-targets`) and
   the test suites (`uv run --project scripts krkr-gate test`).
2. The surface-lock baseline update: `Scripts` gains `toStructString` in
   `docs/surface/runtime_surface.json`.
3. A look at the two tests that pin the VM's member order, in case the
   review prefers not to pin that order.
