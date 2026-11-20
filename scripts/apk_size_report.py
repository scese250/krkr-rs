#!/usr/bin/env python3
"""Report what an Android APK is made of, and verify the surface the engine resolves by name.

Two questions are answered here, both about the artifact that reaches a device:

* **What is in it?** The package is measured: every entry, grouped the way the
  package is built (each ``.dex``, each native library, ``resources.arsc``, the
  ``res/`` tree, the ``META-INF/`` and ``kotlin/`` metadata), with the totals.
  The numbers are printed as a table and as ``key=value`` lines, so a later
  build can be compared against this one without a human reading them.

* **Is the surface the native code resolves still there?** The engine reads
  seven static fields off the engine Activity through JNI, by name, and the
  game-activity glue resolves the native entry point by name. A shrinker that
  renames or removes any of them leaves a package that installs and launches
  and then dies on the device, with nothing in the build log to show for it.
  The names are therefore read out of the package and checked:

  * the compiled manifest names the Activity and the native library
    (``android.app.lib_name``);
  * the DEX still carries that Activity class, with one static field per name
    the native side declares, each with the declared type;
  * the packaged library exports the entry points the glue resolves.

Everything is read from the package itself with the standard library: the DEX
tables, the ELF dynamic symbols and the compiled manifest are parsed here, so
the check does not depend on a particular SDK tool being installed, and it
reads the *tables* rather than searching for byte patterns — a name that is
present as a string but is not a static field of the Activity does not pass.

Usage:
    apk_size_report.py --apk <path to the .apk>
    apk_size_report.py --apk <path> --self-test
"""

from __future__ import annotations

import argparse
import hashlib
import re
import shutil
import struct
import sys
import tempfile
import zipfile
import zlib
from dataclasses import dataclass, field
from pathlib import Path

# --------------------------------------------------------------------------
# What the native side resolves by name
# --------------------------------------------------------------------------

# The static fields of the engine Activity, read through JNI. Their names are
# read from the crate that owns them instead of being repeated here: that file
# is their only owner, and a second copy in this script is a table that can
# drift from the code that reads it.
JNI_FIELDS_SOURCE = "crates/android/src/jni_fields.rs"
KEEP_RULES_SOURCE = "android/app/proguard-rules.pro"

JNI_CONST_RE = re.compile(
    r"(?:pub(?:\(crate\))?\s+)?const\s+(FIELD_[A-Z0-9_]+|SIG_STRING)\s*:\s*&str\s*=\s*\"([^\"]*)\""
)

# The dynamic symbols the game-activity glue resolves by name. The first is
# the entry point the Activity's native code calls into, the second is the
# JNI method the Activity resolves on this library, the third is the engine's
# own entry point that the glue calls.
REQUIRED_SYMBOLS = (
    "GameActivity_onCreate",
    "Java_com_google_androidgamesdk_GameActivity_initializeNativeCode",
    "android_main",
)

# The manifest attribute the Activity carries to name its native library.
LIB_NAME_META = "android.app.lib_name"

# The size the release package is held to.
#
# The first measurement this script printed was a package with no shrinker, an
# uncompressed dex and an unstripped engine library: 37,607,087 bytes
# (35.86 MiB). The dex in that package measured 24,001,412 bytes, and deflated
# the same way the packager deflates it they are 8,285,512 - so the dex alone
# accounts for 15.0 MiB of the package, and the budget is set where it is so
# that a build which loses either the shrinker or the compression fails.
#
# 30 MiB is 5.6 MiB above the 24.4 MiB the package measures with the dex
# compressed and nothing else changed, and 12 MiB above it with the shrinker
# on as well: room for the engine to grow, and no room for the dex to go back
# to being uncompressed.
BUDGET_BYTES = 30 * 1024 * 1024


class CheckError(Exception):
    """A verification step that must not be skipped could not be carried out."""


# --------------------------------------------------------------------------
# The size report
# --------------------------------------------------------------------------


@dataclass
class Entry:
    """One file in the package."""

    name: str
    uncompressed: int
    stored: int
    method: str

    @property
    def stored_method(self) -> str:
        return {0: "stored", 8: "deflated"}.get(self.method, str(self.method))


@dataclass
class Row:
    """One line of the table: a single file, or a whole tree of them."""

    label: str
    key: str
    entries: int
    uncompressed: int
    stored: int
    method: str = ""


def group_of(name: str) -> str:
    """The group a package entry belongs to."""
    if name.endswith(".dex"):
        return "dex"
    if name.startswith("lib/"):
        return "lib"
    if name == "resources.arsc":
        return "arsc"
    if name.startswith("res/"):
        return "res"
    if name.startswith("META-INF/"):
        return "meta-inf"
    if name.startswith("kotlin/"):
        return "kotlin"
    if name.startswith("assets/"):
        return "assets"
    return "other"


GROUP_LABELS = {
    "res": "the res/ tree",
    "meta-inf": "the META-INF/ metadata",
    "kotlin": "the kotlin/ metadata",
    "assets": "the assets/ tree",
}


def read_entries(apk: zipfile.ZipFile) -> list[Entry]:
    """Every entry of the package, with its sizes."""
    entries = []
    for info in apk.infolist():
        entries.append(
            Entry(
                name=info.filename,
                uncompressed=info.file_size,
                stored=info.compress_size,
                method=info.compress_type,
            )
        )
    return entries


