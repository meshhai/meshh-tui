use std::path::Path;

#[path = "support/release_audit.rs"]
mod release_audit;

use release_audit::{ReleaseAuditConfig, find_violations};

#[test]
fn tracked_release_files_do_not_contain_sensitive_or_local_only_values() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let violations = find_violations(manifest_dir, &ReleaseAuditConfig::default())
        .expect("release audit should inspect tracked files");

    assert!(
        violations.is_empty(),
        "tracked files contain release-hardening violations:\n{}",
        violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
