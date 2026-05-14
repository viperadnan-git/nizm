use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::config::Hook;
use crate::glob::Matcher;

/// Execute a hook with scope filtering and CWD isolation.
pub fn exec_hook(
    hook: &Hook,
    staged_files: &[String],
    manifest_dir: &Path,
    abs_cwd: &Path,
    hook_args: &[String],
) -> Result<(i32, Duration, usize)> {
    let (code, elapsed, count, _, _) =
        exec_hook_inner(hook, staged_files, manifest_dir, abs_cwd, hook_args, false)?;
    Ok((code, elapsed, count))
}

/// Execute a hook capturing stdout/stderr (for parallel mode).
pub fn exec_hook_captured(
    hook: &Hook,
    staged_files: &[String],
    manifest_dir: &Path,
    abs_cwd: &Path,
    hook_args: &[String],
) -> Result<(i32, Duration, usize, String, String)> {
    exec_hook_inner(hook, staged_files, manifest_dir, abs_cwd, hook_args, true)
}

fn exec_hook_inner(
    hook: &Hook,
    staged_files: &[String],
    manifest_dir: &Path,
    abs_cwd: &Path,
    hook_args: &[String],
    capture: bool,
) -> Result<(i32, Duration, usize, String, String)> {
    let scoped = scope_files(staged_files, manifest_dir, hook.glob_matcher.as_ref());
    if scoped.is_empty() {
        return Ok((0, Duration::ZERO, 0, String::new(), String::new()));
    }

    let start = Instant::now();
    let (code, stdout, stderr) = run_cmd(&hook.cmd, &scoped, Some(abs_cwd), hook_args, capture)?;
    Ok((code, start.elapsed(), scoped.len(), stdout, stderr))
}

pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

fn run_cmd(
    cmd: &str,
    staged_files: &[&str],
    cwd: Option<&Path>,
    hook_args: &[String],
    capture: bool,
) -> Result<(i32, String, String)> {
    let resolved = resolve_placeholders(cmd, staged_files, hook_args);

    let mut command = Command::new("sh");
    command.args(["-c", &resolved]);

    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    if capture {
        let output = command
            .output()
            .with_context(|| format!("failed to execute: {resolved}"))?;
        Ok((
            output.status.code().unwrap_or(1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    } else {
        let status = command
            .status()
            .with_context(|| format!("failed to execute: {resolved}"))?;
        Ok((status.code().unwrap_or(1), String::new(), String::new()))
    }
}

/// Filter staged files by manifest directory and a pre-compiled matcher.
/// Returns references to relative path portions — no allocations for root manifests.
fn scope_files<'a>(
    staged_files: &'a [String],
    manifest_dir: &Path,
    matcher: Option<&Matcher>,
) -> Vec<&'a str> {
    let dir_str = manifest_dir.to_string_lossy();
    let is_root = dir_str == "." || dir_str.is_empty();
    let prefix = if is_root {
        String::new()
    } else {
        format!("{}/", dir_str)
    };

    staged_files
        .iter()
        .filter_map(|file| {
            let relative = if is_root {
                file.as_str()
            } else {
                file.strip_prefix(&prefix)?
            };
            match matcher {
                Some(m) if !m.is_match(relative) => None,
                _ => Some(relative),
            }
        })
        .collect()
}

/// Replace `{staged_files}` and `{N}` (1-based positional args) in `cmd`.
/// All substituted values are shell-escaped. `{N}` out of range expands to
/// an empty string (matches lefthook). Unknown `{...}` tokens are preserved.
fn resolve_placeholders(cmd: &str, staged_files: &[&str], hook_args: &[String]) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut rest = cmd;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let key = &after[..close];
        let tail = &after[close + 1..];
        if key == "staged_files" {
            for (i, f) in staged_files.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&shell_escape(f));
            }
        } else if let Ok(n) = key.parse::<usize>()
            && n >= 1
        {
            if let Some(arg) = hook_args.get(n - 1) {
                out.push_str(&shell_escape(arg));
            }
        } else {
            out.push_str(&rest[open..open + 1 + close + 1]);
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

fn shell_escape(s: &str) -> String {
    if s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | '@' | ':' | '+' | ',')
    }) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn substitutes_staged_files() {
        let out = resolve_placeholders("ruff {staged_files}", &["a.py", "b.py"], &[]);
        assert_eq!(out, "ruff a.py b.py");
    }

    #[test]
    fn staged_files_shell_escaped() {
        let out = resolve_placeholders("ruff {staged_files}", &["a b.py"], &[]);
        assert_eq!(out, "ruff 'a b.py'");
    }

    #[test]
    fn substitutes_positional_args() {
        let args = s(&[".git/COMMIT_EDITMSG"]);
        let out = resolve_placeholders("commitlint --edit {1}", &[], &args);
        assert_eq!(out, "commitlint --edit .git/COMMIT_EDITMSG");
    }

    #[test]
    fn shell_escapes_positional_args() {
        let args = s(&["path with space.txt"]);
        let out = resolve_placeholders("cat {1}", &[], &args);
        assert_eq!(out, "cat 'path with space.txt'");
    }

    #[test]
    fn missing_positional_arg_expands_to_empty() {
        let out = resolve_placeholders("echo {2}", &[], &s(&["only-one"]));
        assert_eq!(out, "echo ");
    }

    #[test]
    fn unknown_placeholder_is_preserved() {
        let out = resolve_placeholders("echo {nope}", &[], &[]);
        assert_eq!(out, "echo {nope}");
    }

    #[test]
    fn both_placeholders_in_one_cmd() {
        let args = s(&["msgfile"]);
        let out = resolve_placeholders("lint {staged_files} --msg {1}", &["a.py"], &args);
        assert_eq!(out, "lint a.py --msg msgfile");
    }
}
