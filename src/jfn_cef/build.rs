use std::path::PathBuf;

/// Upstream jellyfin-desktop commit this RTX fork is currently based on.
/// Bump this whenever the fork is re-synced onto a newer upstream version, so
/// the in-app version keeps showing which original build it was made from.
const UPSTREAM_BASE: &str = "225af09b";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("CARGO_MANIFEST_DIR has no grandparent")?;

    // `env!` (not std::env::var) so rustc records the dep and re-runs this
    // script when the workspace version bumps.
    println!("cargo:rerun-if-changed=../Cargo.toml");
    let version = env!("CARGO_PKG_VERSION");
    println!("cargo:rustc-env=JFN_APP_VERSION={version}");

    // VERSION_FULL = "<VERSION>+<git short hash>[-dirty]", but only for
    // pre-release VERSIONs (those with a "-suffix"); a clean release stays
    // bare. xtask injects JFN_GIT_HASH/JFN_GIT_DIRTY as the authoritative
    // source; fall back to gitoxide for a bare `cargo build`.
    println!("cargo:rerun-if-env-changed=JFN_GIT_HASH");
    println!("cargo:rerun-if-env-changed=JFN_GIT_DIRTY");
    println!("cargo:rerun-if-env-changed=CEF_RESOURCES_DIR");
    let (git_hash, dirty) = match std::env::var("JFN_GIT_HASH") {
        Ok(h) if !h.is_empty() => {
            let dirty = std::env::var("JFN_GIT_DIRTY").as_deref() == Ok("1");
            (h, dirty)
        }
        _ => git_info(repo_root),
    };
    let fork = if git_hash.is_empty() {
        "local".to_string()
    } else if dirty {
        format!("{git_hash}-dirty")
    } else {
        git_hash.clone()
    };
    // The exact release tag this build corresponds to (CI sets it to the pushed
    // tag, e.g. v2026.06.21). The updater compares this against the latest
    // release tag by string equality — robust regardless of build-clock/UTC,
    // unlike comparing dates. Empty for local builds, which disables the check.
    println!("cargo:rerun-if-env-changed=JFN_RELEASE_TAG");
    let release_tag = std::env::var("JFN_RELEASE_TAG").unwrap_or_default();
    println!("cargo:rustc-env=JFN_RELEASE_TAG={release_tag}");

    // Date shown in About / Playback info. Prefer the RELEASE TAG's date
    // (`vYYYY.MM.DD[.N]` -> `YYYY-MM-DD`) so the shown date always matches the
    // version, no matter which day CI ran or how the tag was dated. Fall back to
    // the wall-clock UTC build date for local/dev builds that have no tag.
    let build_date = release_date(&release_tag).unwrap_or_else(build_date);
    println!("cargo:rustc-env=JFN_BUILD_DATE={build_date}");
    // Shown in the app (About / Playback info). Encodes: this is the RTX fork,
    // the build date + fork commit, and which upstream jellyfin-desktop version
    // and commit it was built from.
    // ASCII-only: this string is reported to the server in the auth header and
    // used as a User-Agent, and a non-ASCII byte there breaks the HTTP request
    // (it caused "Connection Failure"). Keep punctuation plain.
    let version_full = format!(
        "RTX build {build_date} ({fork}) - base jellyfin-desktop {version}@{UPSTREAM_BASE}"
    );
    println!("cargo:rustc-env=JFN_APP_VERSION_FULL={version_full}");
    track_git_refs(repo_root);

    let web_dir = repo_root.join("src").join("web");
    for entry in std::fs::read_dir(&web_dir)?.flatten() {
        let p = entry.path();
        println!("cargo:rerun-if-changed={}", p.display());
    }
    Ok(())
}

/// Fallback for bare `cargo build` (no xtask). Empty hash when there is no repo.
fn git_info(repo_root: &std::path::Path) -> (String, bool) {
    let Ok(repo) = gix::discover(repo_root) else {
        return (String::new(), false);
    };
    let hash = repo
        .head_id()
        .ok()
        .map(|id| id.to_hex_with_len(7).to_string())
        .unwrap_or_default();
    let dirty = repo.is_dirty().unwrap_or(false);
    (hash, dirty)
}

/// A release tag `vYYYY.MM.DD[.N]` -> `YYYY-MM-DD`, so the shown date matches the
/// version. `None` for anything not in that shape (e.g. local builds: empty tag).
fn release_date(tag: &str) -> Option<String> {
    let t = tag.strip_prefix('v').unwrap_or(tag);
    let mut parts = t.split('.');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    let ok = y.len() == 4
        && m.len() == 2
        && d.len() == 2
        && [y, m, d]
            .iter()
            .all(|s| s.bytes().all(|b| b.is_ascii_digit()));
    ok.then(|| format!("{y}-{m}-{d}"))
}

/// UTC build date as `YYYY-MM-DD`, with no external crates. Captured when this
/// script runs (a fresh CI checkout always reruns it, so it reflects the build).
fn build_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    // civil_from_days (Howard Hinnant): days since 1970-01-01 -> Y/M/D.
    let z = secs / 86_400 + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Re-run when HEAD moves. git_dir holds HEAD; common_dir holds refs/packed-refs
/// (they differ under a linked worktree).
fn track_git_refs(repo_root: &std::path::Path) {
    let Ok(repo) = gix::discover(repo_root) else {
        return;
    };
    println!(
        "cargo:rerun-if-changed={}",
        repo.git_dir().join("HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repo.common_dir().join("packed-refs").display()
    );
    if let Ok(Some(r)) = repo.head_ref() {
        let name = r.name().as_bstr().to_string();
        println!(
            "cargo:rerun-if-changed={}",
            repo.common_dir().join(name).display()
        );
    }
}