def build_rows(entries: list[Entry]) -> list[Row]:
    """Group the entries into the rows of the table.

    Every DEX and every native library gets its own row, because those are the
    parts a build changes; the resource, metadata and Kotlin trees are one row
    each, with their entry count.
    """
    rows: list[Row] = []
    trees: dict[str, Row] = {}
    for entry in entries:
        group = group_of(entry.name)
        if group in ("dex", "lib", "arsc", "other"):
            rows.append(
                Row(
                    label=entry.name,
                    key=entry.name,
                    entries=1,
                    uncompressed=entry.uncompressed,
                    stored=entry.stored,
                    method=entry.stored_method,
                )
            )
            continue
        row = trees.get(group)
        if row is None:
            row = Row(
                label=GROUP_LABELS[group],
                key=group,
                entries=0,
                uncompressed=0,
                stored=0,
            )
            trees[group] = row
            rows.append(row)
        row.entries += 1
        row.uncompressed += entry.uncompressed
        row.stored += entry.stored
        row.method = _mixed_method(row.method, entry.stored_method)
    for row in rows:
        if row.entries > 1:
            row.label = f"{row.label} ({row.entries} entries)"
    rows.sort(key=lambda row: (-row.stored, row.label))
    return rows


def _mixed_method(current: str, method: str) -> str:
    """The compression method of a tree of entries."""
    if not current:
        return method
    if current == method:
        return current
    return "mixed"


def format_bytes(value: int) -> str:
    """A byte count with thousands separators."""
    return f"{value:,}"


def format_mib(value: int) -> str:
    """A byte count in mebibytes."""
    return f"{value / (1024 * 1024):.2f} MiB"


def size_report(apk_path: Path, entries: list[Entry], rows: list[Row]) -> list[str]:
    """The table of what the package is made of."""
    file_size = apk_path.stat().st_size
    total_uncompressed = sum(entry.uncompressed for entry in entries)
    total_stored = sum(entry.stored for entry in entries)
    label_width = max(len(row.label) for row in rows) + 2

    lines = [
        f"APK composition: {apk_path}",
        f"  file {format_bytes(file_size)} bytes ({format_mib(file_size)}), "
        f"{len(entries)} entries, "
        f"{format_bytes(total_uncompressed)} bytes uncompressed, "
        f"{format_bytes(total_stored)} bytes stored",
        "",
        f"  {'entry'.ljust(label_width)}{'uncompressed':>16}{'in-APK':>14}{'share':>8}{'method':>9}",
        f"  {'-' * (label_width + 47)}",
    ]
    for row in rows:
        share = (row.stored / total_stored * 100) if total_stored else 0.0
        lines.append(
            f"  {row.label.ljust(label_width)}"
            f"{format_bytes(row.uncompressed):>16}"
            f"{format_bytes(row.stored):>14}"
            f"{share:>7.1f}%"
            f"{row.method:>9}"
        )
    lines.append(f"  {'-' * (label_width + 47)}")
    lines.append(
        f"  {'total'.ljust(label_width)}"
        f"{format_bytes(total_uncompressed):>16}"
        f"{format_bytes(total_stored):>14}"
        f"{'100.0%':>8}"
    )
    lines.append("")
    lines.append("  measured values (one per line, for a later build to compare against):")
    lines.append(f"apk-size file_bytes={file_size}")
    lines.append(f"apk-size entries={len(entries)}")
    lines.append(f"apk-size uncompressed_bytes={total_uncompressed}")
    lines.append(f"apk-size stored_bytes={total_stored}")
    for row in rows:
        lines.append(
            f"apk-size entry={row.key.replace('/', '.')} entries={row.entries} "
            f"uncompressed_bytes={row.uncompressed} stored_bytes={row.stored}"
        )
    return lines


# --------------------------------------------------------------------------
# The DEX
# --------------------------------------------------------------------------

ACC_STATIC = 0x0008
ACC_PUBLIC = 0x0001
ACC_NATIVE = 0x0100


def u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def uleb128(data: bytes, offset: int) -> tuple[int, int]:
    """Read an unsigned LEB128 value; return it and the offset after it."""
    value = 0
    shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return value, offset
        shift += 7


@dataclass
class DexClass:
    """One class of a DEX file."""

    descriptor: str
    superclass: str | None
    access_flags: int
    static_fields: list[tuple[str, str, int]] = field(default_factory=list)
    instance_fields: list[tuple[str, str, int]] = field(default_factory=list)
    methods: list[str] = field(default_factory=list)
    native_methods: list[str] = field(default_factory=list)


