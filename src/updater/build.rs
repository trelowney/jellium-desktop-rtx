//! Build script for the updater side-car.
//!
//! On Windows, embed `updater.rc` (which pulls in `updater.manifest`) so the
//! .exe ships with an `asInvoker` application manifest. This is required, not
//! cosmetic: an executable named like an installer/updater with no manifest is
//! auto-flagged by UAC "Installer Detection" as needing elevation, which makes
//! the parent app's un-elevated `CreateProcess` of it fail with os error 740
//! (ERROR_ELEVATION_REQUIRED) and the self-update do nothing. See
//! `updater.manifest` for the full explanation.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(target_os = "windows")]
    {
        use std::path::PathBuf;
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let rc = manifest_dir.join("updater.rc");
        let manifest = manifest_dir.join("updater.manifest");
        println!("cargo:rerun-if-changed={}", rc.display());
        println!("cargo:rerun-if-changed={}", manifest.display());

        // Errors out if no manifest got embedded, so a silently manifest-less
        // build (which would reintroduce the elevation bug) fails the build.
        embed_resource::compile(&rc, embed_resource::NONE).manifest_required()?;
    }

    Ok(())
}
