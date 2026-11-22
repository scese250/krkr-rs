//! Integration tests for the `saveStruct` plugin.
//!
//! The plugin attaches structured-serialization members to the TJS2 builtins
//! `Array` (`save2`, `saveStruct2`, `toStructString`) and `Dictionary`
//! (`saveStruct2`, `toStructString`). The `Array` members are ordinary
//! instance members; the `Dictionary` members carry `TJS_STATICMEMBER`, so
//! they live on the class object and an instance cannot reach them. The tests
//! drive the members through TJS against a bound [`CompatHost`] and check both
//! the textual `saveStruct` form and the UTF-16LE files the `save*` members
//! write.
//!
//! The plugin has a second text form, and `Scripts.toStructString` writes it:
//! the tests at the end of this file drive that member and pin its text.
//!
//! Every test serializes on the shared VM lock, since the process-wide
//! engine → context binding is single.

mod common;

use std::path::{Path, PathBuf};

use common::vm_lock;
use compat::CompatHost;
use engine::Storage;
use fs::FileSystem;
use tjs::{Tjs2Engine, TjsValue};

/// A unique temporary game directory, removed on drop.
struct GameDir(PathBuf);

impl GameDir {
    fn new() -> Self {
        let mut path = std::env::temp_dir();
        let unique = format!(
            "krkr-compat-savestruct-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        );
        path.push(unique);
        std::fs::create_dir_all(&path).expect("create temp game dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for GameDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A bound VM with a mounted storage attached to the compat services.
struct Env {
    // Drop order matters: the host removes the process-wide binding before
    // the boxed engine/storage it points at are freed.
    _host: CompatHost,
    engine: Box<Tjs2Engine>,
    _storage: Box<Storage>,
    _dir: GameDir,
}

impl Env {
    fn new() -> Env {
        let dir = GameDir::new();
        let mut storage = Box::new(
            Storage::mount_archives(
                FileSystem::host(dir.path()),
                dir.path().to_path_buf(),
                std::iter::empty::<&str>(),
            )
            .expect("mount storage"),
        );
        // Boxed so the address stays stable when it is moved into `Env`.
        let engine = Box::new(Tjs2Engine::new().expect("create engine"));
        let mut host = CompatHost::new();
        // SAFETY: `storage` is boxed and outlives the binding, which `_host`
        // removes on drop.
        unsafe { host.services_mut().attach_storage(&mut *storage) };
        host.install(&engine).expect("install compat surface");
        common::link_all_builtins(&engine);
        Env {
            _host: host,
            engine,
            _storage: storage,
            _dir: dir,
        }
    }

    fn run(&self, script: &str) {
        self.engine
            .exec_script(script, "save-struct-test")
            .unwrap_or_else(|error| panic!("exec {script:?} failed: {error}"));
    }

    fn eval_ok(&self, expression: &str) -> TjsValue {
        self.engine
            .eval(expression, "save-struct-test")
            .unwrap_or_else(|error| panic!("eval {expression:?} failed: {error}"))
    }

    fn string(&self, expression: &str) -> String {
        match self.eval_ok(expression) {
            TjsValue::String(value) => value,
            other => panic!("{expression:?} returned {other:?}, expected a string"),
        }
    }

    /// The raw bytes of `name` under the temp game directory.
    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self._dir.path().join(name)).expect("read output file")
    }

    /// Decode the raw `tjs_char` stream the plugin writes (UTF-16LE, no BOM).
    fn read_struct(&self, name: &str) -> String {
        let bytes = self.read(name);
        assert_eq!(bytes.len() % 2, 0, "UTF-16LE stream has an odd byte count");
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).expect("output is valid UTF-16LE")
    }

    /// A TJS string literal for an absolute path.
    fn path_literal(&self, name: &str) -> String {
        format!("{:?}", self._dir.path().join(name).to_string_lossy())
    }
}

// ---------------------------------------------------------------------------
// Array members
// ---------------------------------------------------------------------------

// The serialized form comes from `getVariantString`
// (`reference/cpp/plugins/saveStruct.cpp:166-205`): `void` → `void` (`:170-172`),
// `null` → `nullptr` (`:174-176`), integers → `int N` (`:194-197`), reals →
// `real N` (`:200-203`), strings quoted (`:186-188`), and `getArrayString`
// joins elements with commas without newlines (`:142-164`).
#[test]
fn array_to_struct_string_matches_the_reference_format() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run(
        "var a = [\"hello\", 42, 3.5, void, null, [1, 2]]; \
         var s = a.toStructString();",
    );

