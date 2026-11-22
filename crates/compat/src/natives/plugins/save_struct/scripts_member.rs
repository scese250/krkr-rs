//! `Scripts.toStructString` — the plugin's second text form.
//!
//! The plugin has two revisions, and the engine carries the members of both:
//! the `Array` and `Dictionary` members write the form of the first revision
//! ([`super::SERIALIZER_SCRIPT`]), and this member writes the form of the
//! second one — the revision that added the `Scripts` member and the
//! formatting flags. The two forms read differently (a `null` value, a real,
//! an octet and a string all have their own text) and lay out differently
//! (the newline default and the formatting flags), so each member keeps the
//! form of the revision it belongs to and the two are never mixed.
//!
//! # The form
//!
//! * a `null` value is `null` and a `void` value is `void`;
//! * an integer is `int 42` and a real is
//!   `real 0x1.8000000000000p0 /* 1.5 */`;
//! * a string is quoted and escaped in full (`\a`, `\b`, `\f`, `\n`, `\r`,
//!   `\t`, `\v`, `\\`, `\'`, `\"` and `\xNN` for the control characters);
//! * an octet is `<% 00 FF %>`;
//! * an object is `%["name"=>value]` and an array is `[value]`;
//! * a dictionary's entries are separated by `,` and the newline, and the
//!   newline is LF when the newline argument is exactly `1` and CRLF for
//!   every other value;
//! * the `option` argument's bits pick the layout: bit `1` indents the
//!   containers, bit `2` writes `(const)` and drops the type prefixes, bit
//!   `4` sorts a dictionary's entries by name, bit `1024` drops the type
//!   prefixes, and bit `2048` keeps an array on one line.
//!
//! # The traversal
//!
//! The members are read through the VM's own enumeration (the builtin
//! `Array.assign` walk), so the member order and the hidden-member
//! filtering are the VM's. The walk runs in the helper script
//! [`HELPER_SCRIPT`]: it classifies every value and pushes it into a token
//! stream, and this module turns the stream into the text. The token stream
//! is a flat array — a container's marker, its entry count, and then one
//! entry per member or element — so the Rust side never has to hold a
//! script object.
#![allow(unsafe_code)]

use std::cmp::Ordering;
use std::ffi::{c_char, c_int, c_void};

use tjs::{
    DetachedValue, MemberRead, NativeObjThisMethodDef, RetainedValue, Tjs2Engine, Tjs2ValueId,
    TjsValue, VAL_INTEGER, VAL_NULL, VAL_OBJECT, VAL_OCTET, VAL_REAL, VAL_STRING, VAL_VOID, Value,
};

use super::super::super::system::{resolve, set_string_out, try_value_as_i64};
use crate::native_util::{args, report_error, value_as_string};
use crate::{CompatCtx, CompatError};

/// The formatting bit that indents the containers.
const SSO_INDENT: i64 = 1;

/// The formatting bit that writes `(const)` and drops the type prefixes.
const SSO_CONST: i64 = 2;

/// The formatting bit that sorts a dictionary's entries by name.
const SSO_SORT: i64 = 4;

/// The formatting bit that writes the hidden members.
///
/// The engine's member enumeration does not report the hidden members, so
/// the member raises on this bit instead of accepting it and writing
/// nothing different.
const SSO_HIDDEN: i64 = 8;

/// The formatting bit that drops the type prefixes.
const SSO_NO_TYPE_PREFIX: i64 = 1024;

/// The formatting bit that keeps an array on one line.
const SSO_ARRAY_NO_INDENT: i64 = 2048;

/// Name of the per-engine namespace dictionary holding the traversal helper.
const NAMESPACE_NAME: &str = "__krkr_save_struct_scripts_ns";

/// Name of the helper member: it returns the token stream of a value.
const TREE_MEMBER: &str = "__krkr_save_struct_scripts_tree";