class DexFile:
    """The class, field and string tables of a DEX file."""

    def __init__(self, data: bytes, label: str):
        if data[:4] != b"dex\n":
            raise CheckError(f"{label}: not a DEX file")
        self.data = data
        self.label = label
        (
            self.file_size,
            self.header_size,
            self.endian_tag,
            self.link_size,
            self.link_off,
            self.map_off,
            self.string_ids_size,
            self.string_ids_off,
            self.type_ids_size,
            self.type_ids_off,
            self.proto_ids_size,
            self.proto_ids_off,
            self.field_ids_size,
            self.field_ids_off,
            self.method_ids_size,
            self.method_ids_off,
            self.class_defs_size,
            self.class_defs_off,
            self.data_size,
            self.data_off,
        ) = struct.unpack_from("<20I", data, 32)
        if self.endian_tag != 0x12345678:
            raise CheckError(f"{label}: unsupported DEX byte order")

    def string(self, index: int) -> str:
        """The string at `index` of the string table."""
        if index >= self.string_ids_size:
            raise CheckError(f"{self.label}: string index {index} out of range")
        offset = u32(self.data, self.string_ids_off + 4 * index)
        _, offset = uleb128(self.data, offset)
        end = self.data.index(b"\x00", offset)
        return self.data[offset:end].decode("utf-8", "replace")

    def type_descriptor(self, index: int) -> str:
        """The type descriptor at `index` of the type table."""
        if index == 0xFFFFFFFF:
            return ""
        if index >= self.type_ids_size:
            raise CheckError(f"{self.label}: type index {index} out of range")
        return self.string(u32(self.data, self.type_ids_off + 4 * index))

    def field(self, index: int) -> tuple[str, str, str]:
        """The (class, type, name) of the field at `index` of the field table."""
        base = self.field_ids_off + 8 * index
        return (
            self.type_descriptor(u16(self.data, base)),
            self.type_descriptor(u16(self.data, base + 2)),
            self.string(u32(self.data, base + 4)),
        )

    def find_class(self, descriptor: str) -> DexClass | None:
        """The class with that descriptor, if the file carries it."""
        for index in range(self.class_defs_size):
            base = self.class_defs_off + 32 * index
            if self.type_descriptor(u32(self.data, base)) == descriptor:
                return self._class(index)
        return None

    def _class(self, index: int) -> DexClass:
        """One class of the file, with its static fields and its methods."""
        base = self.class_defs_off + 32 * index
        class_idx = u32(self.data, base)
        access_flags = u32(self.data, base + 4)
        superclass_idx = u32(self.data, base + 8)
        class_data_off = u32(self.data, base + 24)
        entry = DexClass(
            descriptor=self.type_descriptor(class_idx),
            superclass=self.type_descriptor(superclass_idx) or None,
            access_flags=access_flags,
        )
        if class_data_off:
            self._read_class_data(class_data_off, entry)
        return entry

    def _read_class_data(self, offset: int, entry: DexClass) -> None:
        data = self.data
        static_fields_size, offset = uleb128(data, offset)
        instance_fields_size, offset = uleb128(data, offset)
        direct_methods_size, offset = uleb128(data, offset)
        virtual_methods_size, offset = uleb128(data, offset)

        field_index = 0
        for _ in range(static_fields_size):
            diff, offset = uleb128(data, offset)
            access_flags, offset = uleb128(data, offset)
            field_index += diff
            _, type_descriptor, name = self.field(field_index)
            entry.static_fields.append((name, type_descriptor, access_flags))

        field_index = 0
        for _ in range(instance_fields_size):
            diff, offset = uleb128(data, offset)
            access_flags, offset = uleb128(data, offset)
            field_index += diff
            _, type_descriptor, name = self.field(field_index)
            entry.instance_fields.append((name, type_descriptor, access_flags))

        method_index = 0
        for count in (direct_methods_size, virtual_methods_size):
            method_index = 0
            for _ in range(count):
                diff, offset = uleb128(data, offset)
                access_flags, offset = uleb128(data, offset)
                code_off, offset = uleb128(data, offset)
                method_index += diff
                base = self.method_ids_off + 8 * method_index
                name = self.string(u32(self.data, base + 4))
                entry.methods.append(name)
                if access_flags & ACC_NATIVE:
                    entry.native_methods.append(name)


def refresh_dex_checksums(data: bytearray) -> None:
    """Recompute the signature and the checksum of a DEX file."""
    data[12:32] = hashlib.sha1(bytes(data[32:])).digest()
    data[8:12] = struct.pack("<I", zlib.adler32(bytes(data[12:])) & 0xFFFFFFFF)


# --------------------------------------------------------------------------
# The ELF dynamic symbols
# --------------------------------------------------------------------------


def elf_exported_symbols(data: bytes) -> set[str]:
    """The names the dynamic symbol table defines."""
    if data[:4] != b"\x7fELF":
        raise CheckError("not an ELF file")
    if data[4] != 2:
        raise CheckError("only 64-bit ELF files are supported")
    section_header_offset = struct.unpack_from("<Q", data, 0x28)[0]
    section_header_size = u16(data, 0x3A)
    section_count = u16(data, 0x3C)

    sections = []
    for index in range(section_count):
        base = section_header_offset + index * section_header_size
        sections.append(
            {
                "type": u32(data, base + 4),
                "offset": struct.unpack_from("<Q", data, base + 24)[0],
                "size": struct.unpack_from("<Q", data, base + 32)[0],
                "link": u32(data, base + 40),
                "entry_size": struct.unpack_from("<Q", data, base + 56)[0],
            }
        )

    symbols: set[str] = set()
    for index, section in enumerate(sections):
        if section["type"] != 11:  # SHT_DYNSYM
            continue
        if section["link"] >= len(sections):
            raise CheckError("the dynamic symbol table names a missing string table")
        strings = sections[section["link"]]
        entry_size = section["entry_size"] or 24
        for position in range(0, section["size"], entry_size):
            base = section["offset"] + position
            name_offset = u32(data, base)
            symbol_section = u16(data, base + 6)
            if symbol_section == 0:  # SHN_UNDEF
                continue
            start = strings["offset"] + name_offset
            end = data.index(b"\x00", start)
            symbols.add(data[start:end].decode("utf-8", "replace"))
    return symbols