    assert_eq!(
        env.string("s"),
        "[\"hello\",int 42,real 3.5,void,nullptr,[int 1,int 2]]"
    );
}

// `quoteString` escapes `"` as `\"` and `\` as `\\` inside the double
// quotes (`reference/cpp/plugins/saveStruct.cpp:50-68`).
#[test]
fn array_quotes_and_escapes_strings() {
    let _lock = vm_lock();
    let env = Env::new();

    // A string containing a double quote and a backslash.
    env.run("var a = ['a\"b\\\\c']; var s = a.toStructString();");

    assert_eq!(env.string("s"), "[\"a\\\"b\\\\c\"]");
}

// Octets serialize as `<% xx xx %>` with a trailing space per byte
// (`reference/cpp/plugins/saveStruct.cpp:70-83`, dispatched at `:190-192`).
#[test]
fn array_serializes_octets_as_hex() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var o = <% 00 01 ff 0a %>; var a = [o]; var s = a.toStructString();");

    assert_eq!(env.string("s"), "[<% 00 01 ff 0a %>]");
}

// `Array.save2` writes each element's `GetString()` plus the configured
// newline (`reference/cpp/plugins/saveStruct.cpp:256-257`); the newline flag
// is `param[2]` with CRLF as the default (`:235-236`, constructor at `:25-29`).
#[test]
fn array_save2_writes_strings_with_the_selected_newline() {
    let _lock = vm_lock();
    let env = Env::new();
    let crlf = env.path_literal("crlf.txt");
    let lf = env.path_literal("lf.txt");

    env.run(&format!(
        "var a = [\"alpha\", \"beta\"]; \
         a.save2({crlf}); \
         a.save2({lf}, 0, 1);"
    ));

    assert_eq!(env.read_struct("crlf.txt"), "alpha\r\nbeta\r\n");
    assert_eq!(env.read_struct("lf.txt"), "alpha\nbeta\n");
}

// `Array.saveStruct2` serializes the receiver with `getArrayString`
// (`reference/cpp/plugins/saveStruct.cpp:271-289`) and writes the raw
// `tjs_char` stream.
#[test]
fn array_save_struct2_writes_the_struct_form() {
    let _lock = vm_lock();
    let env = Env::new();
    let path = env.path_literal("array.txt");

    env.run(&format!(
        "var a = [\"x\", 7, [\"y\"]]; a.saveStruct2({path});"
    ));

    assert_eq!(env.read_struct("array.txt"), "[\"x\",int 7,[\"y\"]]");
}

// Relative names resolve against the storage namespace, like the reference's
// `TVPCreateStream(param[0]->AsString(), TJS_BS_WRITE)`
// (`reference/cpp/plugins/saveStruct.cpp:232-233`).
#[test]
fn array_save_struct2_resolves_relative_names_against_the_game_dir() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var a = [\"rel\"]; a.saveStruct2(\"rel_struct.txt\");");
    assert_eq!(env.read_struct("rel_struct.txt"), "[\"rel\"]");
}

// The reference writes the struct form through `TVPCreateStream`, whose
// local file stream creates the missing parent folders of the target before
// the file is created (`tTVPLocalFileStream`,
// `reference/cpp/core/base/impl/StorageImpl.cpp:618-641`); a save into a
// directory that does not exist yet lands there instead of failing.
#[test]
fn array_save_struct2_creates_a_new_subdirectory() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var a = [\"x\", 7]; a.saveStruct2(\"savedata/sub/struct.txt\");");

    assert_eq!(
        env.read_struct("savedata/sub/struct.txt"),
        "[\"x\",int 7]",
        "the save must land in the new subdirectory"
    );
}

// `save2` reads every element with `tTJSVariant::GetString()`, which raises
// the variant-convert error for non-strings (`reference/cpp/plugins/saveStruct.cpp:256`,
// `reference/cpp/core/tjs2/tjsVariant.h:794-798`), so a non-string element
// is a typed error, never a silently converted `"42"`.
#[test]
fn array_save2_rejects_non_string_elements() {
    let _lock = vm_lock();
    let env = Env::new();
    let path = env.path_literal("bad.txt");

    let error = env
        .engine
        .eval(&format!("[1].save2({path})"), "save-struct-test")
        .unwrap_err();
    assert!(
        error.to_string().contains("not a string"),
        "unexpected error: {error}"
    );
}