/// The traversal helper.
///
/// The helper walks a value and pushes it into a flat token stream:
///
/// * a container pushes its marker (`dict` or `array`), its entry count and
///   then one entry per member or element;
/// * an entry is the member name (empty for an array element), the value's
///   kind (`void`, `null`, `int`, `real`, `string`, `octet`, `dict` or
///   `array`) and, for a scalar, the value itself;
/// * an element that cannot be read is pushed as the kind `skip`.
///
/// The member names come from the VM's own enumeration: the builtin
/// `Array.assign` walk, which reports the members in the VM's order and
/// skips the hidden ones. A member's value is read with the ordinary
/// member read, so a member that is a TJS `property` contributes the value
/// its getter returns.
const HELPER_SCRIPT: &str = r#"
var __krkr_save_struct_scripts_ns = %[];

__krkr_save_struct_scripts_ns.__krkr_save_struct_scripts_is_array = function(v) {
    try {
        return v instanceof "Array";
    } catch (e) {
        return false;
    }
};

__krkr_save_struct_scripts_ns.__krkr_save_struct_scripts_kind = function(v) {
    if (v === null) return "null";
    var t = typeof v;
    if (t == "void") return "void";
    if (t == "Integer") return "int";
    if (t == "Real") return "real";
    if (t == "String") return "string";
    if (t == "Octet") return "octet";
    if (this.__krkr_save_struct_scripts_is_array(v)) return "array";
    return "object";
};

__krkr_save_struct_scripts_ns.__krkr_save_struct_scripts_push_node = function(v, out) {
    var kind = this.__krkr_save_struct_scripts_kind(v);
    if (kind == "array") {
        out.push("array");
        this.__krkr_save_struct_scripts_push_array(v, out);
        return;
    }
    if (kind == "object") {
        out.push("dict");
        this.__krkr_save_struct_scripts_push_dict(v, out);
        return;
    }
    out.push(kind);
    if (kind == "int" || kind == "real" || kind == "string" || kind == "octet") {
        out.push(v);
    }
};

__krkr_save_struct_scripts_ns.__krkr_save_struct_scripts_push_array = function(v, out) {
    var count = v.count;
    out.push(count);
    var i = 0;
    while (i < count) {
        var element = null;
        var readable = true;
        try {
            element = v[i];
        } catch (e) {
            readable = false;
        }
        out.push("");
        if (readable) {
            this.__krkr_save_struct_scripts_push_node(element, out);
        } else {
            out.push("skip");
        }
        i = i + 1;
    }
};

__krkr_save_struct_scripts_ns.__krkr_save_struct_scripts_push_dict = function(v, out) {
    var pairs = [];
    pairs.assign(v);
    out.push(pairs.count >> 1);
    var i = 0;
    while (i < pairs.count) {
        out.push(pairs[i]);
        this.__krkr_save_struct_scripts_push_node(v[pairs[i]], out);
        i = i + 2;
    }
};

__krkr_save_struct_scripts_ns.__krkr_save_struct_scripts_tree = function(v) {
    var out = [];
    this.__krkr_save_struct_scripts_push_node(v, out);
    return out;
};
"#;

/// Retained id of the helper namespace.
///
/// The namespace is built as the VM compiles the helper, then retained and
/// its global binding deleted. The engine's retained map dies with the VM,
/// so the id is deliberately leaked on the Rust side — dropping the engine
/// releases the retention.
#[derive(Default)]
struct ScriptsState {
    namespace: Option<Tjs2ValueId>,
}

// ---------------------------------------------------------------------------
// The text
// ---------------------------------------------------------------------------

/// The formatting options the `option` argument selects.
struct Options {
    /// The newline the entries are separated with.
    newline: &'static str,
    /// Whether the containers are broken across lines.
    indent: bool,
    /// Whether every container is prefixed with `(const)`.
    constant: bool,
    /// Whether a dictionary's entries are sorted by name.
    sort: bool,
    /// Whether the `int ` and `real ` type prefixes are dropped.
    no_type_prefix: bool,
    /// Whether an array ignores the indent.
    array_no_indent: bool,
}

impl Options {
    /// The options the newline and option arguments select.
    fn new(newline: i64, option: i64) -> Options {
        Options {
            // The newline is LF for the selector `1` and CRLF for every
            // other value, including `0`.
            newline: if newline == 1 { "\n" } else { "\r\n" },
            indent: option & SSO_INDENT != 0,
            constant: option & SSO_CONST != 0,
            sort: option & SSO_SORT != 0,
            no_type_prefix: option & (SSO_CONST | SSO_NO_TYPE_PREFIX) != 0,
            array_no_indent: option & SSO_ARRAY_NO_INDENT != 0,
        }
    }
}

