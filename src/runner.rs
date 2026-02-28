use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::{config::Hook, style};

/// Execute a hook with scope filtering and CWD isolation.
pub fn exec_hook(
    hook: &Hook,
    staged_files: &[String],
    manifest_dir: &Path,
    abs_cwd: &Path,
) -> Result<i32> {
    let scoped = scope_files(staged_files, manifest_dir, hook.glob.as_deref())?;

    if scoped.is_empty() {
        return Ok(0);
    }

    println!("  {} ({} file(s))", style::bold(&hook.name), scoped.len());
    exec_cmd(&hook.cmd, &scoped, Some(abs_cwd))
}

/// Execute a raw command string.
pub fn exec_cmd(cmd: &str, staged_files: &[String], cwd: Option<&Path>) -> Result<i32> {
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

    let status = command
        .status()
        .with_context(|| format!("failed to execute: {resolved}"))?;

    Ok(status.code().unwrap_or(1))
}

/// Filter staged files by manifest directory and optional glob pattern.
/// Returns paths relative to the manifest directory.
fn scope_files(
    staged_files: &[String],
    manifest_dir: &Path,
    glob_pattern: Option<&str>,
) -> Result<Vec<String>> {
    let dir_str = manifest_dir.to_string_lossy();
    let is_root = dir_str == "." || dir_str.is_empty();
    let prefix = if is_root {
        String::new()
    } else {
        format!("{}/", dir_str)
    };

    Ok(staged_files
        .iter()
        .filter_map(|file| {
            let relative = if is_root {
                file.clone()
            } else {
                file.strip_prefix(&prefix)?.to_string()
            };

            if let Some(pattern) = glob_pattern
                && !glob_match(pattern, &relative)
            {
                return None;
            }

            Some(relative)
        })
        .collect())
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
    // Split into segments
    let pat_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();
    segments_match(&pat_parts, &path_parts)
}

fn segments_match(pat: &[&str], path: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        // ** matches zero or more path segments
        if segments_match(&pat[1..], path) {
            return true;
        }
        if !path.is_empty() {
            return segments_match(pat, &path[1..]);
        }
        return false;
    }
    if path.is_empty() {
        return false;
    }
    wildcard_match(pat[0], path[0]) && segments_match(&pat[1..], &path[1..])
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
