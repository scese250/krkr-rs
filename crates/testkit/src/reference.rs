//! Resolution of the fixtures that ship with the checkout cloned beside this
//! repository.
//!
//! The checkout is a separate clone: its `cpp/` directory holds the engine
//! sources the gate's checks read, and its `tests/test_files` directory holds
//! the fixture files the test suite mounts. It is found the way the gate
//! finds it: [`REFERENCE_ENV`] when it is set, then the conventional
//! directory under the repository root; the first candidate that holds a
//! `cpp/` directory is the checkout.

use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// The environment variable that names the checkout root.
///
/// It is the variable `krkr-gate` reads; the gate's checkout option sets it
/// for the checks it runs.
pub const REFERENCE_ENV: &str = "KRKR_REFERENCE";

/// The fixture directory inside the checkout root.
const TEST_FILES: &str = "tests/test_files";

/// The checkout directory name under the repository root.
const DEFAULT_DIR: &str = "reference";

/// The directory that marks a clone as the checkout.
const CHECKOUT_DIR: &str = "cpp";

/// A fixture below the checkout could not be resolved.
///
/// The [`std::fmt::Display`] text names the fixture, the variable to set,
/// the directory it must point at and the gate's checkout option.
#[derive(Debug)]
pub struct ReferenceFixtureError {
    message: String,
}

impl fmt::Display for ReferenceFixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ReferenceFixtureError {}

/// Resolves a path below the checkout's `tests/test_files`.
///
/// The checkout root is [`REFERENCE_ENV`] when it is set and non-empty,
/// otherwise the conventional checkout directory under `<repo>`, where
/// `<repo>` is the directory above this crate that holds both `Cargo.toml`
/// and `crates`. The first candidate that holds a `cpp/` directory is the
/// checkout — the rule the gate's checks use, so the gate and the test suite
/// read the same clone. A relative [`REFERENCE_ENV`] value is resolved
/// against `<repo>`, because a test binary runs with the package directory
/// as its working directory.
///
/// # Errors
///
/// Returns [`ReferenceFixtureError`] when no candidate is a checkout or when
/// the checkout holds no such fixture. The message names the fixture, the
/// variable to set, the directory it must point at and the gate's checkout
/// option.
pub fn reference_fixture(relative: impl AsRef<Path>) -> Result<PathBuf, ReferenceFixtureError> {
    let named = env::var_os(REFERENCE_ENV).filter(|value| !value.is_empty());
    let repo = repo_root();
    resolve_fixture(relative.as_ref(), named.as_deref(), repo.as_deref())
}

/// Resolves a fixture of the checkout, failing the test when the checkout is
/// unavailable.
///
/// The panic carries the resolver's message, so the reason the test did not
/// run is in the test output and not in a captured stderr line. A test that
/// cannot run must not report a pass.
pub fn require_reference_fixture(relative: impl AsRef<Path>) -> PathBuf {
    match reference_fixture(relative) {
        Ok(fixture) => fixture,
        Err(error) => panic!("{error}"),
    }
}

/// Resolves a fixture below the checkout's `tests/test_files`.
///
/// `named` is the value of [`REFERENCE_ENV`] and `repo` is the repository
/// root; both are arguments so the resolution is exercised without the
/// machine's environment.
fn resolve_fixture(
    relative: &Path,
    named: Option<&OsStr>,
    repo: Option<&Path>,
) -> Result<PathBuf, ReferenceFixtureError> {
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();
    let fixture_name = relative.display();
    if let Some(value) = named {
        let mut path = PathBuf::from(value);
        if path.is_relative()
            && let Some(repo) = repo
        {
            path = repo.join(&path);
        }
        let path = expand_home(path);
        let origin = format!("{REFERENCE_ENV}={}", path.display());
        candidates.push((path, origin));
    }
    if let Some(repo) = repo {
        let path = repo.join(DEFAULT_DIR);
        let origin = format!("the default {}", path.display());
        candidates.push((path, origin));
    }
    let mut rejected: Vec<String> = Vec::new();
    for (path, origin) in candidates {
        if !path.join(CHECKOUT_DIR).is_dir() {
            rejected.push(format!("{origin} (no {CHECKOUT_DIR}/ directory)"));
            continue;
        }
        let fixture = path.join(TEST_FILES).join(relative);
        if fixture.exists() {
            return Ok(fixture);
        }
        return Err(ReferenceFixtureError {
            message: format!(
                "the reference fixture `{fixture_name}` is unavailable: the checkout at {} \
                 holds no {TEST_FILES}/{fixture_name}; set {REFERENCE_ENV} to the clone root \
                 that holds {TEST_FILES}, or pass `--reference PATH` to krkr-gate",
                path.display(),
            ),
        });
    }
    let tried = if rejected.is_empty() {
        "no directory could be tried".to_string()
    } else {
        rejected.join("; ")
    };
    Err(ReferenceFixtureError {
        message: format!(
            "the reference fixture `{fixture_name}` is unavailable: no checkout was found (tried \
             {tried}); set {REFERENCE_ENV} to the clone root that holds {TEST_FILES} and a \
             {CHECKOUT_DIR}/ directory, or pass `--reference PATH` to krkr-gate",
        ),
    })
}