# --------------------------------------------------------------------------
# The compiled manifest
# --------------------------------------------------------------------------

RES_STRING_POOL_TYPE = 0x0001
RES_TABLE_TYPE = 0x0002
RES_XML_TYPE = 0x0003
RES_XML_START_ELEMENT = 0x0102
RES_XML_END_ELEMENT = 0x0103
TYPE_STRING = 0x03
ANDROID_NAMESPACE = "http://schemas.android.com/apk/res/android"


class StringPool:
    """The string table of a resource chunk."""

    def __init__(self, data: bytes, offset: int):
        if u16(data, offset) != RES_STRING_POOL_TYPE:
            raise CheckError("expected a string pool")
        self.data = data
        self.header_size = u16(data, offset + 2)
        self.chunk_size = u32(data, offset + 4)
        string_count = u32(data, offset + 8)
        flags = u32(data, offset + 16)
        self.utf8 = bool(flags & (1 << 8))
        self.strings_start = offset + u32(data, offset + 20)
        self.offsets = [
            u32(data, offset + self.header_size + 4 * index)
            for index in range(string_count)
        ]

    def get(self, index: int) -> str:
        if index == 0xFFFFFFFF:  # the "no value" reference
            return ""
        if index >= len(self.offsets):
            raise CheckError(f"string index {index} out of range")
        pointer = self.strings_start + self.offsets[index]
        data = self.data
        if self.utf8:
            first = data[pointer]
            pointer += 1
            if first & 0x80:
                length16 = ((first & 0x7F) << 8) | data[pointer]
                pointer += 1
            else:
                length16 = first
            length8 = data[pointer]
            pointer += 1
            raw = data[pointer : pointer + length8]
            return raw.decode("utf-8", "replace")
        length = u16(data, pointer)
        pointer += 2
        return data[pointer : pointer + 2 * length].decode("utf-16-le", "replace")


@dataclass
class ManifestActivity:
    """An activity of the compiled manifest."""

    name: str
    lib_name: str | None = None


@dataclass
class Manifest:
    """What the compiled manifest declares."""

    package: str | None = None
    activities: list[ManifestActivity] = field(default_factory=list)

    def engine_activity(self) -> ManifestActivity | None:
        """The activity that names a native library."""
        for activity in self.activities:
            if activity.lib_name:
                return activity
        return None


def parse_xml_tree(data: bytes, offset: int) -> Manifest:
    """Read a compiled XML document."""
    manifest = Manifest()
    header_size = u16(data, offset + 2)
    position = offset + header_size
    if u16(data, position) != RES_STRING_POOL_TYPE:
        position = offset + max(header_size, 16)
    pool = StringPool(data, position)
    position += pool.chunk_size
    if u16(data, position) == 0x0180:  # the resource map
        position += u32(data, position + 4)

    stack: list[str] = []
    end = offset + u32(data, offset + 4)
    while position < end:
        node_type = u16(data, position)
        node_header_size = u16(data, position + 2)
        node_size = u32(data, position + 4)
        if node_type == RES_XML_START_ELEMENT:
            name = pool.get(u32(data, position + 20))
            attribute_start = u16(data, position + 24)
            attribute_size = u16(data, position + 26)
            attribute_count = u16(data, position + 28)
            attributes = {}
            for index in range(attribute_count):
                base = position + 16 + attribute_start + index * attribute_size
                namespace = pool.get(u32(data, base))
                attribute = pool.get(u32(data, base + 4))
                value_type = data[base + 15]
                value = u32(data, base + 16)
                attributes[(namespace, attribute)] = (value_type, value)
            stack.append(name)
            _read_manifest_element(manifest, pool, stack, name, attributes)
        elif node_type == RES_XML_END_ELEMENT:
            if stack:
                stack.pop()
        position += node_size
        if node_size == 0 or node_header_size == 0:
            break
    return manifest


def _read_manifest_element(
    manifest: Manifest,
    pool: StringPool,
    stack: list[str],
    name: str,
    attributes: dict[tuple[str, str], tuple[int, int]],
) -> None:
    """Record what the manifest declares, element by element."""
    if name == "manifest":
        value_type, value = attributes.get(("", "package"), (TYPE_STRING, 0))
        if value_type == TYPE_STRING:
            manifest.package = pool.get(value)
        return
    if name == "activity":
        value_type, value = attributes.get((ANDROID_NAMESPACE, "name"), (TYPE_STRING, 0))
        if value_type == TYPE_STRING:
            manifest.activities.append(ManifestActivity(name=pool.get(value)))
        return
    if name == "meta-data":
        value_type, value = attributes.get((ANDROID_NAMESPACE, "name"), (TYPE_STRING, 0))
        if value_type != TYPE_STRING or pool.get(value) != LIB_NAME_META:
            return
        if len(stack) < 2:
            return
        parent = manifest.activities[-1] if manifest.activities else None
        if parent is None:
            return
        value_type, value = attributes.get((ANDROID_NAMESPACE, "value"), (TYPE_STRING, 0))
        if value_type == TYPE_STRING:
            parent.lib_name = pool.get(value)