/// A value of the token stream.
enum Node {
    /// A scalar value.
    Scalar(Scalar),
    /// A dictionary: the member names and their values.
    Dict(Vec<(String, Node)>),
    /// An array: its elements.
    Array(Vec<Node>),
    /// An element that could not be read.
    Skip,
}

/// The scalar values of the token stream.
enum Scalar {
    /// The `void` value.
    Void,
    /// The `null` value.
    Null,
    /// An integer.
    Integer(i64),
    /// A real.
    Real(f64),
    /// A string.
    Text(String),
    /// An octet.
    Octet(Vec<u8>),
}

/// The hexadecimal digits the octet and the real forms use.
const HEX_DIGITS: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
];

/// Write a value's text.
fn write_node(text: &mut String, node: &Node, options: &Options, level: usize) {
    match node {
        Node::Skip => {}
        Node::Scalar(scalar) => write_scalar(text, scalar, options),
        Node::Dict(entries) => write_dict(text, entries, options, level),
        Node::Array(elements) => write_array(text, elements, options, level),
    }
}

/// Write a scalar value's text.
fn write_scalar(text: &mut String, scalar: &Scalar, options: &Options) {
    match scalar {
        Scalar::Void => text.push_str("void"),
        Scalar::Null => text.push_str("null"),
        Scalar::Integer(value) => {
            write_type_prefix(text, options, "int ");
            text.push_str(&value.to_string());
        }
        Scalar::Real(value) => {
            write_type_prefix(text, options, "real ");
            text.push_str(&real_expression_text(*value));
        }
        Scalar::Text(value) => text.push_str(&quote_c(value)),
        Scalar::Octet(bytes) => text.push_str(&octet_text(bytes)),
    }
}

/// Write the `int ` or `real ` type prefix, unless the options drop it.
fn write_type_prefix(text: &mut String, options: &Options, prefix: &str) {
    if !options.no_type_prefix {
        text.push_str(prefix);
    }
}

/// Write a dictionary's text.
///
/// The entries are separated by `,` and the newline. The indent breaks the
/// dictionary across lines: the first entry starts on a new line one space
/// deeper than the dictionary, and the closing bracket stands alone at the
/// dictionary's own level.
fn write_dict(text: &mut String, entries: &[(String, Node)], options: &Options, level: usize) {
    let mut entries: Vec<&(String, Node)> = entries
        .iter()
        .filter(|(_, node)| !matches!(node, Node::Skip))
        .collect();
    if options.sort {
        entries.sort_by(|left, right| compare_names(&left.0, &right.0));
    }
    if options.constant {
        text.push_str("(const)");
    }
    text.push_str("%[");
    let indented = options.indent && !entries.is_empty();
    let mut level = level;
    if indented {
        level += 1;
        write_newline(text, options, level);
    }
    let mut first = true;
    for (name, node) in entries {
        if !first {
            text.push(',');
            write_newline(text, options, level);
        }
        first = false;
        text.push_str(&quote_c(name));
        text.push_str("=>");
        write_node(text, node, options, level);
    }
    if indented {
        level -= 1;
        write_newline(text, options, level);
    }
    text.push(']');
}

/// Write an array's text.
///
/// The elements are separated by `,`. The indent breaks the array across
/// lines, unless the options keep an array on one line.
fn write_array(text: &mut String, elements: &[Node], options: &Options, level: usize) {
    let elements: Vec<&Node> = elements
        .iter()
        .filter(|node| !matches!(node, Node::Skip))
        .collect();
    if options.constant {
        text.push_str("(const)");
    }
    text.push('[');
    let indented = options.indent && !options.array_no_indent && !elements.is_empty();
    let mut level = level;
    if indented {
        level += 1;
        write_newline(text, options, level);
    }
    let mut first = true;
    for element in elements {
        if !first {
            text.push(',');
            if indented {
                write_newline(text, options, level);
            }
        }
        first = false;
        write_node(text, element, options, level);
    }
    if indented {
        level -= 1;
        write_newline(text, options, level);
    }
    text.push(']');
}

