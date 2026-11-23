# Gate checks (`krkr-gates`)

The repository's mechanical gate checks live here as one uv project. Its
console entry point is `krkr-gate`.

## Run

From the repository root:

```bash
uv run --project scripts krkr-gate gate              # full gate (main agent; includes cargo)
uv run --project scripts krkr-gate <check>           # one check
uv run --project scripts krkr-gate <check> --self-test
```

`gate` runs the ordered sequence and includes the `cargo fmt`/`clippy`
steps, so it is for the main agent; the tests run separately with
`krkr-gate test`. `gate --no-cargo` runs only the
compiler-free checks. Every check also runs on its own; `android-target`
shells out to `cargo tree`, the rest read the tree.

The static gate sequence is `cargo fmt` → `stateless` → `error-handling` →
`house-style` → `log-format` → `layering` → `storage-boundary` →
`native-construction` → `lifecycle-order` → `storage-media` →
`callback-objthis` → `native-arity` → `void-getter` →
`native-instance-lookup` → `cargo clippy` →
`cargo clippy platform-android` → `android-target` → `apk-surface` →
`surface-lock` →
`native-parity --no-doc` → `plugin-parity --no-doc` → `host-facts` → `ci` →
`doc-lists`. The ordered gate, the test suites and the CI release pipeline
are documented in [`../docs/rules/gates.md`](../docs/rules/gates.md).

## Layout

- `krkr_gates/` — the package; `cli.py` is the entry point.
- `krkr_gates/checks/` — one module per check, each exposing `run(argv) -> int`.
- `fixtures/` — the checks' own test corpus; see below.
- `allowlists/` — the reviewed exceptions, one file per check.
- `apk_surface_check.py` / `apk_surface_check.sh` — the release job's
  APK surface check; not a gate check, because it needs the built package. The
  compile-free half of the same contract is `krkr-gate apk-surface`. See below.
- `pyproject.toml` / `uv.lock` — the project metadata.

### The APK surface check

`apk_surface_check.sh` runs in the release workflow's `apk` job, after the
package is assembled and before it is uploaded. It verifies the names the
native code resolves *by name* — the static fields the engine reads off its
Activity through JNI, and the game-activity glue's lookup of the native entry
point — and the classes and members the keep rules name.
The check reads the DEX tables, the ELF symbol table and the compiled manifest
directly, with the standard library only, so it needs no SDK tool and no
device; `--self-test` mutates the real package four ways and asserts that each
mutation is caught.

### The slow-frame statistics

`slow_frame_stats.py` turns a run's slow-frame records into the statistics of
the slow frames. The engine reports every frame whose gap reaches the
threshold the caller set (`--slow-frame-ms`), as one record: a header that
names the frame, the gap and the cost that dominates it, and the continuation
lines that carry the frame's own costs. The script reads those records by
their `key=value` fields and reports how many frames reached the threshold,
the distribution of their gaps, every cost summed over them and ranked, which
cost dominated how many of them, and the worst frames with their own
breakdown. The record is parsed by its fields, never by the position of a
column, and a broken record is counted and left out rather than guessed at.

```bash
python3 scripts/slow_frame_stats.py --threshold-ms 20 run.log
python3 scripts/slow_frame_stats.py --self-test
```

The threshold is the caller's: 20 ms is a 50 fps budget, 33.3 ms is a 30 fps
one. The output is plain text, one `key=value` fact per line, prefixed with
`slow-frame-stats `, and it carries the frame numbers and the cost names
only — nothing of the game's own content.

### The fixtures are the checks' own tests

Every check carries a `--self-test` that runs it against a small corpus of
deliberately broken and deliberately clean inputs, so a check that stops
detecting anything fails the gate instead of passing it silently. The corpus
lives in `fixtures/`, one directory per check:

- a `bad_*` file is a case the check **must** report — a pointer cast without
  a safety note, an error swallowed by `unwrap_or`, a layer that reaches into
  another layer's engine state;
- a `good_*` file is a case the check **must not** report — the same code with
  the problem fixed;
