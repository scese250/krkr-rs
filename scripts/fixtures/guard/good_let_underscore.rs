// Guard self-test fixture: near-miss patterns that MUST NOT be flagged by the
// `let-underscore` rule.  Not compiled and not part of any crate.

fn write_header(file_system: &FileSystem, file: &FileState) -> Result<(), FsError> {
    fs::write_at(file_system, file, 0, b"header")?;
    fs::flush(file_system, file)?;
    Ok(())
}

fn append_line(file_system: &FileSystem, file: &FileState) -> Result<usize, FsError> {
    let written = fs::append(file_system, file, b"line")?;
    fs::flush(file_system, file)?;
    Ok(written)
}
