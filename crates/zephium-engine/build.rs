fn main() {
    println!("cargo:rustc-check-cfg=cfg(zephium_protection_qa_profile)");
    // The protection QA app's release-equivalent profile, named only by its
    // output directory; the desktop build script refuses it for anything else.
    let profile = std::env::var_os("OUT_DIR").and_then(|out_dir| {
        std::path::Path::new(&out_dir)
            .ancestors()
            .nth(3)
            .and_then(|profile| profile.file_name())
            .map(|name| name == "protection-qa")
    });
    if profile == Some(true) {
        println!("cargo:rustc-cfg=zephium_protection_qa_profile");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Standalone test executables do not inherit the desktop manifest.
        // Wry imports GetWindowSubclass, supplied by Common Controls v6.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
    }
}