- three checks (`storage-boundary`, `storage-media`, `host-facts`) scan a whole
  miniature source tree under `fixtures/<check>/tree/` and compare what they
  find against an `expected.json`, so a file there is a *sample* of the code
  the check reads, not a file it names individually;
- `stateless` scans a miniature crate tree under `fixtures/stateless/tree/`
  the same way, except that it has no `expected.json`: its corpus is a broken
  sample per rule and a clean sample beside it, and the self-test asserts that
  the broken sample trips every rule and the clean one trips none;
- `doc-lists` compares whole trees: `fixtures/doc_lists/good/` holds a
  registry and the two pages that agree with it, `fixtures/doc_lists/bad/`
  holds the same registry and pages that drift from it in every way the rules
  can see. The self-test asserts that the bad tree trips every rule and that
  the good tree trips none.

They are therefore not a reference copy of anything: they are the inputs of
the checks' self-tests, and the gate exercises them on every run
(`check-host-facts: self-test ok (4 seam(s), 2 unwired in the fixture)`).
Deleting one makes that check's self-test fail.

## Checks

`stateless`, `error-handling`, `house-style`, `log-format`, `layering`,
`storage-boundary`, `native-construction`, `lifecycle-order`, `storage-media`,
`callback-objthis`, `native-arity`, `void-getter`, `native-instance-lookup`,
`host-facts`, `android-target`, `apk-surface`, `ci`, `surface-lock`,
`native-parity`, `plugin-parity`, `doc-lists`, the `extract-xp3` helper, and
`gate`.

`stateless` fails a process-global mutable state in the bottom-core crates
(`static mut`, `thread_local!`, `lazy_static!`, `LazyLock`, `OnceLock`,
`OnceCell`) and a stateful (`&mut self`) method on a service object
(`RenderPipeline`, `FileSystem`, `Media`). Its fixture corpus under
`fixtures/stateless/tree/` holds one broken sample per rule and one clean
sample beside it, and the self-test asserts that every rule still fires on the
broken sample and that the clean sample trips nothing.

`doc-lists` keeps the pages that list the gate's steps in step with the code:
it reads `GATE_SEQUENCE` from `scripts/krkr_gates/cli.py` — the registry the
gate dispatches from, and the one owner of the list — and compares it with
the ordered table in [`../docs/rules/gates.md`](../docs/rules/gates.md) §1
(the steps, their order and their kind) and with the table in
[`../docs/rules/code_styles.md`](../docs/rules/code_styles.md) §10 (every
check the gate runs). A missing, extra, reordered or mistyped entry is a
finding with the file and the line; the check never writes, because a guard
that rewrote the pages would hide the drift it exists to catch. Its fixture
corpus under `fixtures/doc_lists/` holds a tree whose pages agree with the
registry and one that drifts from it in every way the rules can see.

`house-style` also enforces the AGENTS §6 comment rules: a **non-test**
comment may not cite the external engine (`reference/cpp`, a `*.cpp:line`, a
`.h`/`.hpp` filename, `C++`, `TVP`, `KiriKiri`, `ported`/`upstream`/`port of`)
or narrate history (`used to`, `previously`, `before this`, `replaced by`);
test comments keep those citations. In addition, no Rust file — tests
included — may carry `#[allow(clippy::…)]` / `#![allow(clippy::…)]` /
`#[expect(clippy::…)]`; the `clippy-allow` rule has no allowlist exemption.
Legacy comments that predate these rules are recorded in
`allowlists/house_style_baseline.txt` (JSON Lines keyed to the normalized
comment line, tracked in `TODO` §15); a stale baseline entry is a note, and
new citations outside the baseline still fail. `allowlists/house_style_allow.txt`
holds only reviewed false positives.

`layering` enforces the layer rules that a compiler cannot: L1 crates may not
name TVP/TJS/KAG vocabulary, L2 may not carry TVP vocabulary or storage ids,
L3 may not contain platform code, and L3 reaches the engine only through
`crates/compat/src/engine_api.rs` (the `l3-engine-api` rule). It also checks
that every `unsafe` block carries a `// SAFETY:` note and that the file-level
`unsafe_code` allow appears only in reviewed FFI files.