/// Write the newline and the spaces of the level.
fn write_newline(text: &mut String, options: &Options, level: usize) {
    text.push_str(options.newline);
    for _ in 0..level {
        text.push(' ');
    }
}

/// Compare two member names the way the VM compares its strings: by UTF-16
/// code unit, so a name above the basic multilingual plane sorts before a
/// name in the private use area.
fn compare_names(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// Write a string's quoted text.
///
/// The escaping covers the C escapes and writes a control character as
/// `\xNN`. A hexadecimal digit that follows such an escape is written as an
/// escape of its own, so the text cannot swallow it.
fn quote_c(value: &str) -> String {
    let mut text = String::with_capacity(value.len() + 2);
    text.push('"');
    let mut hex_escape = false;
    for character in value.chars() {
        match character {
            '\u{07}' => {
                text.push_str("\\a");
                hex_escape = false;
            }
            '\u{08}' => {
                text.push_str("\\b");
                hex_escape = false;
            }
            '\u{0c}' => {
                text.push_str("\\f");
                hex_escape = false;
            }
            '\u{0a}' => {
                text.push_str("\\n");
                hex_escape = false;
            }
            '\u{0d}' => {
                text.push_str("\\r");
                hex_escape = false;
            }
            '\u{09}' => {
                text.push_str("\\t");
                hex_escape = false;
            }
            '\u{0b}' => {
                text.push_str("\\v");
                hex_escape = false;
            }
            '\\' => {
                text.push_str("\\\\");
                hex_escape = false;
            }
            '\'' => {
                text.push_str("\\'");
                hex_escape = false;
            }
            '"' => {
                text.push_str("\\\"");
                hex_escape = false;
            }
            _ => {
                if hex_escape && character.is_ascii_hexdigit() {
                    text.push_str(&hex_escape_text(character));
                    continue;
                }
                if (character as u32) < 0x20 {
                    text.push_str(&hex_escape_text(character));
                    hex_escape = true;
                } else {
                    text.push(character);
                    hex_escape = false;
                }
            }
        }
    }
    text.push('"');
    text
}

/// Write the `\xNN` escape of a character.
fn hex_escape_text(character: char) -> String {
    format!("\\x{:02x}", character as u32)
}

/// Write an octet's text: the bytes as uppercase hexadecimal, separated by
/// single spaces.
fn octet_text(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 3 + 4);
    text.push_str("<% ");
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        text.push(HEX_DIGITS[usize::from(*byte >> 4)]);
        text.push(HEX_DIGITS[usize::from(*byte & 0x0f)]);
    }
    text.push_str(" %>");
    text
}

/// Write a real's text: the hexadecimal form, the decimal form and the
/// comment that carries the decimal one.
fn real_expression_text(value: f64) -> String {
    format!(
        "{} /* {} */",
        real_hex_text(value),
        real_decimal_text(value)
    )
}

/// Write a real's hexadecimal form.
///
/// The form is the sign, `0x1.`, the significand as thirteen hexadecimal
/// digits and the exponent in base ten. The special values keep their own
/// text.
fn real_hex_text(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity".to_string()
        } else {
            "+Infinity".to_string()
        };
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "+0.0".to_string()
        };
    }
    let bits = value.to_bits();
    let mut text = String::with_capacity(24);
    let sign = bits >> 63;
    if sign == 1 {
        text.push('-');
    }
    text.push_str("0x1.");
    let significand = bits & 0x000f_ffff_ffff_ffff;
    for shift in (0u32..13).rev() {
        let digit = significand >> (shift * 4);
        let digit = digit & 0xf;
        text.push(HEX_DIGITS[digit as usize]);
    }
    text.push('p');
    let exponent_bits = (bits >> 52) & 0x7ff;
    let exponent = exponent_bits as i64 - 1023;
    text.push_str(&exponent.to_string());
    text
}

