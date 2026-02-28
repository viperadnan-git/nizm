use anyhow::{Context, Result};
use globset::GlobBuilder;
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

    let glob_matcher = match glob_pattern {
        Some(pattern) => {
            // Auto-prepend **/ for patterns without path separators
            // so *.py matches at any depth
            let effective = if pattern.contains('/') {
                pattern.to_string()
            } else {
                format!("**/{pattern}")
            };
            let glob = GlobBuilder::new(&effective)
                .literal_separator(true)
                .build()
                .with_context(|| format!("invalid glob: {pattern}"))?;
            Some(glob.compile_matcher())
        }
        None => None,
    };

    Ok(staged_files
        .iter()
        .filter_map(|file| {
            // Scope to manifest directory (FR-7, FR-9)
            let relative = if is_root {
                file.clone()
            } else {
                file.strip_prefix(&prefix)?.to_string()
            };

            // Apply glob filter
            if let Some(ref matcher) = glob_matcher
                && !matcher.is_match(&relative)
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
