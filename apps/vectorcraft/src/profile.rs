//! Native profile paths. Isolated profiles never migrate preferences from a user's old profile.
//!
//! In order:
//! 1. `VECTORCRAFT_CONFIG_DIR`, when set (tests, agents, custom setups).
//! 2. **Portable mode** (#1366), as in PhotoCraft: a marker file ([`PORTABLE_MARKERS`]) beside the
//!    executable puts the settings in `<exe dir>/VectorCraftData`, so a portable copy leaves nothing
//!    in `%APPDATA%`. The Windows portable zip ships with `portable.txt`; the MSI doesn't. If that
//!    folder can't be written (read-only medium, Program Files), a warning is logged and the
//!    platform folder is used.
//! 3. The platform convention.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Files beside the executable that switch on portable mode (either one; contents are ignored).
pub const PORTABLE_MARKERS: [&str; 2] = ["portable.txt", "VectorCraft.portable"];
/// The settings folder created beside the executable in portable mode.
pub const PORTABLE_DATA_DIR: &str = "VectorCraftData";

pub fn prefs_path(legacy: bool) -> Option<PathBuf> {
    prefs_path_with(std::env::consts::OS, legacy, |key| std::env::var_os(key), portable_dir)
}

/// `portable` is the portable settings folder, if any: only asked for when no override is set.
fn prefs_path_with(os: &str, legacy: bool, env: impl Fn(&str) -> Option<OsString>, portable: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    if let Some(root) = env("VECTORCRAFT_CONFIG_DIR") {
        // Never fall back to DrawCraft's real profile when the isolated file does not exist.
        return (!legacy && !root.is_empty()).then(|| PathBuf::from(root).join("ui.json"));
    }
    if !legacy && let Some(dir) = portable() {
        return Some(dir.join("ui.json"));
    }
    let (name, lower) = if legacy { ("DrawCraft", "drawcraft") } else { ("VectorCraft", "vectorcraft") };
    let base = match os {
        "macos" => env("HOME").map(|p| PathBuf::from(p).join("Library/Application Support").join(name)),
        "windows" => env("APPDATA").map(|p| PathBuf::from(p).join(name)),
        _ => env("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| env("HOME").map(|p| PathBuf::from(p).join(".config"))).map(|p| p.join(lower)),
    };
    base.map(|p| p.join("ui.json"))
}

/// The portable settings folder beside this executable, looked for once per process (it probes
/// the disk, and the settings path is asked for many times).
fn portable_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        portable_dir_in(exe.parent()?).unwrap_or_else(|warning| {
            log::warn!("{warning}");
            eprintln!("vectorcraft: {warning}");
            None
        })
    })
    .clone()
}

/// `<exe_dir>/VectorCraftData` when a portable marker sits in `exe_dir`, created if missing; `None`
/// without a marker, and the warning to show when there is one but that folder can't be written.
fn portable_dir_in(exe_dir: &Path) -> Result<Option<PathBuf>, String> {
    let Some(marker) = PORTABLE_MARKERS.iter().map(|m| exe_dir.join(m)).find(|p| p.is_file()) else { return Ok(None) };
    let data = exe_dir.join(PORTABLE_DATA_DIR);
    match ensure_writable(&data) {
        Ok(()) => Ok(Some(data)),
        Err(e) => Err(format!(
            "portable mode ({} found) but {} isn't writable ({e}); settings are kept in the user's settings folder instead",
            marker.display(),
            data.display()
        )),
    }
}

