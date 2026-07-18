//! Per-user filesystem locations.
//!
//! - Linux: XDG Base Directory (config/cache/state) with `$HOME` fallback.
//! - macOS: `~/.config` for config (matches existing installs), `~/Library`
//!   for cache/logs.
//! - Windows: `%APPDATA%` for config, `%LOCALAPPDATA%` for cache/logs.
//!
//! Each directory getter creates the directory (and parents) if missing
//! before returning.

use std::env;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

// Separate data dir from upstream jellium-desktop so the two installs don't
// share config: the upstream app rewrites settings.json without the RTX keys,
// which would silently disable RTX in this build. Keeping a distinct dir lets
// both run side by side with independent settings, cache, logs and device id.
// Old pre-rename dir was "jellyfin-desktop-rtx"; migrated on first run.
const APP_DIR_NAME: &str = "jellium-desktop-rtx";
const LOG_FILE_NAME: &str = "jellium-desktop.log";

#[derive(Default)]
struct Overrides {
    config_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
}

static OVERRIDES: OnceLock<Mutex<Overrides>> = OnceLock::new();

fn overrides() -> MutexGuard<'static, Overrides> {
    OVERRIDES
        .get_or_init(|| Mutex::new(Overrides::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub fn set_config_dir_override(path: PathBuf) {
    overrides().config_dir = Some(path);
}

pub fn set_cache_dir_override(path: PathBuf) {
    overrides().cache_dir = Some(path);
}

fn config_override() -> Option<PathBuf> {
    overrides().config_dir.clone()
}

fn cache_override() -> Option<PathBuf> {
    overrides().cache_dir.clone()
}

fn env_or(var: &str, fallback: &str) -> String {
    match env::var(var) {
        Ok(v) if !v.is_empty() => v,
        _ => fallback.to_string(),
    }
}

#[cfg(not(windows))]
fn home() -> String {
    env_or("HOME", "/tmp")
}

fn ensure(path: PathBuf) -> PathBuf {
    let _ = fs::create_dir_all(&path);
    path
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

/// `Ok(false)` means another process won the race and created `path` first.
pub fn write_atomic_noclobber(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    match tmp.persist_noclobber(path) {
        Ok(_) => Ok(true),
        Err(err) if err.error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err.error),
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(path) = config_override() {
        return ensure(path);
    }
    ensure(imp::config_base().join(APP_DIR_NAME))
}

pub fn cache_dir() -> PathBuf {
    if let Some(path) = cache_override() {
        return ensure(path);
    }
    ensure(imp::cache_base().join(APP_DIR_NAME))
}

/// One-time settings migration from a prior install into this build's data dir
/// ([`APP_DIR_NAME`]). On first run we have no settings yet; copy `settings.json`
/// and device `instance.json` from the first prior install that has them, in
/// priority order:
///   1. `jellyfin-desktop-rtx` — this fork's own pre-rename dir (keeps the RTX
///      keys and the existing server login),
///   2. `jellium-desktop` — upstream's current dir,
///   3. `jellyfin-desktop` — upstream's pre-rename dir.
/// So the user keeps their login and preferences instead of reconfiguring.
/// No-op when our settings already exist, when a config override is set, or when
/// there's nothing to copy.
pub fn migrate_legacy_config() {
    // A custom config override opts out — only the default location migrates.
    if config_override().is_some() {
        return;
    }
    let dst = config_dir();
    let legacy_names = ["jellyfin-desktop-rtx", "jellium-desktop", "jellyfin-desktop"];
    for name in ["settings.json", "instance.json"] {
        let dstf = dst.join(name);
        if dstf.exists() {
            continue;
        }
        for legacy_name in legacy_names {
            let legacy = imp::config_base().join(legacy_name);
            if legacy == dst {
                continue;
            }
            let srcf = legacy.join(name);
            if srcf.exists() {
                let _ = fs::copy(&srcf, &dstf);
                break;
            }
        }
    }
}

pub fn log_dir() -> PathBuf {
    ensure(imp::log_dir_path())
}

pub fn mpv_home() -> PathBuf {
    ensure(config_dir().join("mpv"))
}

pub fn log_path() -> PathBuf {
    log_dir().join(LOG_FILE_NAME)
}

/// Where logs go when no log file was requested explicitly. Linux: `None` —
/// stderr/journalctl is the norm. macOS/Windows: GUI processes have no
/// user-visible stderr, so default to the platform log file.
pub fn default_log_file() -> Option<PathBuf> {
    imp::DEFAULT_LOG_TO_FILE.then(log_path)
}

#[cfg_attr(target_os = "linux", path = "imp_linux.rs")]
#[cfg_attr(target_os = "macos", path = "imp_macos.rs")]
#[cfg_attr(windows, path = "imp_windows.rs")]
mod imp;