// ---------------------------------------------------------------------------
// Dictionary serializer, reached through an Array
// ---------------------------------------------------------------------------

// `getVariantString` routes a non-Array object to `getDictString`
// (`reference/cpp/plugins/saveStruct.cpp:174-183`), so a dictionary nested in
// an array is serialized by the same `getDictString` the reference uses;
// `getArrayString` wraps the result in `[`/`]` (`:142-164`). The dictionary
// emits `%[`, comma+newline-separated `"key"=>value` pairs and `]`
// (`:128-137`, separator at `:106-114`). The `Dictionary` members carry
// `TJS_STATICMEMBER` (`:364-366`) and are never copied onto the instance, so
// they must not leak into this enumeration.
#[test]
fn dictionary_to_struct_string_matches_the_reference_format() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run(
        "var d = %[]; d[\"name\"] = \"Kirikiri\"; d[\"count\"] = 3; \
         var s = [d].toStructString();",
    );

    let s = env.string("s");
    assert!(s.starts_with("[%["), "unexpected prefix: {s}");
    assert!(s.ends_with("]]"), "unexpected suffix: {s}");
    assert!(
        s.contains("\"name\"=>\"Kirikiri\""),
        "missing string member: {s}"
    );
    assert!(
        s.contains("\"count\"=>int 3"),
        "missing integer member: {s}"
    );
    // The `Dictionary` members the plugin attaches are static
    // (`reference/cpp/plugins/saveStruct.cpp:364-366`), and the `scripts_ex`
    // helpers are class-level, so neither is copied onto the instance and
    // neither appears in the reference's `EnumMembers` walk.
    assert!(
        !s.contains("getObjectKeys") && !s.contains("saveStruct2") && !s.contains("toStructString"),
        "attached class members leaked into the struct: {s}"
    );
}

// An empty dictionary nested in an array round-trips to `[%[]]`: `getDictString`
// writes `%[`/`]` with no pairs (`reference/cpp/plugins/saveStruct.cpp:128-137`)
// and `getArrayString` wraps it in `[`/`]` (`:142-164`).
#[test]
fn empty_dictionary_serializes_to_empty_brackets() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var d = %[]; var s = [d].toStructString();");
    assert_eq!(env.string("s"), "[%[]]");
}

// The `toStructString` newline flag is `param[0]` of the Array receiver the
// dictionary is nested in (`reference/cpp/plugins/saveStruct.cpp:302`,
// reached through `getArrayString` at `:142-164`): `0` selects CRLF, non-zero
// LF (`:25-29`), and it controls only the dictionary separator — arrays are
// always comma-joined (`:156`).
#[test]
fn dictionary_newline_flag_selects_crlf_or_lf() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run(
        "var d = %[]; d[\"a\"] = 1; d[\"b\"] = 2; \
         var crlf = [d].toStructString(0); \
         var lf = [d].toStructString(1);",
    );

    let crlf = env.string("crlf");
    let lf = env.string("lf");
    assert!(crlf.starts_with("[%["), "unexpected prefix: {crlf}");
    assert!(crlf.ends_with("]]"), "unexpected suffix: {crlf}");
    assert!(crlf.contains("\r\n"), "expected CRLF separator: {crlf:?}");
    assert!(lf.starts_with("[%["), "unexpected prefix: {lf}");
    assert!(lf.ends_with("]]"), "unexpected suffix: {lf}");
    assert!(lf.contains('\n'), "expected LF separator: {lf:?}");
    assert!(!lf.contains("\r\n"), "LF mode must not emit CRLF: {lf:?}");
}

