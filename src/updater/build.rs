//! Build script for the updater side-car.
//!
//! On Windows, embed `updater.rc` (which pulls in `updater.manifest`) so the
//! .exe ships with an `asInvoker` application manifest. This is required, not
//! cosmetic: an executable named like an installer/updater with no manifest is
//! auto-flagged by UAC "Installer Detection" as needing elevation, which makes
//! the parent app's un-elevated `CreateProcess` of it fail with os error 740
//! (ERROR_ELEVATION_REQUIRED) and the self-update do nothing. See
//! `updater.manifest` for the full explanation.
//!
//! We also VALIDATE the manifest here (on every host): `rc.exe`/`windres` embed
//! it verbatim without checking it, and `manifest_required()` only checks that a
//! manifest resource exists — not that it's well-formed. A malformed manifest
//! builds cleanly but makes Windows fail to build the process activation context
//! at launch (`ERROR_SXS_CANT_GEN_ACTCTX`, os error 14001), so the side-car
//! never starts and the self-update silently dies. That exact bug shipped for
//! months (an XML comment containing `--`, illegal per the XML spec); the guard
//! below rejects it so it can never ship again.

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = manifest_dir.join("updater.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());

    // Reject an XML comment containing "--" (illegal per the XML spec; the only
    // legal "--" is the closing "-->"). Windows rejects such a manifest with
    // ERROR_SXS_CANT_GEN_ACTCTX (os error 14001) at launch. Runs on every host
    // so a plain `cargo check` catches it, not just the Windows release build.
    let text = std::fs::read_to_string(&manifest)?;
    let mut rest = text.as_str();
    while let Some(i) = rest.find("<!--") {
        let after = &rest[i + 4..];
        let end = after
            .find("-->")
            .ok_or("updater.manifest: unterminated XML comment")?;
        if after[..end].contains("--") {
            return Err(
                "updater.manifest: an XML comment contains '--' (illegal per \
                        the XML spec). Windows rejects the manifest with \
                        ERROR_SXS_CANT_GEN_ACTCTX (os error 14001) and the side-car \
                        won't launch. Reword the comment to remove the '--'."
                    .into(),
            );
        }
        rest = &after[end + 3..];
    }

    #[cfg(target_os = "windows")]
    {
        let rc = manifest_dir.join("updater.rc");
        println!("cargo:rerun-if-changed={}", rc.display());

        // Errors out if no manifest got embedded, so a silently manifest-less
        // build (which would reintroduce the elevation bug) fails the build.
        embed_resource::compile(&rc, embed_resource::NONE).manifest_required()?;
    }

    Ok(())
}
