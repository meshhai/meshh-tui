use std::{fs, path::Path};

#[test]
fn github_actions_runs_required_rust_feedback_loop() {
    let workflow_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows")
        .join("ci.yml");

    let workflow = fs::read_to_string(&workflow_path).unwrap_or_else(|error| {
        panic!(
            "expected a GitHub Actions workflow at {}: {error}",
            workflow_path.display()
        )
    });

    for trigger in ["push", "pull_request"] {
        assert!(
            workflow.contains(trigger),
            "CI workflow should run on {trigger}; workflow was:\n{workflow}"
        );
    }

    for command in [
        "cargo fmt --all -- --check",
        "cargo clippy --workspace --all-targets -- -D warnings",
        "cargo test --workspace",
        "cargo doc --workspace --no-deps",
    ] {
        assert!(
            workflow.contains(command),
            "CI workflow should run `{command}`; workflow was:\n{workflow}"
        );
    }
}
