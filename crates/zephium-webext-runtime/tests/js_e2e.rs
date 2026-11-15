//! Exercises the injected binding against the actual Rust call pipeline.

#[path = "support/e2e_host.rs"]
mod host;

#[path = "../src/browser/fake.rs"]
mod fake;
pub use fake::FakeBrowser;
pub use zephium_webext_runtime::*;

use std::process::Command;

#[test]
fn real_bundle_dispatches_in_node() {
    let directory = tempfile::tempdir().expect("bundle directory");
    std::fs::write(
        directory.path().join("bundle.js"),
        zephium_webext_runtime::js::BUNDLE,
    )
    .expect("bundle");
    let output = match Command::new("node")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/js/e2e.mjs"))
        .env(
            "ZEPHIUM_WEBEXT_E2E_HOST",
            std::env::current_exe().expect("test executable"),
        )
        .env("ZEPHIUM_WEBEXT_BUNDLE_DIR", directory.path())
        .output()
    {
        Ok(output) => output,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && std::env::var_os("CI").is_none() =>
        {
            eprintln!("skipping: node is not installed");
            return;
        }
        Err(error) => panic!("could not run node: {error}"),
    };
    assert!(
        output.status.success(),
        "end-to-end binding tests failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "stdio peer launched by the Node end-to-end test"]
fn stdio_host() {
    host::serve();
}