/// Expands a leading `~` to the user's home directory.
fn expand_home(path: PathBuf) -> PathBuf {
    match expanded_home(&path) {
        Some(expanded) => expanded,
        None => path,
    }
}

/// The path with a leading `~` expanded to the user's home directory, or
/// `None` when the path does not start with one.
fn expanded_home(path: &Path) -> Option<PathBuf> {
    let home = home_directory()?;
    let mut components = path.components();
    let Some(Component::Normal(first)) = components.next() else {
        return None;
    };
    if first != OsStr::new("~") {
        return None;
    }
    let rest: PathBuf = components.collect();
    Some(if rest.as_os_str().is_empty() {
        home
    } else {
        home.join(rest)
    })
}

/// The user's home directory.
fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The repository root above this crate.
fn repo_root() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|candidate| {
            candidate.join("Cargo.toml").is_file() && candidate.join("crates").is_dir()
        })
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that stands in for the repository root.
    fn fake_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("krkr-reference-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the root");
        dir
    }

    /// Turns a directory into a checkout: it holds the `cpp/` directory that
    /// marks a clone as the checkout, and the fixture directory.
    fn make_checkout(dir: &Path) {
        std::fs::create_dir_all(dir.join(CHECKOUT_DIR)).expect("create the checkout marker");
        std::fs::create_dir_all(dir.join(TEST_FILES)).expect("create the fixture directory");
    }

    /// A fixture file inside a checkout.
    fn write_fixture(checkout: &Path, relative: &str) -> PathBuf {
        let fixture = checkout.join(TEST_FILES).join(relative);
        std::fs::create_dir_all(fixture.parent().expect("the fixture directory"))
            .expect("create the fixture directory");
        std::fs::write(&fixture, b"fixture").expect("write the fixture");
        fixture
    }

    #[test]
    fn the_named_checkout_is_the_first_candidate() {
        let named = fake_root("named");
        make_checkout(&named);
        let repo = fake_root("named-repo");
        make_checkout(&repo.join(DEFAULT_DIR));
        let fixture = write_fixture(&named, "emote");
        let resolved = resolve_fixture(
            Path::new("emote"),
            Some(named.as_os_str()),
            Some(repo.as_path()),
        )
        .expect("the named checkout holds the fixture");
        assert_eq!(resolved, fixture);
    }

    #[test]
    fn a_named_directory_that_is_not_a_checkout_is_rejected() {
        let plain = fake_root("plain");
        let repo = fake_root("plain-repo");
        make_checkout(&repo.join(DEFAULT_DIR));
        let fixture = write_fixture(&repo.join(DEFAULT_DIR), "emote");
        let resolved = resolve_fixture(
            Path::new("emote"),
            Some(plain.as_os_str()),
            Some(repo.as_path()),
        )
        .expect("the default checkout holds the fixture");
        assert_eq!(resolved, fixture);
    }

    #[test]
    fn the_fixture_resolves_under_the_checkout() {
        let checkout = fake_root("resolves");
        make_checkout(&checkout);
        let fixture = write_fixture(&checkout, "emote");
        let resolved = resolve_fixture(Path::new("emote"), Some(checkout.as_os_str()), None)
            .expect("the checkout holds the fixture");
        assert_eq!(resolved, fixture);
    }

    #[test]
    fn the_error_names_the_fixture_the_variable_and_the_directory() {
        let repo = fake_root("error-names");
        make_checkout(&repo.join(DEFAULT_DIR));
        let error = resolve_fixture(Path::new("emote/missing.psb"), None, Some(repo.as_path()))
            .expect_err("the checkout holds no such fixture");
        let message = error.to_string();
        assert!(message.contains("emote/missing.psb"), "{message}");
        assert!(message.contains(REFERENCE_ENV), "{message}");
        assert!(message.contains(TEST_FILES), "{message}");
        assert!(message.contains("--reference"), "{message}");
    }

    #[test]
    fn the_error_names_the_candidates_that_were_tried() {
        let plain = fake_root("tried");
        let repo = fake_root("tried-repo");
        std::fs::create_dir_all(repo.join(DEFAULT_DIR)).expect("create the default directory");
        let error = resolve_fixture(
            Path::new("emote"),
            Some(plain.as_os_str()),
            Some(repo.as_path()),
        )
        .expect_err("no candidate is a checkout");
        let message = error.to_string();
        assert!(message.contains("emote"), "{message}");
        assert!(message.contains(REFERENCE_ENV), "{message}");
        assert!(
            message.contains(plain.to_str().expect("the path")),
            "{message}"
        );
        assert!(message.contains("no cpp/ directory"), "{message}");
    }

    #[test]
    fn the_required_fixture_fails_with_the_resolver_s_reason() {
        let error = reference_fixture("emote/no-such-fixture")
            .expect_err("no checkout holds such a fixture");
        let failed =
            std::panic::catch_unwind(|| require_reference_fixture("emote/no-such-fixture"))
                .expect_err("the required fixture fails the test");
        let message = failed
            .downcast_ref::<String>()
            .expect("the panic carries a message");
        assert_eq!(*message, error.to_string());
    }
}
