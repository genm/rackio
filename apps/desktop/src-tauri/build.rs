use std::{env, path::Path};

fn main() {
    let mut attributes = tauri_build::Attributes::new();
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // Tauri links its Windows app manifest (the Common Controls v6
        // dependency) into binaries only. Test executables that build a Tauri
        // app then import v6-only comctl32 entry points without the manifest
        // that selects v6, and Windows refuses to load them with
        // STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139). Embed the same manifest
        // into every linked artifact instead, so the app and its tests load
        // the same way.
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri build script failed: {error:#}");
    }
}