// Nested arrays recurse through `getArrayString`
// (`reference/cpp/plugins/saveStruct.cpp:178-180`) and nested dictionaries
// through `getDictString` (`:181-183`); `getArrayString` wraps the outer
// dictionary in `[`/`]` (`:142-164`).
#[test]
fn dictionary_serializes_nested_arrays_and_dictionaries() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run(
        "var inner = %[]; inner[\"z\"] = null; \
         var d = %[]; d[\"list\"] = [1, \"two\"]; d[\"dict\"] = inner; \
         var s = [d].toStructString();",
    );

    let s = env.string("s");
    assert!(s.starts_with("[%["), "unexpected prefix: {s}");
    assert!(s.ends_with("]]"), "unexpected suffix: {s}");
    assert!(
        s.contains("\"list\"=>[int 1,\"two\"]"),
        "missing nested array: {s}"
    );
    assert!(
        s.contains("\"dict\"=>%[\"z\"=>nullptr]"),
        "missing nested dict: {s}"
    );
}

// `Array.saveStruct2` serializes its receiver with `getArrayString`
// (`reference/cpp/plugins/saveStruct.cpp:271-289`), which reaches
// `getDictString` for the nested dictionary; the `Dictionary` member itself
// is static (`:364-366`) and is not the receiver here.
#[test]
fn dictionary_save_struct2_writes_the_struct_form() {
    let _lock = vm_lock();
    let env = Env::new();
    let path = env.path_literal("dict.txt");

    env.run(&format!(
        "var d = %[]; d[\"k\"] = \"v\"; [d].saveStruct2({path});"
    ));

    assert_eq!(env.read_struct("dict.txt"), "[%[\"k\"=>\"v\"]]");
}

// ---------------------------------------------------------------------------
// Attach surface
// ---------------------------------------------------------------------------

// The reference attaches `save2`/`saveStruct2`/`toStructString` to `Array`
// without a static flag (`reference/cpp/plugins/saveStruct.cpp:310-313`, via
// `ncbAttachTJS2Class::RegistVariant`,
// `reference/cpp/core/plugin/ncbind.hpp:2031-2032`), so they live on the
// `Array` class object and the creation callback copies them onto every array
// instance because they are not `TJS_STATICMEMBER`
// (`reference/cpp/core/tjs2/tjsNative.cpp:346-360`). The reference attaches
// `saveStruct2`/`toStructString` to `Dictionary` with `TJS_STATICMEMBER`
// (`saveStruct.cpp:364-366`), which stores them on the class object and sets
// the symbol's static bit
// (`reference/cpp/core/tjs2/tjsObject.cpp:1390-1391`); the creation callback
// skips them, so an instance never has them.
#[test]
fn members_are_reachable_on_the_builtins_like_the_reference() {
    let _lock = vm_lock();
    let env = Env::new();

    // Array members are reachable on the class and on an instance.
    assert_eq!(env.string("typeof Array.save2"), "Object");
    assert_eq!(env.string("typeof Array.saveStruct2"), "Object");
    assert_eq!(env.string("typeof Array.toStructString"), "Object");
    assert_eq!(env.string("typeof [1].save2"), "Object");
    assert_eq!(env.string("typeof [1].saveStruct2"), "Object");
    assert_eq!(env.string("[1].toStructString()"), "[int 1]");

    // Dictionary members are reachable on the class only.
    env.run("var d = %[]; d[\"k\"] = 1;");
    assert_eq!(env.string("typeof Dictionary.saveStruct2"), "Object");
    assert_eq!(env.string("typeof Dictionary.toStructString"), "Object");
    assert_eq!(env.string("typeof d.saveStruct2"), "undefined");
    assert_eq!(env.string("typeof d.toStructString"), "undefined");

    // An unrelated builtin is untouched: `String` has no such member.
    let error = env
        .engine
        .eval("\"x\".toStructString()", "save-struct-test")
        .unwrap_err();
    assert!(
        !error.to_string().is_empty(),
        "expected an error for a String receiver"
    );
}

// The reference registers only the Array members
// (`reference/cpp/plugins/saveStruct.cpp:310-314`) and the Dictionary members
// (`:364-367`); it exposes no free functions. The port therefore retains its
// serializer helpers internally and removes every script global the
// registration ABI created for them. The helpers live on the global object,
// so their absence is observed through `typeof global.<name>`; a bare
// `typeof <name>` is a must-exist read that raises for the same absence
// (`reference/cpp/core/tjs2/tjsInterCodeExec.cpp:1899-1904`, pinned by
// `crates/compat/tests/typeof_spellings.rs`).
#[test]
fn serializer_helpers_are_not_globals() {
    let _lock = vm_lock();
    let env = Env::new();

    for name in [
        "__krkr_save_struct_quote",
        "__krkr_save_struct_quote_octet",
        "__krkr_save_struct_is_array",
        "__krkr_save_struct_variant",
        "__krkr_save_struct_arr",
        "__krkr_save_struct_dict",
        "__krkr_save_struct_serialize",
    ] {
        assert_eq!(
            env.string(&format!("typeof global.{name}")),
            "undefined",
            "{name} must not be a global"
        );
        // A bare `typeof` is a must-exist read, so it raises for the absent
        // name; a leaked global would report `"Object"` instead.
        assert!(
            env.engine
                .eval(&format!("typeof {name}"), "save-struct-test")
                .is_err(),
            "{name} must not be reachable as a bare global"
        );
    }
}