def parse_manifest(manifest_bytes: bytes) -> Manifest:
    """Read the compiled manifest."""
    if u16(manifest_bytes, 0) != RES_XML_TYPE:
        raise CheckError("the manifest is not a compiled XML document")
    return parse_xml_tree(manifest_bytes, 0)


# --------------------------------------------------------------------------
# The checks
# --------------------------------------------------------------------------


def read_jni_field_names(repo_root: Path) -> tuple[dict[str, str], str]:
    """The static field names and the JNI signature the native side declares."""
    source = repo_root / JNI_FIELDS_SOURCE
    try:
        text = source.read_text(encoding="utf-8")
    except OSError as error:
        raise CheckError(f"cannot read {JNI_FIELDS_SOURCE}: {error}") from error
    fields: dict[str, str] = {}
    signature = None
    for name, value in JNI_CONST_RE.findall(text):
        if name == "SIG_STRING":
            signature = value
        else:
            fields[name] = value
    if not fields:
        raise CheckError(f"{JNI_FIELDS_SOURCE} declares no static field names")
    if signature is None:
        raise CheckError(f"{JNI_FIELDS_SOURCE} declares no JNI signature")
    return fields, signature


@dataclass
class KeepRule:
    """One class a keep rule names, and the members it names."""

    name: str
    kind: str
    members: list[tuple[str, str, str]]


KEEP_RULE_RE = re.compile(r"^-keep(classmembers)?\s+class\s+([\w.$]+)\s*\{")

JAVA_PRIMITIVES = {
    "void": "V",
    "boolean": "Z",
    "byte": "B",
    "char": "C",
    "short": "S",
    "int": "I",
    "long": "J",
    "float": "F",
    "double": "D",
}


def java_type_to_descriptor(text: str) -> str:
    """The type descriptor a Java type name stands for."""
    if text in JAVA_PRIMITIVES:
        return JAVA_PRIMITIVES[text]
    return "L" + text.replace(".", "/") + ";"


def parse_keep_member(line: str) -> tuple[str, str, str] | None:
    """The (kind, type, name) a keep-rule member line stands for."""
    text = line.strip()
    if not text or text.startswith("#"):
        return None
    text = text.rstrip(";").strip()
    if "<init>" in text:
        return ("init", "", "<init>")
    if "<methods>" in text:
        return ("native", "", "<methods>")
    text = text.replace("native ", "").strip()
    if "(" in text:
        tokens = text[: text.index("(")].strip().split()
        if not tokens:
            return None
        return ("method", " ".join(tokens[:-1]), tokens[-1])
    tokens = text.split()
    if not tokens:
        return None
    static = tokens[0] == "static"
    if static:
        tokens = tokens[1:]
    kind = "static-field" if static else "field"
    if len(tokens) == 1:
        return (kind, "", tokens[0])
    return (kind, " ".join(tokens[:-1]), tokens[-1])


def parse_keep_rules(path: Path) -> list[KeepRule]:
    """The classes the keep rules name, with the members they name."""
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise CheckError(f"cannot read {KEEP_RULES_SOURCE}: {error}") from error
    rules: list[KeepRule] = []
    current: KeepRule | None = None
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        match = KEEP_RULE_RE.match(line)
        if match:
            current = KeepRule(
                name=match.group(2),
                kind="keep" if match.group(1) is None else "keepclassmembers",
                members=[],
            )
            rules.append(current)
            continue
        if line.startswith("}"):
            current = None
            continue
        if current is not None:
            member = parse_keep_member(line)
            if member is not None:
                current.members.append(member)
    if not rules:
        raise CheckError(f"{KEEP_RULES_SOURCE} names no class")
    return rules

def qualify(package: str | None, name: str) -> str:
    """The class name a manifest entry stands for."""
    if name.startswith("."):
        if not package:
            raise CheckError(
                "the manifest names an activity relatively but declares no package"
            )
        return f"{package}{name}"
    if "." not in name:
        if not package:
            raise CheckError(
                "the manifest names an activity without a package and declares none"
            )
        return f"{package}.{name}"
    return name


def jni_name(symbol: str) -> tuple[str, str] | None:
    """The class and the method a JNI symbol stands for."""
    if not symbol.startswith("Java_"):
        return None
    body = symbol[len("Java_") :]
    class_part, _, method = body.rpartition("_")
    if not class_part or not method:
        return None
    return class_part.replace("_", "/"), method


def find_class(dexes: dict[str, DexFile], descriptor: str) -> tuple[str, DexClass] | None:
    """The class with that descriptor, in whichever DEX file carries it."""
    for name, dex in dexes.items():
        candidate = dex.find_class(descriptor)
        if candidate is not None:
            return name, candidate
    return None


def declares_method(dexes: dict[str, DexFile], descriptor: str, method: str) -> bool:
    """Whether the class, or one of its superclasses, declares a method."""
    seen = set()
    while descriptor and descriptor not in seen:
        seen.add(descriptor)
        found = find_class(dexes, descriptor)
        if found is None:
            return False
        candidate = found[1]
        if method in candidate.methods:
            return True
        descriptor = candidate.superclass or ""
    return False


