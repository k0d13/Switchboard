fn main() {
    // tauri-build only embeds its manifest in the app exe, but test exes need it too: without
    // Common Controls v6 they fail to start with STATUS_ENTRYPOINT_NOT_FOUND. So embed our own
    // copy in every target instead.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());

    let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows)).expect("tauri build failed");
}
