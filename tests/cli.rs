//! Integration test: the binary validates a default configuration and exits `0`.
//!
//! This is the seed of the headless test suite (TODO §0.3, ROADMAP §8).

use std::process::Command;

fn crab() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crab"));
    // Without `--config`, crab reads `$XDG_CONFIG_HOME/crab/config.toml`.
    // Point the child at a directory that does not exist so the test exercises
    // the built-in defaults rather than the developer's own config.
    command.env(
        "XDG_CONFIG_HOME",
        std::env::temp_dir().join("crab-test-no-config"),
    );
    command
}

#[test]
fn validate_defaults_succeeds() {
    let output = crab()
        .arg("--validate")
        .output()
        .expect("run crab --validate");
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
    let dir = std::env::temp_dir().join("crab-test-config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.toml");
    std::fs::write(&path, "[layout]\ngaps = -5\n").unwrap();

    let output = crab()
        .args(["--validate", "--config"])
        .arg(&path)
        .output()
        .expect("run crab --validate");
    assert!(!output.status.success(), "a negative gap must be rejected");
}

#[test]
fn validate_rejects_the_old_flat_layout_keys() {
    let dir = std::env::temp_dir().join("crab-test-config");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("legacy.toml");
    std::fs::write(&path, "gaps = 4\n").unwrap();

    let output = crab()
        .args(["--validate", "--config"])
        .arg(&path)
        .output()
        .expect("run crab --validate");
    assert!(!output.status.success(), "the pre-`[layout]` form is gone");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[layout]"), "unexpected stderr: {stderr}");
}

#[test]
fn validate_accepts_the_shipped_example() {
    let output = crab()
        .args(["--validate", "--config", "resources/default-config.toml"])
        .output()
        .expect("run crab --validate");
    assert!(
        output.status.success(),
        "expected success; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
