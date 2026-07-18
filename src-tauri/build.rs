fn main() {
    // Re-embed the Windows app icon whenever the icon files change. cargo does
    // not track them as build inputs by default, so without this an icon swap
    // relinks the exe with the STALE icon resource.
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::build()
}
