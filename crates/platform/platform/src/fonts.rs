//! Host system-font directories.
//!
//! The engine (L2) may not name a platform path, so the composition root
//! supplies this list to the text engine. [`system_font_dirs`] returns the
//! directories the host installs fonts into.
//!
//! The list belongs to the **selected platform**, not to the machine the
//! build runs on: the platform is chosen at compile time, so the Android
//! backend carries the Android directories and the desktop backend carries
//! the desktop's. The Android backend is therefore compile-checked and tested
//! with the Android list, whatever the host the check runs on.
//!
//! `fontdb`'s own `load_system_fonts` already knows the Windows, macOS and
//! Unix/X11 locations, but it deliberately excludes Android from its Unix
//! scan, so on Android this list is the only source of system faces.
//!
//! [`default_font_families`] names the families the host installs as its own
//! default font, in the order the engine tries them.

use std::path::PathBuf;

/// Directories Android mounts its system fonts into.
///
/// `/system/fonts` holds the platform's `NotoSansCJK-Regular.ttc`; the
/// `product` and `vendor` partitions carry OEM/vendor aliases on some
/// devices.
#[cfg(feature = "platform-android")]
pub(crate) const ANDROID_SYSTEM_FONT_DIRS: &[&str] =
    &["/system/fonts", "/product/fonts", "/vendor/fonts"];

/// System-wide font directories on a Unix-like desktop.
#[cfg(feature = "platform-linux")]
const UNIX_SYSTEM_FONT_DIRS: &[&str] = &["/usr/share/fonts", "/usr/local/share/fonts"];

/// The default user-interface font families of an Android host.
///
/// The platform's own font is `Roboto`; the `Noto` and `Droid` families are
/// the ones a vendor image carries instead.
#[cfg(feature = "platform-android")]
const ANDROID_DEFAULT_FONT_FAMILIES: &[&str] = &["Roboto", "Noto Sans", "Droid Sans"];

/// The default user-interface font families of a Unix-like desktop.
///
/// The desktop's own default font is the first family the host installs; the
/// rest are the metric-compatible faces a distribution ships instead.
#[cfg(feature = "platform-linux")]
const UNIX_DEFAULT_FONT_FAMILIES: &[&str] =
    &["DejaVu Sans", "Liberation Sans", "FreeSans", "Noto Sans"];

/// Returns the host's system font directories.
///
/// On Android this is the platform's `/system/fonts`, `/product/fonts` and
/// `/vendor/fonts` directories. On a Unix-like desktop it is the system
/// directories plus the per-user XDG locations
/// (`$XDG_DATA_HOME/fonts`, falling back to `$HOME/.local/share/fonts`, and
/// the legacy `$HOME/.fonts`).
///
/// The caller owns the returned list and hands it to the text engine. A
/// returned directory may not exist; the caller treats a missing entry as
/// an absent source rather than an error.
pub fn system_font_dirs() -> Vec<PathBuf> {
    platform_font_dirs()
}

/// The font directories of the selected platform.
#[cfg(feature = "platform-android")]
fn platform_font_dirs() -> Vec<PathBuf> {
    ANDROID_SYSTEM_FONT_DIRS.iter().map(PathBuf::from).collect()
}

/// The font directories of the selected platform.
#[cfg(feature = "platform-linux")]
fn platform_font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = UNIX_SYSTEM_FONT_DIRS.iter().map(PathBuf::from).collect();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    if let Some(data_home) = data_home {
        dirs.push(data_home.join("fonts"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".fonts"));
    }
    dirs
}

/// Returns the host's default user-interface font families, in preference
/// order.
///
/// The first family the host provides is the engine's default face: the face
/// whose metrics a face name that resolves to nothing takes. The list names
/// the families the host installs as its own default font, so the engine
/// never has to guess a family the host does not have. A host that provides
/// none of them still has a default face: the engine falls back to one of the
/// host's own faces.
///
/// The caller owns the returned list and hands it to the text engine.
pub fn default_font_families() -> &'static [&'static str] {
    platform_default_font_families()
}

/// The default font families of the selected platform.
#[cfg(feature = "platform-android")]
fn platform_default_font_families() -> &'static [&'static str] {
    ANDROID_DEFAULT_FONT_FAMILIES
}

/// The default font families of the selected platform.
#[cfg(feature = "platform-linux")]
fn platform_default_font_families() -> &'static [&'static str] {
    UNIX_DEFAULT_FONT_FAMILIES
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The host always names at least one directory, so an empty list can
    /// never silently disable system-font discovery.
    #[test]
    fn system_font_dirs_are_not_empty() {
        assert!(!system_font_dirs().is_empty());
    }

    /// The host always names at least one default font family, so the
    /// engine's default face is never left without a candidate.
    #[test]
    fn default_font_families_are_not_empty() {
        let families = default_font_families();
        assert!(!families.is_empty());
        for family in families {
            assert!(!family.is_empty(), "a default font family is empty");
        }
    }

    /// The Android list must name the platform directory the text engine has
    /// no other way to reach; `fontdb`'s own Unix scan excludes Android.
    #[cfg(feature = "platform-android")]
    #[test]
    fn android_font_dirs_include_system_fonts() {
        assert!(!ANDROID_SYSTEM_FONT_DIRS.is_empty());
        assert!(ANDROID_SYSTEM_FONT_DIRS.contains(&"/system/fonts"));
        for dir in ANDROID_SYSTEM_FONT_DIRS {
            assert!(
                Path::new(dir).is_absolute(),
                "Android font directory {dir} is relative"
            );
        }
    }

    /// The Android backend scans the platform's own font directory: the
    /// directory `fontdb`'s own scan never reaches on Android.
    #[cfg(feature = "platform-android")]
    #[test]
    fn the_android_backend_scans_the_platform_font_directory() {
        let dirs = system_font_dirs();
        assert!(
            dirs.iter().any(|dir| dir == Path::new("/system/fonts")),
            "the Android backend must scan the platform's font directory: {dirs:?}"
        );
        assert_eq!(
            default_font_families(),
            ANDROID_DEFAULT_FONT_FAMILIES,
            "the Android backend names the platform's own default font"
        );
    }

    /// The desktop backend scans the system font directories, and the
    /// per-user locations beside them.
    #[cfg(feature = "platform-linux")]
    #[test]
    fn the_desktop_backend_scans_the_system_font_directories() {
        let dirs = system_font_dirs();
        assert!(
            dirs.iter().any(|dir| dir == Path::new("/usr/share/fonts")),
            "the desktop backend must scan the system font directory: {dirs:?}"
        );
        assert_eq!(
            default_font_families(),
            UNIX_DEFAULT_FONT_FAMILIES,
            "the desktop backend names the desktop's own default font"
        );
    }
}
