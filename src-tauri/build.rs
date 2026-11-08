fn main() {
    // Re-embed the Windows app icon whenever the icon files change. cargo does
    // not track them as build inputs by default, so without this an icon swap
    // relinks the exe with the STALE icon resource.
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=icons/icon.ico");

    // A custom Windows manifest, for its compatibility section: see the file.
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    let windows = tauri_build::WindowsAttributes::new()
        .app_manifest(include_str!("windows-app-manifest.xml"));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}