/// Create `dir` and prove a file can be written in it (a folder can exist yet be read-only).
fn ensure_writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")?;
    // A leftover probe is harmless; don't give up portable mode over it.
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn environment(key: &str) -> Option<OsString> {
        match key {
            "HOME" => Some("/user".into()),
            "APPDATA" => Some("/roaming".into()),
            "XDG_CONFIG_HOME" => Some("/xdg-config".into()),
            _ => None,
        }
    }

    /// No portable marker beside the executable.
    fn installed() -> Option<PathBuf> {
        None
    }

    #[test]
    fn isolated_profile_has_no_legacy_fallback_on_any_platform() {
        for os in ["macos", "windows", "linux"] {
            let env = |key: &str| {
                if key == "VECTORCRAFT_CONFIG_DIR" { Some("/isolated".into()) } else { environment(key) }
            };
            assert_eq!(prefs_path_with(os, false, env, installed), Some("/isolated/ui.json".into()));
            assert_eq!(prefs_path_with(os, true, env, installed), None);
        }
    }

    #[test]
    fn empty_override_does_not_fall_back_to_user_files() {
        let env = |key: &str| if key == "VECTORCRAFT_CONFIG_DIR" { Some(OsString::new()) } else { environment(key) };
        assert_eq!(prefs_path_with("macos", false, env, installed), None);
        assert_eq!(prefs_path_with("macos", true, env, installed), None);
    }

    #[test]
    fn unset_override_keeps_platform_paths_and_legacy_migration() {
        assert_eq!(prefs_path_with("macos", false, environment, installed), Some("/user/Library/Application Support/VectorCraft/ui.json".into()));
        assert_eq!(prefs_path_with("macos", true, environment, installed), Some("/user/Library/Application Support/DrawCraft/ui.json".into()));
        assert_eq!(prefs_path_with("windows", false, environment, installed), Some("/roaming/VectorCraft/ui.json".into()));
        assert_eq!(prefs_path_with("linux", false, environment, installed), Some("/xdg-config/vectorcraft/ui.json".into()));
        assert_eq!(
            prefs_path_with("linux", true, |key| if key == "HOME" { environment(key) } else { None }, installed),
            Some("/user/.config/drawcraft/ui.json".into())
        );
    }

    #[test]
    fn portable_mode_keeps_the_settings_beside_the_executable() {
        let portable = || Some(PathBuf::from("/usb/VectorCraft/VectorCraftData"));
        for os in ["macos", "windows", "linux"] {
            assert_eq!(prefs_path_with(os, false, environment, portable), Some("/usb/VectorCraft/VectorCraftData/ui.json".into()), "{os}");
        }
        // DrawCraft's old settings are still read once, as an installed copy reads them.
        assert_eq!(prefs_path_with("windows", true, environment, portable), Some("/roaming/DrawCraft/ui.json".into()));
    }

    #[test]
    fn the_config_dir_override_wins_over_portable_mode_without_probing_it() {
        let env = |key: &str| if key == "VECTORCRAFT_CONFIG_DIR" { Some("/isolated".into()) } else { environment(key) };
        let portable = || -> Option<PathBuf> { panic!("the portable folder is not looked for under VECTORCRAFT_CONFIG_DIR") };
        assert_eq!(prefs_path_with("windows", false, env, portable), Some("/isolated/ui.json".into()));
        assert_eq!(prefs_path_with("windows", true, env, portable), None);
    }

    fn temp(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("vectorcraft-profile-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn either_marker_beside_the_executable_makes_a_writable_data_folder() {
        for marker in PORTABLE_MARKERS {
            let exe = temp("portable");
            std::fs::write(exe.join(marker), b"").unwrap();
            let dir = portable_dir_in(&exe).unwrap().unwrap();
            assert_eq!(dir, exe.join(PORTABLE_DATA_DIR), "{marker}");
            assert!(dir.is_dir(), "the data folder is created");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the write probe is removed");
            let _ = std::fs::remove_dir_all(&exe);
        }
    }

    #[test]
    fn without_a_marker_file_there_is_no_portable_folder() {
        let exe = temp("installed");
        assert_eq!(portable_dir_in(&exe), Ok(None));
        // A folder named like the marker is not a marker.
        std::fs::create_dir_all(exe.join("portable.txt")).unwrap();
        assert_eq!(portable_dir_in(&exe), Ok(None));
        assert!(!exe.join(PORTABLE_DATA_DIR).exists(), "no data folder without a marker");
        let _ = std::fs::remove_dir_all(&exe);
    }

    #[test]
    fn an_unwritable_portable_folder_is_reported_instead_of_used() {
        let exe = temp("unwritable");
        std::fs::write(exe.join("portable.txt"), b"").unwrap();
        // A file where the data folder should go: it can't be created, on every platform.
        std::fs::write(exe.join(PORTABLE_DATA_DIR), b"in the way").unwrap();
        let warning = portable_dir_in(&exe).unwrap_err();
        assert!(warning.contains("portable") && warning.contains(PORTABLE_DATA_DIR), "{warning}");
        let _ = std::fs::remove_dir_all(&exe);
    }
}