// `DictAdd` attaches only `saveStruct2` and `toStructString`, both with
// `TJS_STATICMEMBER` (`reference/cpp/plugins/saveStruct.cpp:364-366`); `save2`
// is an Array-only member (`:310-313`). `typeof` on a missing member reports
// `undefined` without raising
// (`reference/cpp/core/tjs2/tjsInterCodeExec.cpp:1905-1910`), and the static
// members are not copied onto a `Dictionary` instance
// (`reference/cpp/core/tjs2/tjsNative.cpp:346-360`). Both `save2` and
// `saveStruct2` are reachable as Array instance members (`saveStruct.cpp:310-313`).
#[test]
fn dictionary_has_no_save2_member() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run(
        "var d = %[]; \
         var dictSave2 = (typeof d.save2 == \"undefined\") ? 1 : 0; \
         var dictSaveStruct2 = (typeof d.saveStruct2 == \"undefined\") ? 1 : 0; \
         var arrSave2 = (typeof [1].save2 == \"undefined\") ? 0 : 1; \
         var arrSaveStruct2 = (typeof [1].saveStruct2 == \"undefined\") ? 0 : 1;",
    );
    assert_eq!(env.eval_ok("dictSave2"), TjsValue::Integer(1));
    assert_eq!(env.eval_ok("dictSaveStruct2"), TjsValue::Integer(1));
    assert_eq!(env.eval_ok("arrSave2"), TjsValue::Integer(1));
    assert_eq!(env.eval_ok("arrSaveStruct2"), TjsValue::Integer(1));
}

// The file writers require at least the filename — `TJS_E_BADPARAMCOUNT`
// (`reference/cpp/plugins/saveStruct.cpp:230`, `:273`, `:333`) — while
// `toStructString` takes its newline flag optionally (`:297-301`) and must
// succeed with no arguments at all.
#[test]
fn save_members_require_a_filename_but_to_struct_string_does_not() {
    let _lock = vm_lock();
    let env = Env::new();

    for call in [
        "[1].save2()".to_string(),
        "[1].saveStruct2()".to_string(),
        // The Dictionary writer is reachable on the class object; the
        // argument check runs before the receiver is used.
        "Dictionary.saveStruct2()".to_string(),
    ] {
        let error = env
            .engine
            .eval(&call, "save-struct-test")
            .err()
            .unwrap_or_else(|| panic!("{call} should have raised"));
        assert!(!error.to_string().is_empty(), "{call}: empty error");
    }

    // No-argument `toStructString` is legal and defaults to CRLF mode.
    assert_eq!(env.string("[1].toStructString()"), "[int 1]");
}

// The file writers read the newline flag from `param[2]` with `param[1]`
// (the `utf` flag) deliberately unused — the commented-out reference code
// (`reference/cpp/plugins/saveStruct.cpp:234-236`) — so `save2(path, 1)`
// still writes CRLF and the `utf` argument does not switch the encoding.
#[test]
fn save2_ignores_the_utf_flag_and_keeps_crlf() {
    let _lock = vm_lock();
    let env = Env::new();
    let path = env.path_literal("utf_flag.txt");

    env.run(&format!("var a = [\"x\"]; a.save2({path}, 1);"));

    assert_eq!(env.read_struct("utf_flag.txt"), "x\r\n");
}