/// Write a real's decimal form.
///
/// The form is the ordinary C `%g` form with fifteen significant digits:
/// the plain form while the exponent is in `[-4, 15)`, the exponent form
/// otherwise, and the trailing zeros removed.
fn real_decimal_text(value: f64) -> String {
    let scientific = format!("{:.14e}", value);
    let (mantissa, exponent) = match scientific.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, exponent),
        None => (scientific.as_str(), "0"),
    };
    let (magnitude, negative) = decimal_exponent(exponent);
    // The plain form is used while the exponent is in -4..15; the exponent
    // form is used below minus four and from fifteen on.
    let plain = (negative && magnitude <= 4) || (!negative && magnitude <= 14);
    if !plain {
        return exponent_form(mantissa, magnitude, negative);
    }
    // The plain form keeps fifteen significant digits, so its length is
    // `14 - exponent`.
    let length = if negative {
        14 + magnitude
    } else {
        14 - magnitude
    };
    strip_trailing_zeros(&format!("{:.*}", length, value))
}

/// Write the exponent form of a decimal number.
fn exponent_form(mantissa: &str, magnitude: usize, negative: bool) -> String {
    let mantissa = strip_trailing_zeros(mantissa);
    format!(
        "{}e{}{:02}",
        mantissa,
        if negative { '-' } else { '+' },
        magnitude
    )
}

/// Read the exponent of a decimal number's exponent form: its magnitude and
/// whether it is negative.
fn decimal_exponent(text: &str) -> (usize, bool) {
    let mut magnitude = 0usize;
    let mut negative = false;
    for (index, character) in text.chars().enumerate() {
        match character {
            '-' if index == 0 => negative = true,
            '+' if index == 0 => {}
            _ => match character.to_digit(10) {
                Some(digit) => {
                    // The digit is 0..=9, so the conversion cannot truncate.
                    magnitude = magnitude.saturating_mul(10).saturating_add(digit as usize);
                }
                None => break,
            },
        }
    }
    (magnitude, negative)
}

/// Remove the trailing zeros of a decimal number's fractional part, and the
/// point when nothing follows it.
fn strip_trailing_zeros(text: &str) -> String {
    if !text.contains('.') {
        return text.to_string();
    }
    let trimmed = text.trim_end_matches('0');
    trimmed.strip_suffix('.').unwrap_or(trimmed).to_string()
}

// ---------------------------------------------------------------------------
// The token stream
// ---------------------------------------------------------------------------

/// The token stream the helper returns.
struct Tokens<'a> {
    engine: &'a Tjs2Engine,
    array: &'a DetachedValue,
    index: usize,
}