@dataclass
class CheckResult:
    """What the verification found."""

    problems: list[str] = field(default_factory=list)
    lines: list[str] = field(default_factory=list)
    hints: list[str] = field(default_factory=list)


def verify(apk_path: Path, repo_root: Path) -> CheckResult:
    """Verify that the names the native side resolves are still in the package."""
    result = CheckResult()
    fields, signature = read_jni_field_names(repo_root)

    with zipfile.ZipFile(apk_path) as apk:
        names = apk.namelist()

        # The compiled manifest: which Activity, and which native library.
        if "AndroidManifest.xml" not in names:
            raise CheckError("the package carries no compiled manifest")
        manifest = parse_manifest(apk.read("AndroidManifest.xml"))
        activity = manifest.engine_activity()
        if activity is None:
            raise CheckError(
                f"the manifest declares no activity with a {LIB_NAME_META} entry"
            )
        class_name = qualify(manifest.package, activity.name)
        result.lines.append(
            f"manifest: the activity that names a native library is {class_name}, "
            f"{LIB_NAME_META}={activity.lib_name}"
        )
        if not activity.lib_name:
            raise CheckError(f"the manifest declares an empty {LIB_NAME_META}")

        # The Activity class and its static fields, read out of the DEX.
        descriptor = f"L{class_name.replace('.', '/')};"
        dexes = {
            name: DexFile(apk.read(name), name)
            for name in sorted(name for name in names if name.endswith(".dex"))
        }
        found = find_class(dexes, descriptor)
        if found is None:
            result.problems.append(
                f"the DEX carries no class {class_name}; the manifest names it, so the "
                f"package would not start"
            )
        else:
            dex_name, candidate = found
            result.lines.append(f"dex: {class_name} is in {dex_name}")
            if candidate.superclass:
                result.lines.append(f"dex:   superclass {candidate.superclass}")
            if not candidate.access_flags & ACC_PUBLIC:
                result.problems.append(
                    f"{class_name} is not public; the framework loads the Activity the "
                    f"manifest names by name, so it would not start"
                )
            by_name = {
                field_name: (type_descriptor, flags)
                for field_name, type_descriptor, flags in candidate.static_fields
            }
            for constant, field_name in sorted(fields.items()):
                entry = by_name.get(field_name)
                if entry is None:
                    result.problems.append(
                        f"{class_name} has no static field {field_name}; the native side "
                        f"reads it by name with GetStaticFieldID, so the engine would not "
                        f"start"
                    )
                    result.hints.append(
                        f"-keepclassmembers class {class_name} "
                        f"{{ static java.lang.String {field_name}; }}"
                    )
                    continue
                type_descriptor, flags = entry
                if not flags & ACC_STATIC:
                    result.problems.append(
                        f"{class_name}.{field_name} is not static; the native side reads it "
                        f"with GetStaticFieldID"
                    )
                if type_descriptor != signature:
                    result.problems.append(
                        f"{class_name}.{field_name} has type {type_descriptor}; the native "
                        f"side reads it as {signature}"
                    )
                result.lines.append(
                    f"dex:   static field {field_name} {type_descriptor} "
                    f"({'static' if flags & ACC_STATIC else 'not static'})"
                )

        # The keep rules are the contract with the shrinker: every class and
        # every member they name has to be in the package under that name.
        rules = parse_keep_rules(repo_root / KEEP_RULES_SOURCE)
        for rule in rules:
            descriptor = f"L{rule.name.replace('.', '/')};"
            found = find_class(dexes, descriptor)
            if found is None:
                result.problems.append(
                    f"the DEX carries no class {rule.name}; the keep rules name it, so "
                    f"the shrinker removed something the app resolves by name"
                )
                continue
            dex_name, candidate = found
            result.lines.append(f"rules: {rule.name} is in {dex_name}")
            if not candidate.access_flags & ACC_PUBLIC:
                result.problems.append(
                    f"{rule.name} is not public; the framework or the native library "
                    f"resolves it by name"
                )
            statics = {
                name: (type_text, flags)
                for name, type_text, flags in candidate.static_fields
            }
            instances = {
                name: (type_text, flags)
                for name, type_text, flags in candidate.instance_fields
            }
            for kind, type_text, name in rule.members:
                if kind == "init":
                    if "<init>" not in candidate.methods:
                        result.problems.append(
                            f"{rule.name} has no constructor; the keep rules name it"
                        )
                    continue
                if kind == "native":
                    if not candidate.native_methods:
                        result.problems.append(
                            f"{rule.name} has no native method; the keep rules name them"
                        )
                    continue
                if kind in ("field", "static-field"):
                    entry = (
                        statics.get(name)
                        if kind == "static-field"
                        else instances.get(name)
                    )
                    if entry is None:
                        result.problems.append(
                            f"{rule.name} has no {kind.replace('-', ' ')} {name}; the "
                            f"keep rules name it"
                        )
                        continue
                    descriptor_text, _flags = entry
                    if type_text and descriptor_text != java_type_to_descriptor(type_text):
                        result.problems.append(
                            f"{rule.name}.{name} has type {descriptor_text}; the keep "
                            f"rules name {type_text}"
                        )
                    continue
                if name not in candidate.methods:
                    result.problems.append(
                        f"{rule.name} declares no method {name}; the keep rules name it"
                    )

        # Every name the native side reads has to be in the rules as well, or
        # the shrinker is free to rename it.
        rules_text = (repo_root / KEEP_RULES_SOURCE).read_text(encoding="utf-8")
        for _constant, field_name in sorted(fields.items()):
            if field_name not in rules_text:
                result.problems.append(
                    f"the keep rules do not name {field_name}; the native side reads "
                    f"it by name, so the shrinker may rename it"
                )
                result.hints.append(
                    f"-keepclassmembers class {class_name} "
                    f"{{ static java.lang.String {field_name}; }}"
                )
        # The service files name the implementations the runtime loads by
        # name, and a shrinker that renames a class rewrites the file with it.
        for name in names:
            if not name.startswith("META-INF/services/"):
                continue
            payload = apk.read(name).decode("utf-8", "replace")
            for line in payload.splitlines():
                entry = line.strip()
                if not entry or entry.startswith("#"):
                    continue
                descriptor = f"L{entry.replace('.', '/')};"
                if find_class(dexes, descriptor) is None:
                    result.problems.append(
                        f"the service file {name} names {entry}, which the DEX does "
                        f"not carry; the runtime loads it by name"
                    )
                else:
                    result.lines.append(
                        f"services: {name} names {entry}, and the DEX carries it"
                    )
        # The native library, and the entry points the glue resolves in it.
        library = f"lib{activity.lib_name}.so"
        candidates = [
            name
            for name in names
            if name.startswith("lib/") and name.endswith(f"/{library}")
        ]
        if not candidates:
            result.problems.append(
                f"the package carries no lib/<abi>/{library}; the Activity "
                f"would fail to load its native library"
            )
        else:
            for candidate in candidates:
                exported = elf_exported_symbols(apk.read(candidate))
                result.lines.append(f"native: {candidate}")
                for symbol in REQUIRED_SYMBOLS:
                    if symbol not in exported:
                        result.problems.append(
                            f"{candidate} does not export {symbol}; the game-activity glue "
                            f"resolves it by name, so the engine would not start"
                        )
                        continue
                    result.lines.append(f"native:   exports {symbol}")
                    # A JNI symbol names a class and a method: the class has to
                    # be in the DEX under that name, or the lookup fails on the
                    # device.
                    jni = jni_name(symbol)
                    if jni is None:
                        continue
                    jni_class, jni_method = jni
                    jni_descriptor = f"L{jni_class};"
                    if find_class(dexes, jni_descriptor) is None:
                        result.problems.append(
                            f"the DEX carries no class {jni_class.replace('/', '.')}; the "
                            f"native library resolves {jni_method} on it by name"
                        )
                        result.hints.append(
                            f"-keep class {jni_class.replace('/', '.')} "
                            f"{{ native {jni_method}; }}"
                        )
                    elif not declares_method(dexes, jni_descriptor, jni_method):
                        result.problems.append(
                            f"{jni_class.replace('/', '.')} declares no method "
                            f"{jni_method}; the native library resolves it by name"
                        )
                        result.hints.append(
                            f"-keep class {jni_class.replace('/', '.')} "
                            f"{{ native {jni_method}; }}"
                        )
                    else:
                        result.lines.append(
                            f"native:   the DEX carries {jni_class.replace('/', '.')} "
                            f"with {jni_method}"
                        )
    return result


