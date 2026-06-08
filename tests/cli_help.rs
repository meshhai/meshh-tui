use std::process::Command;

#[test]
fn help_lists_supported_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_meshh"))
        .arg("--help")
        .output()
        .expect("failed to run meshh --help");

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).expect("help output should be valid utf-8");
    assert!(stdout.contains("login"), "help output was:\n{stdout}");
    assert!(stdout.contains("tui"), "help output was:\n{stdout}");
    assert!(stdout.contains("update"), "help output was:\n{stdout}");
}
