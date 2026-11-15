fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        pkg_config::Config::new()
            .atleast_version("2.54")
            .probe("webkit2gtk-web-extension-4.1")
            .expect("WebKitGTK web-process development package >= 2.54 is required");
    }
}
