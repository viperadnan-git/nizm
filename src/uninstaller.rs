use anyhow::{Context, Result};
use dialoguer::Confirm;
use serde::Serialize;
use std::path::Path;

use crate::{
    config::{self, HookType},
    installer, style,
};

const ALL_HOOK_TYPES: &[HookType] = &[
    HookType::PreCommit,
    HookType::PrePush,
    HookType::CommitMsg,
    HookType::PrepareCommitMsg,
];

pub fn uninstall(repo_root: &Path, purge: bool) -> Result<()> {
    let mut removed_any = false;

    for ht in ALL_HOOK_TYPES {
        let hook_path = repo_root.join(format!(".git/hooks/{}", ht.as_str()));
        if !hook_path.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&hook_path)?;
        if !installer::is_nizm_managed(&content) {
            continue;
        }

        let hook_name = ht.as_str();
        let remaining = remove_block(&content);

        if remaining.trim().is_empty() || remaining.trim() == "#!/bin/sh" {
            std::fs::remove_file(&hook_path)
                .with_context(|| format!("failed to remove {hook_name} hook"))?;
            println!("{}", style::green(&format!("{hook_name} hook removed")));
        } else {
            std::fs::write(&hook_path, &remaining)
                .with_context(|| format!("failed to update {hook_name} hook"))?;
            println!(
                "{}",
                style::green(&format!("nizm block removed from {hook_name} hook"))
            );
        }
        removed_any = true;
    }

    if !removed_any {
        println!("{}", style::yellow("no nizm hooks found — nothing to do"));
        return Ok(());
    }

    // Purge hook config from manifests
    let should_purge = if purge {
        true
    } else {
        Confirm::new()
            .with_prompt("also remove nizm hook config from manifests?")
            .default(false)
            .interact()?
    };

    if should_purge {
        purge_manifests(repo_root)?;
    }

    Ok(())
}

/// Remove the nizm block from hook content.
fn remove_block(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == installer::BLOCK_START);
    let end = lines.iter().rposition(|l| l.trim() == installer::BLOCK_END);

    let (start, end) = match (start, end) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return content.to_string(),
    };

    let mut result: Vec<&str> = Vec::new();
    result.extend_from_slice(&lines[..start]);
    if end + 1 < lines.len() {
        result.extend_from_slice(&lines[end + 1..]);
    }

    // Clean up extra blank lines at the junction
    let mut out = result.join("\n");
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Remove nizm hook sections from all discovered manifests.
fn purge_manifests(repo_root: &Path) -> Result<()> {
    let manifests = config::discover_manifests(repo_root)?;
    let mut cleaned = 0;

    for manifest_path in &manifests {
        let full_path = repo_root.join(manifest_path);
        let filename = manifest_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        let removed = match filename {
            "pyproject.toml" => purge_toml(&full_path, &["tool", "nizm"])?,
            "Cargo.toml" => purge_toml(&full_path, &["package", "metadata", "nizm"])?,
            ".nizm.toml" => {
                std::fs::remove_file(&full_path)
                    .with_context(|| format!("failed to remove {}", full_path.display()))?;
                true
            }
            "package.json" => purge_json(&full_path)?,
            _ => false,
        };

        if removed {
            println!("  {} {}", style::green("cleaned"), manifest_path.display());
            cleaned += 1;
        }
    }

    if cleaned == 0 {
        println!("  no nizm config found in manifests");
    }

    Ok(())
}

/// Remove a TOML key path (e.g. tool.nizm) from a file.
/// Cleans up empty parent tables left behind.
/// Returns true if something was removed.
fn purge_toml(file_path: &Path, key_path: &[&str]) -> Result<bool> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;

    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse TOML")?;

    if key_path.is_empty() {
        return Ok(false);
    }

    let parent_path = &key_path[..key_path.len() - 1];
    let leaf_key = key_path[key_path.len() - 1];

    // Navigate to the parent table, then remove the leaf
    let mut table = doc.as_table_mut();
    for &key in parent_path {
        match table.get_mut(key).and_then(|v| v.as_table_mut()) {
            Some(t) => table = t,
            None => return Ok(false),
        }
    }

    if table.remove(leaf_key).is_none() {
        return Ok(false);
    }

    // Clean up empty parent tables bottom-up
    let mut text = doc.to_string();
    for depth in (0..parent_path.len()).rev() {
        let mut reparsed = text
            .parse::<toml_edit::DocumentMut>()
            .context("re-parse failed")?;

        // Check if the table at this depth is empty (immutable pass)
        let is_empty = {
            let mut t: &toml_edit::Table = reparsed.as_table();
            let key = parent_path[depth];
            for &k in &parent_path[..depth] {
                match t.get(k).and_then(|v| v.as_table()) {
                    Some(inner) => t = inner,
                    None => break,
                }
            }
            t.get(key)
                .and_then(|v| v.as_table())
                .is_some_and(|tbl| tbl.is_empty())
        };

        if is_empty {
            // Mutable pass to remove
            let mut t = reparsed.as_table_mut();
            for &k in &parent_path[..depth] {
                t = t[k].as_table_mut().unwrap();
            }
            t.remove(parent_path[depth]);
            text = reparsed.to_string();
        } else {
            break;
        }
    }

    std::fs::write(file_path, text)
        .with_context(|| format!("failed to write {}", file_path.display()))?;

    Ok(true)
}

/// Remove the "nizm" key from a package.json file.
/// Returns true if something was removed.
fn purge_json(file_path: &Path) -> Result<bool> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;

    let mut root: serde_json::Value =
        serde_json::from_str(&content).context("failed to parse JSON")?;

    let obj = match root.as_object_mut() {
        Some(o) => o,
        None => return Ok(false),
    };

    if obj.remove("nizm").is_none() {
        return Ok(false);
    }

    // Detect original indent and rewrite with same style
    let indent = detect_json_indent(&content);
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    root.serialize(&mut ser)?;
    let mut output = String::from_utf8(buf)?;
    output.push('\n');

    std::fs::write(file_path, output)
        .with_context(|| format!("failed to write {}", file_path.display()))?;

    Ok(true)
}

fn detect_json_indent(content: &str) -> String {
    for line in content.lines().skip(1) {
        let trimmed = line.trim_start();
        if !trimmed.is_empty() {
            let leading = &line[..line.len() - trimmed.len()];
            if !leading.is_empty() {
                return leading.to_string();
            }
        }
    }
    "  ".to_string()
}
