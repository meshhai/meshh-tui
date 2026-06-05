use std::{fs, path::Path};

#[test]
fn release_workflow_builds_smoke_tests_and_publishes_archives() {
    let workflow_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows")
        .join("release.yml");

    let workflow = fs::read_to_string(&workflow_path).unwrap_or_else(|error| {
        panic!(
            "expected a GitHub Actions release workflow at {}: {error}",
            workflow_path.display()
        )
    });

    for expected in [
        "tags:",
        "\"v*\"",
        "x86_64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "cargo build --release --locked --target",
        "\"$bin\" --help",
        "\"$bin\" login --help",
        "\"$bin\" tui --help",
        "shasum -a 256",
        "Get-FileHash",
        "gh release create",
    ] {
        assert!(
            workflow.contains(expected),
            "release workflow should contain `{expected}`; workflow was:\n{workflow}"
        );
    }
}