// `Array.saveStruct2` writes the nested dictionary's separator as `,` plus
// the newline selected by `param[2]` (`reference/cpp/plugins/saveStruct.cpp:106-114`,
// flag at `:279`): LF mode must not contain a CRLF anywhere in the output.
#[test]
fn dictionary_save_struct2_file_newline_flag_selects_lf() {
    let _lock = vm_lock();
    let env = Env::new();
    let lf = env.path_literal("dict_lf.txt");
    let crlf = env.path_literal("dict_crlf.txt");

    env.run(&format!(
        "var d = %[]; d[\"a\"] = 1; d[\"b\"] = 2; \
         [d].saveStruct2({lf}, 0, 1); \
         [d].saveStruct2({crlf}, 0, 0);"
    ));

    let lf_text = env.read_struct("dict_lf.txt");
    assert!(lf_text.starts_with("[%["), "unexpected prefix: {lf_text:?}");
    assert!(lf_text.ends_with("]]"), "unexpected suffix: {lf_text:?}");
    assert!(lf_text.contains('\n'), "expected LF separator: {lf_text:?}");
    assert!(
        !lf_text.contains("\r\n"),
        "LF mode must not emit CRLF: {lf_text:?}"
    );
    let crlf_text = env.read_struct("dict_crlf.txt");
    assert!(
        crlf_text.starts_with("[%["),
        "unexpected prefix: {crlf_text:?}"
    );
    assert!(
        crlf_text.ends_with("]]"),
        "unexpected suffix: {crlf_text:?}"
    );
    assert!(
        crlf_text.contains("\r\n"),
        "expected CRLF separator: {crlf_text:?}"
    );
}

// An empty array serializes to `[]` (`reference/cpp/plugins/saveStruct.cpp:142-164`:
// the loop never runs and only the brackets are written).
#[test]
fn empty_array_serializes_to_brackets() {
    let _lock = vm_lock();
    let env = Env::new();

    assert_eq!(env.string("[].toStructString()"), "[]");
}

// ---------------------------------------------------------------------------
// `Scripts.toStructString` — the plugin's second text form
// ---------------------------------------------------------------------------

// The upstream attaches `Scripts.toStructString(target [, newline [, option]])`
// to the `Scripts` class object as a static member (`Main.cpp:381-383`), and
// the member serializes the target into the plugin's structural text form
// (`Main.cpp:184-208`). A dictionary is written as `%[` … `]`
// (`Main.cpp:150-159`) with the entries separated by `,` and the newline
// (`Main.cpp:118-119`), and the member order is the VM's own enumeration
// order.
//
// The dictionary literal of this engine spells an entry `name: value`
// (`%[a: 1]`); the upstream's own literal spelling does not parse here, and
// the members it names are the same.
#[test]
fn scripts_to_struct_string_writes_the_dictionary_form() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var empty = Scripts.toStructString(%[]);");
    assert_eq!(env.string("empty"), "%[]");

    env.run("var pair = Scripts.toStructString(%[a: 1, b: \"x\"]);");
    assert_eq!(env.string("pair"), "%[\"a\"=>int 1,\n\"b\"=>\"x\"]");
}

// The indent bit (`Main.cpp:116`, `Main.cpp:144`; the writer
// `json/Writer.hpp:30-45`) breaks a container across lines: the first entry
// starts on a new line one space deeper than the container, the entries
// stay separated by `,` and the newline, and the closing bracket stands
// alone at the container's own level.
#[test]
fn scripts_to_struct_string_indents_the_containers() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var flat = Scripts.toStructString(%[a: 1, b: \"x\"], 1, 1);");
    assert_eq!(env.string("flat"), "%[\n \"a\"=>int 1,\n \"b\"=>\"x\"\n]");

    env.run("var nested = Scripts.toStructString(%[a: %[b: 1]], 1, 1);");
    assert_eq!(
        env.string("nested"),
        "%[\n \"a\"=>%[\n  \"b\"=>int 1\n ]\n]"
    );
}

// An array is written as `[` … `]` (`Main.cpp:162-182`): the elements are
// separated by `,` (`Main.cpp:171-174`), and the indent breaks the array
// across lines unless the array-ignores-indent bit is set
// (`Main.cpp:168`, `Main.cpp:180`). An empty container is never broken
// across lines (`Main.cpp:144`, `Main.cpp:168`).
#[test]
fn scripts_to_struct_string_writes_the_array_form() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var flat = Scripts.toStructString([1, 2]);");
    assert_eq!(env.string("flat"), "[int 1,int 2]");

    env.run("var indented = Scripts.toStructString([1, 2], 1, 1);");
    assert_eq!(env.string("indented"), "[\n int 1,\n int 2\n]");

    env.run("var one_line = Scripts.toStructString([1, 2], 1, 1 | 2048);");
    assert_eq!(env.string("one_line"), "[int 1,int 2]");

    env.run("var empty = Scripts.toStructString([]);");
    assert_eq!(env.string("empty"), "[]");
}

