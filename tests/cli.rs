//! Integration test: the binary validates a default configuration and exits `0`.
//!
//! This is the seed of the headless test suite (TODO §0.3, ROADMAP §8).

use std::process::Command;

fn crawm() -> Command {
    Command::new(env!("CARGO_BIN_EXE_crawm"))
}

#[test]
fn validate_defaults_succeeds() {
    let output = crawm()
        .arg("--validate")
        .output()
        .expect("run crawm --validate");
    assert!(
        output.status.success(),
        "expected success; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("ok:"), "unexpected stdout: {stdout}");
}

#[test]
fn validate_rejects_a_bad_config() {
    let dir = std::env::temp_dir().join("crawm-test-config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.toml");
    std::fs::write(&path, "gaps = -5\n").unwrap();

    let output = crawm()
        .args(["--validate", "--config"])
        .arg(&path)
        .output()
        .expect("run crawm --validate");
    assert!(!output.status.success(), "a negative gap must be rejected");
}