def check_budget(apk_path: Path, budget: int) -> tuple[bool, list[str]]:
    """Hold the package to the size budget."""
    size = apk_path.stat().st_size
    lines = [
        f"budget: the package is {format_bytes(size)} bytes "
        f"({format_mib(size)}); the budget is {format_bytes(budget)} bytes "
        f"({format_mib(budget)})",
        f"apk-size budget_bytes={budget}",
    ]
    return size <= budget, lines


# --------------------------------------------------------------------------
# The self-test
# --------------------------------------------------------------------------


def write_mutated_apk(source: Path, target: Path, mutate) -> None:
    """Write a copy of the package with one entry changed."""
    with zipfile.ZipFile(source) as apk:
        with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as copy:
            for info in apk.infolist():
                payload = apk.read(info.filename)
                payload = mutate(info.filename, payload)
                if payload is None:
                    continue
                copy.writestr(info.filename, payload)


def rename_bytes(payload: bytes, old: str, new: str) -> bytes | None:
    """Replace a name in a payload; the replacement must be the same length."""
    if len(old) != len(new):
        raise CheckError("the replacement must be the same length")
    if old.encode("utf-8") not in payload:
        return None
    return payload.replace(old.encode("utf-8"), new.encode("utf-8"))


def self_test(apk_path: Path, repo_root: Path) -> int:
    """Prove that the checks reject a package that lost one of the names."""
    failures = 0
    with tempfile.TemporaryDirectory(prefix="apk-size-report-") as workdir:
        work = Path(workdir)

        cases = [
            (
                "a renamed static field",
                "dex",
                lambda name, payload: _mutate_dex_field(name, payload),
            ),
            (
                "a renamed native entry point",
                "native",
                lambda name, payload: _mutate_native_symbol(name, payload),
            ),
            (
                "a renamed JNI class",
                "jni-class",
                lambda name, payload: _mutate_jni_class(name, payload),
            ),
            (
                "a missing native library",
                "library",
                lambda name, payload: None
                if name.endswith("libkrkr_android.so")
                else payload,
            ),
        ]

        for description, kind, mutate in cases:
            target = work / f"{kind}.apk"
            write_mutated_apk(apk_path, target, mutate)
            try:
                result = verify(target, repo_root)
            except CheckError as error:
                result = CheckResult(problems=[str(error)])
            if result.problems:
                print(
                    f"apk-size-report: self-test: {description} is rejected "
                    f"({result.problems[0]})"
                )
            else:
                print(
                    f"apk-size-report: self-test: FAILED: {description} was accepted",
                    file=sys.stderr,
                )
                failures += 1
        # The rules and the native contract can drift apart: a field the
        # native side reads that the rules no longer name is a rename waiting
        # to happen, and it is caught without a package at all.
        drifted = work / "drifted"
        for relative in (KEEP_RULES_SOURCE, JNI_FIELDS_SOURCE):
            (drifted / relative).parent.mkdir(parents=True, exist_ok=True)
        rules_text = (repo_root / KEEP_RULES_SOURCE).read_text(encoding="utf-8")
        (drifted / KEEP_RULES_SOURCE).write_text(
            rules_text.replace("    static java.lang.String pendingGameDir;\n", ""),
            encoding="utf-8",
        )
        shutil.copy(repo_root / JNI_FIELDS_SOURCE, drifted / JNI_FIELDS_SOURCE)
        try:
            result = verify(apk_path, drifted)
        except CheckError as error:
            result = CheckResult(problems=[str(error)])
        if result.problems:
            print(
                f"apk-size-report: self-test: a keep rule that forgot a field is "
                f"rejected ({result.problems[0]})"
            )
        else:
            print(
                "apk-size-report: self-test: FAILED: a keep rule that forgot a "
                "field was accepted",
                file=sys.stderr,
            )
            failures += 1

    if failures:
        print(
            f"apk-size-report: self-test: {failures} case(s) were not rejected",
            file=sys.stderr,
        )
        return 1
    return 0