// The const bit (`Main.cpp:202`, `Main.cpp:150`, `Main.cpp:164`) writes
// `(const)` in front of every container and drops the `int ` and `real `
// type prefixes at every depth; the type-prefix bit (`Main.cpp:202`) drops
// the prefixes without the `(const)`.
#[test]
fn scripts_to_struct_string_writes_the_const_form() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var constant = Scripts.toStructString(%[a: 1], 1, 2);");
    assert_eq!(env.string("constant"), "(const)%[\"a\"=>1]");

    env.run("var no_prefix = Scripts.toStructString([1, 2], 1, 1024);");
    assert_eq!(env.string("no_prefix"), "[1,2]");
}

// The sort bit (`Main.cpp:99-101`, `Main.cpp:130-143`) writes a
// dictionary's entries in ascending name order; without it the entries keep
// the VM's own member order, which for `%[b: 2, a: 1, c: 3]` puts `c`
// first.
#[test]
fn scripts_to_struct_string_sorts_the_entries() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var sorted = Scripts.toStructString(%[b: 1, a: \"x\"], 1, 4);");
    assert_eq!(env.string("sorted"), "%[\"a\"=>\"x\",\n\"b\"=>int 1]");

    env.run("var unsorted = Scripts.toStructString(%[b: 2, a: 1, c: 3]);");
    assert_eq!(
        env.string("unsorted"),
        "%[\"c\"=>int 3,\n\"a\"=>int 1,\n\"b\"=>int 2]"
    );

    env.run("var ordered = Scripts.toStructString(%[b: 2, a: 1, c: 3], 1, 4);");
    assert_eq!(
        env.string("ordered"),
        "%[\"a\"=>int 1,\n\"b\"=>int 2,\n\"c\"=>int 3]"
    );
}

// The option argument defaults to `0` when it is absent (`Main.cpp:374`):
// the default form is neither indented nor sorted. The upstream's manual
// declares the default as `ssoIndent|ssoSort` (`manual.tjs:57`), but the
// implementation passes `0`, so the entries keep the VM's member order.
#[test]
fn scripts_to_struct_string_defaults_to_no_options() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var text = Scripts.toStructString(%[b: 2, a: 1, c: 3]);");
    assert_eq!(
        env.string("text"),
        "%[\"c\"=>int 3,\n\"a\"=>int 1,\n\"b\"=>int 2]"
    );
}

// The value texts (`Main.cpp:200-207`): the type prefix is `int ` or `real `
// (`Main.cpp:203-204`) and the expression text is the VM's own
// (`Main.cpp:206`; the engine's VM
// `crates/tjs/cpp/tjs2/tjsUtils.cpp:174-232`). A `null` value is `null`
// (`Main.cpp:192-193`) and a `void` value is `void`. The real's text is the
// hexadecimal form and the decimal form in a comment (the engine's VM
// `crates/tjs/cpp/tjs2/tjsVariant.cpp:219-297`), the string's text is the
// C-escaped content (the engine's VM
// `crates/tjs/cpp/tjs2/tjsString.cpp:164-231`) and the octet's text is the
// uppercase hexadecimal bytes (the engine's VM
// `crates/tjs/cpp/tjs2/tjsVariant.cpp:114-140`).
#[test]
fn scripts_to_struct_string_writes_the_value_texts() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var realValue = Scripts.toStructString(1.5);");
    assert_eq!(
        env.string("realValue"),
        "real 0x1.8000000000000p0 /* 1.5 */"
    );

    env.run("var one = Scripts.toStructString(1.0);");
    assert_eq!(env.string("one"), "real 0x1.0000000000000p0 /* 1 */");

    env.run("var quote = Scripts.toStructString(\"a\\\"b\");");
    assert_eq!(env.string("quote"), "\"a\\\"b\"");

    env.run("var line = Scripts.toStructString(\"a\\nb\");");
    assert_eq!(env.string("line"), "\"a\\nb\"");

    env.run("var octetValue = Scripts.toStructString(<% 00 ff %>);");
    assert_eq!(env.string("octetValue"), "<% 00 FF %>");

    env.run("var nothing = Scripts.toStructString(null);");
    assert_eq!(env.string("nothing"), "null");

    env.run("var voided = Scripts.toStructString(void);");
    assert_eq!(env.string("voided"), "void");

    env.run("var negative = Scripts.toStructString(-42);");
    assert_eq!(env.string("negative"), "int -42");
}

