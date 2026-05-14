use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::config::Hook;
use crate::glob::Matcher;

/// Execute a hook with scope filtering and CWD isolation.
/// Returns (exit_code, duration, scoped_file_count).
pub fn exec_hook(
    hook: &Hook,
    staged_files: &[String],
    manifest_dir: &Path,
    abs_cwd: &Path,
) -> Result<(i32, Duration, usize)> {
    let scoped = scope_files(staged_files, manifest_dir, hook.glob.as_deref())?;

    if scoped.is_empty() {
        return Ok((0, Duration::ZERO, 0));
    }

    let start = Instant::now();
    let code = exec_cmd(&hook.cmd, &scoped, Some(abs_cwd))?;
    let elapsed = start.elapsed();

    Ok((code, elapsed, scoped.len()))
}

/// Execute a hook capturing stdout/stderr (for parallel mode).
/// Returns (exit_code, duration, scoped_file_count, stdout, stderr).
pub fn exec_hook_captured(
    hook: &Hook,
    staged_files: &[String],
    manifest_dir: &Path,
    abs_cwd: &Path,
) -> Result<(i32, Duration, usize, String, String)> {
    let scoped = scope_files(staged_files, manifest_dir, hook.glob.as_deref())?;

    if scoped.is_empty() {
        return Ok((0, Duration::ZERO, 0, String::new(), String::new()));
    }

    let start = Instant::now();
    let (code, stdout, stderr) = run_cmd(&hook.cmd, &scoped, Some(abs_cwd), true)?;
    let elapsed = start.elapsed();

    Ok((code, elapsed, scoped.len(), stdout, stderr))
}

pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

/// Execute a raw command string.
pub fn exec_cmd(cmd: &str, staged_files: &[&str], cwd: Option<&Path>) -> Result<i32> {
    let (code, _, _) = run_cmd(cmd, staged_files, cwd, false)?;
    Ok(code)
}

fn run_cmd(
    cmd: &str,
    staged_files: &[&str],
    cwd: Option<&Path>,
    capture: bool,
) -> Result<(i32, String, String)> {
    let file_list = staged_files
        .iter()
        .map(|f| shell_escape(f))
        .collect::<Vec<_>>()
        .join(" ");

    let resolved = cmd.replace("{staged_files}", &file_list);

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

/// Filter staged files by manifest directory and optional glob patterns.
/// Returns references to relative path portions — no allocations for root manifests.
fn scope_files<'a>(
    staged_files: &'a [String],
    manifest_dir: &Path,
    glob_patterns: Option<&[String]>,
) -> Result<Vec<&'a str>> {
    let dir_str = manifest_dir.to_string_lossy();
    let is_root = dir_str == "." || dir_str.is_empty();
    let prefix = if is_root {
        String::new()
    } else {
        format!("{}/", dir_str)
    };

    let matcher = match glob_patterns {
        Some(patterns) => Some(Matcher::new(patterns)?),
        None => None,
    };

    Ok(staged_files
        .iter()
        .filter_map(|file| {
            let relative = if is_root {
                file.as_str()
            } else {
                file.strip_prefix(&prefix)?
            };
            match &matcher {
                Some(m) if !m.is_match(relative) => None,
                _ => Some(relative),
            }
        })
        .collect())
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