def _mutate_dex_field(name: str, payload: bytes) -> bytes | None:
    """Rename one of the static fields the native side reads."""
    if not name.endswith(".dex"):
        return payload
    renamed = rename_bytes(payload, "pendingGameDir", "pendingGameXir")
    if renamed is None:
        return payload
    data = bytearray(renamed)
    refresh_dex_checksums(data)
    return bytes(data)


def _mutate_native_symbol(name: str, payload: bytes) -> bytes | None:
    """Rename one of the symbols the glue resolves."""
    if not name.endswith(".so"):
        return payload
    renamed = rename_bytes(payload, "GameActivity_onCreate", "GameActivity_onCreatX")
    if renamed is None:
        return payload
    return renamed


def _mutate_jni_class(name: str, payload: bytes) -> bytes | None:
    """Rename the class a JNI symbol names."""
    if not name.endswith(".dex"):
        return payload
    renamed = rename_bytes(
        payload,
        "Lcom/google/androidgamesdk/GameActivity;",
        "Lcom/google/androidgamesdk/GameActivitY;",
    )
    if renamed is None:
        return payload
    data = bytearray(renamed)
    refresh_dex_checksums(data)
    return bytes(data)


# --------------------------------------------------------------------------
# The entry point
# --------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--apk", required=True, help="the package to report on")
    parser.add_argument(
        "--repo-root",
        default=str(Path(__file__).resolve().parent.parent),
        help="the checkout the native side is read from",
    )
    parser.add_argument(
        "--budget-bytes",
        type=int,
        default=BUDGET_BYTES,
        help="the size the package is held to",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="prove that the checks reject a package that lost a name",
    )
    args = parser.parse_args(argv)

    apk_path = Path(args.apk)
    repo_root = Path(args.repo_root)
    if not apk_path.is_file():
        print(f"apk-size-report: {apk_path} is not a file", file=sys.stderr)
        return 1

    if args.self_test:
        return self_test(apk_path, repo_root)

    with zipfile.ZipFile(apk_path) as apk:
        entries = read_entries(apk)
    rows = build_rows(entries)
    for line in size_report(apk_path, entries, rows):
        print(line)

    print("")
    within_budget, budget_lines = check_budget(apk_path, args.budget_bytes)
    for line in budget_lines:
        print(line)

    print("")
    try:
        result = verify(apk_path, repo_root)
    except CheckError as error:
        print(f"apk-size-report: the surface check could not be carried out: {error}")
        return 1
    for line in result.lines:
        print(line)
    if result.problems:
        print("")
        for problem in result.problems:
            print(f"apk-size-report: {problem}")
        if result.hints:
            print("")
            print(
                "apk-size-report: a shrinker has to keep the names the native side "
                "resolves:"
            )
            for hint in dict.fromkeys(result.hints):
                print(f"apk-size-report:   {hint}")
        return 1
    print("apk-size-report: the names the native side resolves are all present")

    if not within_budget:
        print(
            f"apk-size-report: the package is over the budget of "
            f"{format_bytes(args.budget_bytes)} bytes",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