// The newline selector (`Main.cpp:372`; the writer `json/Writer.hpp:12-23`):
// the selector `1` is LF and every other value is CRLF. The selector
// defaults to `1` when the argument is absent (`Main.cpp:372`).
#[test]
fn scripts_to_struct_string_selects_the_newline() {
    let _lock = vm_lock();
    let env = Env::new();

    env.run("var lf = Scripts.toStructString(%[a: 1, b: 2]);");
    assert_eq!(env.string("lf"), "%[\"a\"=>int 1,\n\"b\"=>int 2]");

    env.run("var crlf = Scripts.toStructString(%[a: 1, b: 2], 0);");
    assert_eq!(env.string("crlf"), "%[\"a\"=>int 1,\r\n\"b\"=>int 2]");

    env.run("var other = Scripts.toStructString(%[a: 1, b: 2], 2);");
    assert_eq!(env.string("other"), "%[\"a\"=>int 1,\r\n\"b\"=>int 2]");
}

// The member needs a target (`Main.cpp:370`).
#[test]
fn scripts_to_struct_string_requires_a_target() {
    let _lock = vm_lock();
    let env = Env::new();

    let error = env
        .engine
        .eval("Scripts.toStructString()", "save-struct-test")
        .expect_err("the call without a target must raise");
    assert!(
        error.to_string().contains("requires 1 argument"),
        "unexpected error: {error}"
    );
}

// The newline and the option are read as integers (`Main.cpp:372`,
// `Main.cpp:374`); the VM's integer conversion raises for an object and for
// an octet (the engine's VM
// `crates/tjs/cpp/tjs2/tjsVariant.h:864-884`).
#[test]
fn scripts_to_struct_string_rejects_a_non_numeric_argument() {
    let _lock = vm_lock();
    let env = Env::new();

    for call in [
        "Scripts.toStructString(%[a: 1], %[])",
        "Scripts.toStructString(%[a: 1], 1, %[])",
    ] {
        let error = env
            .engine
            .eval(call, "save-struct-test")
            .err()
            .unwrap_or_else(|| panic!("{call} should have raised"));
        assert!(!error.to_string().is_empty(), "{call}: empty error");
    }
}

// The hidden-member bit (`Main.cpp:98`) asks for the members a class marks
// hidden. The engine's member enumeration does not report them, so the
// member raises on the bit instead of accepting it and writing the same
// text as without it.
#[test]
fn scripts_to_struct_string_rejects_the_hidden_option() {
    let _lock = vm_lock();
    let env = Env::new();

    let error = env
        .engine
        .eval("Scripts.toStructString(%[a: 1], 1, 8)", "save-struct-test")
        .expect_err("the hidden-member bit must raise");
    assert!(
        error.to_string().contains("hidden"),
        "unexpected error: {error}"
    );
}

// The upstream attaches the member to the `Scripts` class object
// (`Main.cpp:381-383`), so the class carries it. The traversal helper lives
// on a namespace the plugin removes from the global object
// (`Main.cpp:411-421`), so the script sees no helper function.
#[test]
fn the_scripts_member_is_a_static_member_without_leaking_helpers() {
    let _lock = vm_lock();
    let env = Env::new();

    assert_eq!(env.string("typeof Scripts.toStructString"), "Object");

    for name in [
        "__krkr_save_struct_scripts_ns",
        "__krkr_save_struct_scripts_kind",
        "__krkr_save_struct_scripts_push_node",
        "__krkr_save_struct_scripts_push_array",
        "__krkr_save_struct_scripts_push_dict",
        "__krkr_save_struct_scripts_tree",
        "__krkr_save_struct_scripts_is_array",
    ] {
        assert_eq!(
            env.string(&format!("typeof global.{name}")),
            "undefined",
            "{name} must not be a global"
        );
    }
}
