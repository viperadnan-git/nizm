use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::config::Hook;

/// Execute a hook with scope filtering and CWD isolation.
/// Returns (exit_code, duration, scoped_file_count).
pub fn exec_hook(
    hook: &Hook,
    staged_files: &[String],
    manifest_dir: &Path,
    abs_cwd: &Path,
) -> Result<(i32, Duration, usize)> {
    let scoped = scope_files(staged_files, manifest_dir, hook.glob.as_deref());

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
    let scoped = scope_files(staged_files, manifest_dir, hook.glob.as_deref());

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

/// Filter staged files by manifest directory and optional glob pattern.
/// Returns references to relative path portions — no allocations for root manifests.
fn scope_files<'a>(
    staged_files: &'a [String],
    manifest_dir: &Path,
    glob_pattern: Option<&str>,
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

            if let Some(pattern) = glob_pattern
                && !glob_match(pattern, relative)
            {
                return None;
            }

            Some(relative)
        })
        .collect()
}

fn shell_escape(s: &str) -> String {
    if s.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

/// Simple glob matcher supporting:
/// - `*.ext` / `**/*.ext` — extension match at any depth
/// - `*.{a,b,c}` — multiple extensions
/// - `dir/*.ext` — single-level match under dir
fn glob_match(pattern: &str, path: &str) -> bool {
    // Patterns without `/` match at any depth (like `*.py`)
    let pattern = if pattern.contains('/') {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    };

    // Handle `*.{ext1,ext2}` — expand alternations on the extension
    if let Some(star_pos) = pattern.rfind("*.{") {
        let after = &pattern[star_pos + 3..];
        if let Some(close) = after.find('}') {
            let prefix = &pattern[..star_pos];
            return after[..close]
                .split(',')
                .any(|ext| glob_match_simple(&format!("{prefix}*.{ext}"), path));
        }
    }

    glob_match_simple(&pattern, path)
}

fn glob_match_simple(pattern: &str, path: &str) -> bool {
    let pat_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();
    segments_match(&pat_parts, &path_parts)
}

/// Iterative segment matcher using an explicit stack instead of recursion.
fn segments_match(pat: &[&str], path: &[&str]) -> bool {
    let mut stack = vec![(0usize, 0usize)];

    while let Some((pi, pathi)) = stack.pop() {
        if pi == pat.len() {
            if pathi == path.len() {
                return true;
            }
            continue;
        }
        if pat[pi] == "**" {
            // ** matches zero segments (skip **)
            stack.push((pi + 1, pathi));
            // ** matches one+ segments (consume one path segment, keep **)
            if pathi < path.len() {
                stack.push((pi, pathi + 1));
            }
        } else if pathi < path.len() && wildcard_match(pat[pi], path[pathi]) {
            stack.push((pi + 1, pathi + 1));
        }
    }

    false
}

/// Match a single segment with `*` wildcards (no `/`).
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star_pi, mut star_ti) = (usize::MAX, 0);

    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}
