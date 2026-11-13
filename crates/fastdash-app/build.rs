fn main() {
    #[cfg(windows)]
    {
        // build.rs runs on the host. A Windows host cross-compiling the wasm
        // artifact must not embed the desktop manifest into it.
        let target = std::env::var("TARGET").unwrap_or_default();
        if target.contains("wasm32") {
            return;
        }
        // The exe's icon and manifest. Re-run when either changes: cargo does
        // not track resource inputs, so an icon swap would otherwise relink
        // with the stale resource.
        println!("cargo:rerun-if-changed=resources/fastdash.rc");
        println!("cargo:rerun-if-changed=resources/fastdash.manifest.xml");
        println!("cargo:rerun-if-changed=resources/icons/icon.ico");
        embed_resource::compile("resources/fastdash.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed the Windows resources");
    }
}
