// Guard self-test fixture: the discarded results below MUST be flagged by the
// `let-underscore` rule.  Not compiled and not part of any crate.

fn write_header(file_system: &FileSystem, file: &FileState) {
    let _ = fs::write_at(file_system, file, 0, b"header");
    let _ = fs::flush(file_system, file);
}

fn append_line(file_system: &FileSystem, file: &FileState) {
    let _ = fs::append(file_system, file, b"line");
    let _ = fs::flush(file_system, file);
}