`ci` statically validates the release workflow contract: the `v*` tag
trigger (and the absence of any `android-*` tag), a build-only workflow that
does not rerun the local static gate, independent Linux and Android build jobs,
the Linux archive, the Android build/packaging scripts, and a single final
release job that attaches both artifacts. It reads the workflow as text
(standard library only) and has a fixture self-test under `fixtures/ci/` that
proves every rule fires.

`void-getter` fails a native property getter that returns a `StoredValue`
field whose constructor default is `StoredValue::Void` unchanged: the script
receives a void value, and reading a member from it raises, where the
reference returns an array, a dictionary or a native object. A getter that
matches the void default and returns the reference's non-void value is clean.
A property whose reference getter genuinely reads back void is recorded in
`allowlists/void_getter_allow.txt` as `path:field — reason`; a reason that
starts with `debt:` tracks a known defect and does not silence the finding.

`native-instance-lookup` guards the native-instance ABI seam in
`crates/tjs/cpp/tjs2_abi.cpp`: `tjs2_get_native_instance` must resolve a
live-but-invalidated instance through `GetNativePtrForLookup()`, that accessor
must stay the valid-only class lookup, and the four method/property/constructor
dispatch bodies must keep using the invalidation-guarded `GetNativePtr()`.
Its fixture self-test applies four mutations and requires the matching rule to
fire.

## Environment

- `KRKR_TEST_GAME` — a game directory for the `real_game`/`boot_game` tests.
- `KRKR_TEST_SAVE` — a real KAG save BMP for the save-fixture test.
- `KRKR_REPO` — the checkout root; docs use it where a command starts outside
  the repository.
- `KRKR_REFERENCE` — the reference checkout the gate's checks and the test
  suite read (a clone with a `cpp/` directory and a `tests/test_files`
  directory). It is found the same way everywhere: `KRKR_REFERENCE` when it
  is set, then `<repo>/reference`; the first candidate that holds a `cpp/`
  directory is the checkout. A relative value is resolved against the
  repository root. `krkr-gate --reference PATH <check>` sets it for that run,
  and `krkr-gate --reference PATH gate` hands it to every check the gate
  runs. Without it, a clone at another path makes those four checks fail
  with the paths they tried instead of scanning nothing.
- The reference-dependent tests of the `compat` crate read the same checkout:
  the tests whose name starts with `reference_checkout_` mount the fixtures
  under the checkout's `tests/test_files`, and they fail — naming the
  fixture, the variable and the directory it must point at — when the
  checkout is not there. A test that cannot run never reports a pass. Run
  them with the checkout named:

  ```bash
  KRKR_REFERENCE=/path/to/krkr2 cargo test -p compat \
      --test motion_player --test motion_timing --test env_image
  ```

  The variable must point at the clone root that holds `tests/test_files`
  (and the `cpp/` directory that marks a clone as the checkout).
- `KRKR_CHECK_HASH_ORDER` — set it to `1` (or `true`) to compile the TJS2 hash
  table's additional order list check into the build
  (`KRKR_CHECK_HASH_ORDER=1 cargo build`). The check walks the whole table on
  every operation and aborts the process on the first violation of the list's
  bookkeeping, naming the operation that broke it and the operations that led
  to it. It is off unless the variable is set, and it is a diagnostic to
  switch on while the bookkeeping is investigated, never a setting to leave
  on.

## Notes

- No third-party runtime dependencies; the build backend is hatchling.
- `native-parity` and `plugin-parity` regenerate their reports under `docs/`
  unless `--no-doc` is given; the gate passes `--no-doc` to both, so a gate
  run never rewrites a report — regenerate one deliberately. The reports cite
  reference files as `reference/cpp/...` whatever path the checkout actually
  lives at.
- `surface-lock` regenerates the VM dispatch dump by running the compat
  surface test unless `--no-regen` is given; the gate compares that dump with
  the committed baseline and fails on a change, and `--update` rewrites the
  committed baseline and is the integrator's separate, reviewed action.