impl<'a> Tokens<'a> {
    /// The stream of the token array.
    fn new(engine: &'a Tjs2Engine, array: &'a DetachedValue) -> Tokens<'a> {
        Tokens {
            engine,
            array,
            index: 0,
        }
    }

    /// The next token.
    fn next(&mut self) -> Result<MemberRead, String> {
        let value = self
            .engine
            .member_read(self.array.raw_id(), &self.index.to_string())?;
        self.index += 1;
        Ok(value)
    }

    /// The next token, as a string.
    fn next_string(&mut self) -> Result<String, String> {
        match self.next()? {
            MemberRead::Scalar(TjsValue::String(text)) => Ok(text),
            other => Err(format!(
                "saveStruct: the token stream holds {other:?}, expected a string"
            )),
        }
    }

    /// The next token, as an integer.
    fn next_integer(&mut self) -> Result<i64, String> {
        match self.next()? {
            MemberRead::Scalar(TjsValue::Integer(value)) => Ok(value),
            other => Err(format!(
                "saveStruct: the token stream holds {other:?}, expected an integer"
            )),
        }
    }

    /// The next token, as a real.
    fn next_real(&mut self) -> Result<f64, String> {
        match self.next()? {
            MemberRead::Scalar(TjsValue::Real(value)) => Ok(value),
            other => Err(format!(
                "saveStruct: the token stream holds {other:?}, expected a real"
            )),
        }
    }

    /// The next token, as an octet's bytes.
    fn next_octet(&mut self) -> Result<Vec<u8>, String> {
        match self.next()? {
            MemberRead::Octet(bytes) => Ok(bytes),
            other => Err(format!(
                "saveStruct: the token stream holds {other:?}, expected an octet"
            )),
        }
    }

    /// The next token, as an entry count.
    fn next_count(&mut self) -> Result<usize, String> {
        let count = self.next_integer()?;
        usize::try_from(count)
            .map_err(|_| format!("saveStruct: the token stream holds the entry count {count}"))
    }

    /// Read the next node.
    fn read_node(&mut self) -> Result<Node, String> {
        let kind = self.next_string()?;
        self.read_body(&kind)
    }

    /// Read the next value.
    fn read_value(&mut self) -> Result<Node, String> {
        let kind = self.next_string()?;
        match kind.as_str() {
            "void" => Ok(Node::Scalar(Scalar::Void)),
            "null" => Ok(Node::Scalar(Scalar::Null)),
            "int" => Ok(Node::Scalar(Scalar::Integer(self.next_integer()?))),
            "real" => Ok(Node::Scalar(Scalar::Real(self.next_real()?))),
            "string" => Ok(Node::Scalar(Scalar::Text(self.next_string()?))),
            "octet" => Ok(Node::Scalar(Scalar::Octet(self.next_octet()?))),
            "dict" | "array" => self.read_body(&kind),
            "skip" => Ok(Node::Skip),
            other => Err(format!(
                "saveStruct: the token stream holds the unknown kind '{other}'"
            )),
        }
    }

    /// Read the body of a container.
    fn read_body(&mut self, kind: &str) -> Result<Node, String> {
        match kind {
            "dict" => {
                let count = self.next_count()?;
                let mut entries = Vec::new();
                for _ in 0..count {
                    let name = self.next_string()?;
                    entries.push((name, self.read_value()?));
                }
                Ok(Node::Dict(entries))
            }
            "array" => {
                let count = self.next_count()?;
                let mut elements = Vec::new();
                for _ in 0..count {
                    // The name slot of an array element is empty.
                    let _name = self.next_string()?;
                    elements.push(self.read_value()?);
                }
                Ok(Node::Array(elements))
            }
            other => Err(format!(
                "saveStruct: the token stream holds the unknown container kind '{other}'"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// The member
// ---------------------------------------------------------------------------

/// `Scripts.toStructString(target [, newline [, option]])`.
///
/// The member serializes the target into the plugin's second text form.
extern "C" fn scripts_to_struct_string(
    engine: *mut c_void,
    _objthis: *mut c_void,
    argc: c_int,
    argv: *const Value,
    out: *mut Value,
    out_error: *mut *mut c_char,
) -> c_int {
    // SAFETY: the trampoline guarantees `argc` valid `Value` entries.
    let arguments = unsafe { args(argv, argc) };
    let Some(target) = arguments.first() else {
        return report_error(out_error, "Scripts.toStructString requires 1 argument");
    };
    let Some(resolved) = resolve(engine) else {
        return report_error(
            out_error,
            "Scripts.toStructString: compat context is not bound",
        );
    };
    let newline = match arguments.get(1) {
        Some(value) => match try_value_as_i64(value) {
            Ok(value) => value,
            Err(error) => {
                return report_error(
                    out_error,
                    &format!("Scripts.toStructString: newline: {error}"),
                );
            }
        },
        // The newline defaults to the selector `1`, which is LF.
        None => 1,
    };
    let option = match arguments.get(2) {
        Some(value) => match try_value_as_i64(value) {
            Ok(value) => value,
            Err(error) => {
                return report_error(
                    out_error,
                    &format!("Scripts.toStructString: option: {error}"),
                );
            }
        },
        None => 0,
    };
    if option & SSO_HIDDEN != 0 {
        return report_error(
            out_error,
            "Scripts.toStructString: the hidden-member option (8) is not supported: the \
             engine's member enumeration does not report the hidden members",
        );
    }
    let options = Options::new(newline, option);
    let ctx = resolved.ctx();
    // SAFETY: `ctx` is only created for a bound, live engine.
    let services = unsafe { ctx.services() };
    let namespace = {
        let state = services.state::<ScriptsState>();
        state.namespace
    };
    let Some(namespace) = namespace else {
        return report_error(
            out_error,
            "Scripts.toStructString: the traversal helper is not installed",
        );
    };
    match serialize(resolved.engine(), namespace, target, &options) {
        Ok(text) => {
            set_string_out(&ctx, out, &text);
            0
        }
        Err(error) => report_error(out_error, &format!("Scripts.toStructString: {error}")),
    }
}

/// Serialize the target into the text form.
fn serialize(
    engine: &Tjs2Engine,
    namespace: Tjs2ValueId,
    target: &Value,
    options: &Options,
) -> Result<String, String> {
    let mut text = String::new();
    match target.ty {
        VAL_VOID => text.push_str("void"),
        VAL_NULL => text.push_str("null"),
        VAL_INTEGER => {
            write_type_prefix(&mut text, options, "int ");
            text.push_str(&target.integer.to_string());
        }
        VAL_REAL => {
            write_type_prefix(&mut text, options, "real ");
            text.push_str(&real_expression_text(target.real));
        }
        VAL_STRING => text.push_str(&quote_c(&value_as_string(target))),
        VAL_OCTET => {
            // SAFETY: the bytes are owned by the VM and valid for the call.
            let bytes = match unsafe { target.octet_bytes() } {
                Some(bytes) => bytes,
                None => &[],
            };
            text.push_str(&octet_text(bytes));
        }
        VAL_OBJECT => {
            let node = read_target(engine, namespace, target)?;
            write_node(&mut text, &node, options, 0);
        }
        other => return Err(format!("the value of type {other} cannot be serialized")),
    }
    Ok(text)
}

/// Read the target's node through the traversal helper.
fn read_target(
    engine: &Tjs2Engine,
    namespace: Tjs2ValueId,
    target: &Value,
) -> Result<Node, String> {
    let object = engine.retain_object_arg(target)?;
    let stream = engine.call_member(
        namespace,
        TREE_MEMBER,
        &[TjsValue::Retained(object.raw_id().as_ptr() as u64)],
    )?;
    let array = engine.retain_value_detached(&stream)?;
    let mut tokens = Tokens::new(engine, &array);
    tokens.read_node()
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register the member on the `Scripts` class object.
pub(crate) fn register(ctx: &mut CompatCtx) -> Result<(), CompatError> {
    // SAFETY: `ctx` is only created for a bound, live engine.
    let engine = unsafe { ctx.engine() };
    engine
        .exec_script(HELPER_SCRIPT, "saveStruct")
        .map_err(|error| {
            CompatError::Register(format!(
                "saveStruct: could not install the traversal helper: {error}"
            ))
        })?;
    let namespace = match engine.eval_retained(NAMESPACE_NAME, "saveStruct") {
        Ok(RetainedValue::Object(object)) => {
            let id = object.raw_id();
            // The engine owns the retention and frees it at destruction.
            std::mem::forget(object);
            id
        }
        Ok(RetainedValue::Value(value)) => {
            return Err(CompatError::Register(format!(
                "saveStruct: the traversal helper namespace is not an object ({value:?})"
            )));
        }
        Err(error) => {
            return Err(CompatError::Register(format!(
                "saveStruct: could not resolve the traversal helper namespace: {error}"
            )));
        }
    };
    let global = engine.global_object().map_err(|error| {
        CompatError::Register(format!(
            "saveStruct: could not resolve the global object: {error}"
        ))
    })?;
    // The helper's members live on the namespace, so retaining the namespace
    // keeps them alive and the script sees no free function.
    engine
        .delete_member(global.raw_id(), NAMESPACE_NAME)
        .map_err(|error| {
            CompatError::Register(format!(
                "saveStruct: could not remove the traversal helper namespace: {error}"
            ))
        })?;
    // SAFETY: `ctx` is only created for a bound, live engine.
    unsafe { ctx.services() }.state::<ScriptsState>().namespace = Some(namespace);

    crate::native_util::register_builtin_static_members(
        engine,
        "Scripts",
        &[NativeObjThisMethodDef {
            name: "toStructString",
            f: scripts_to_struct_string,
        }],
        &[],
    )
    .map_err(CompatError::Register)?;

    Ok(())
}
