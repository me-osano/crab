//! Spawning processes for `spawn:` binds and `[autostart] exec_once`.
//!
//! A command line is split shell-style (quotes and backslashes, no variable
//! expansion) and executed directly, never through a shell, so a config line
//! cannot accidentally become shell syntax. Children inherit the compositor's
//! environment, with `WAYLAND_DISPLAY` pointed at crab's own socket so a
//! spawned client connects to us rather than to the host compositor.
//!
//! Each child is waited for on a short-lived thread; without that, exited
//! children would stay zombies until crab itself exits.

use std::process::{Command, Stdio};

/// Splits a command line into an argument vector.
///
/// Whitespace separates arguments; single and double quotes group them
/// (backslash escapes inside double quotes and outside quotes). An empty or
/// unterminated command is an error.
pub(crate) fn parse_command(line: &str) -> Result<Vec<String>, String> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut started = false;

    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else if ch == '\\' && q == '"' {
                    escaped = true;
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '\\' => {
                    escaped = true;
                    started = true;
                }
                '\'' | '"' => {
                    quote = Some(ch);
                    started = true;
                }
                c if c.is_whitespace() => {
                    if started {
                        argv.push(core::mem::take(&mut current));
                        started = false;
                    }
                }
                c => {
                    current.push(c);
                    started = true;
                }
            },
        }
    }

    if escaped {
        return Err("trailing backslash".to_owned());
    }
    if let Some(q) = quote {
        return Err(format!("unterminated {q} quote"));
    }
    if started {
        argv.push(current);
    }
    if argv.is_empty() {
        return Err("empty command".to_owned());
    }
    Ok(argv)
}

/// Runs `argv` detached, waiting for it in the background.
///
/// `wayland_display` is crab's socket name; it overrides the inherited
/// `WAYLAND_DISPLAY` so the child talks to this compositor.
pub(crate) fn spawn(argv: &[String], wayland_display: Option<&str>) {
    let Some((program, args)) = argv.split_first() else {
        return;
    };
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    if let Some(name) = wayland_display {
        command.env("WAYLAND_DISPLAY", name);
    }
    match command.spawn() {
        Ok(mut child) => {
            let program = program.clone();
            std::thread::spawn(move || match child.wait() {
                Ok(status) if !status.success() => {
                    tracing::debug!(%program, ?status, "spawned process exited");
                }
                Ok(_) => {}
                Err(err) => tracing::debug!(%program, ?err, "failed to wait for a spawned process"),
            });
        }
        Err(err) => tracing::warn!(%program, ?err, "failed to spawn a process"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_plain_command() {
        assert_eq!(
            parse_command("rofi -show drun").unwrap(),
            ["rofi", "-show", "drun"]
        );
    }

    #[test]
    fn keeps_quoted_arguments_together() {
        assert_eq!(
            parse_command("foot --title \"my terminal\" 'two words'").unwrap(),
            ["foot", "--title", "my terminal", "two words"]
        );
    }

    #[test]
    fn escapes_spaces_and_quotes() {
        assert_eq!(
            parse_command(r#"echo a\ b "a \"quote\"""#).unwrap(),
            ["echo", "a b", "a \"quote\""]
        );
    }

    #[test]
    fn rejects_empty_and_unterminated_commands() {
        assert!(parse_command("   ").is_err());
        assert!(parse_command("echo 'open").is_err());
        assert!(parse_command(r"echo trailing\").is_err());
    }
}
