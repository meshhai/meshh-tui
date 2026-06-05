use std::{fs, path::Path};

#[test]
fn unix_installer_uses_release_archives_and_verifies_checksums() {
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("install.sh");

    let script = fs::read_to_string(&script_path).unwrap_or_else(|error| {
        panic!(
            "expected a Unix installer script at {}: {error}",
            script_path.display()
        )
    });

    for expected in [
        "meshhai/meshh-tui",
        "/releases/latest",
        "meshh_${version_number}_${target}.tar.gz",
        ".sha256",
        "shasum -a 256 -c",
        "sha256sum -c",
        "validate_archive",
        "tar -tzf",
        "tar -tvzf",
        "../* | */../*",
        "unexpected archive entry",
        "release archive binary was not a regular file",
        "tar -xzf \"$archive_name\" \"$archive_dir/$binary\"",
        "MESHH_SKIP_CHECKSUM",
        "MESHH_INSTALL_DIR",
        "MESHH_VERSION",
        "meshh login",
    ] {
        assert!(
            script.contains(expected),
            "install.sh should contain `{expected}`; script was:\n{script}"
        );
    }
}
