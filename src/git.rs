use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::style;

pub fn repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to run git — is it installed?")?;

    if !output.status.success() {
        anyhow::bail!("not a git repository");
    }

    let root = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(PathBuf::from(root))
}

pub fn tracked_files() -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-files"])
        .current_dir(repo_root()?)
        .output()
        .context("failed to run git ls-files")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git ls-files failed: {stderr}");
    }

    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

pub fn staged_files() -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-only", "--diff-filter=ACMR"])
        .output()
        .context("failed to run git — is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout).context("git output is not valid UTF-8")?;

    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

fn unstaged_files() -> Result<std::collections::HashSet<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .output()
        .context("git diff failed")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff --name-only failed: {stderr}");
    }

    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Check if any staged files also have unstaged changes (partial staging).
pub fn has_partial_staging(staged: &[String]) -> Result<bool> {
    let unstaged = unstaged_files()?;
    Ok(staged.iter().any(|f| unstaged.contains(f)))
}

/// Create a rescue snapshot ref for recovery.
pub fn create_rescue_ref() -> Result<()> {
    let output = Command::new("git")
        .args(["stash", "create"])
        .output()
        .context("git stash create failed")?;

    let hash = String::from_utf8(output.stdout)?.trim().to_string();
    if hash.is_empty() {
        return Ok(());
    }

    let status = Command::new("git")
        .args(["update-ref", "refs/nizm-backup", &hash])
        .status()
        .context("failed to store rescue ref")?;

    if !status.success() {
        anyhow::bail!("failed to create rescue ref");
    }

    Ok(())
}

pub fn drop_rescue_ref() -> Result<()> {
    let _ = Command::new("git")
        .args(["update-ref", "-d", "refs/nizm-backup"])
        .status();
    Ok(())
}

pub fn rescue_ref_exists() -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "refs/nizm-backup"])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn apply_rescue_ref() -> Result<()> {
    let output = Command::new("git")
        .args(["stash", "apply", "refs/nizm-backup"])
        .output()
        .context("failed to apply rescue ref")?;

    if output.status.success() {
        drop_rescue_ref()?;
        return Ok(());
    }

    // Check if apply created conflicts (partial apply) vs rejected entirely
    let conflicts = conflicted_files().unwrap_or_default();
    if !conflicts.is_empty() {
        // Partially applied with conflicts — drop ref, user resolves manually
        drop_rescue_ref()?;
        anyhow::bail!("recovery applied with conflicts — resolve them manually");
    }

    // Apply rejected entirely (e.g. dirty working tree) — keep ref for retry
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!("recovery failed: {}", stderr.trim());
}

/// Stash unstaged changes, keeping the index (staged content) in the working tree.
pub fn stash_keep_index() -> Result<()> {
    let output = Command::new("git")
        .args(["stash", "push", "--keep-index", "--include-untracked"])
        .stdout(Stdio::null())
        .output()
        .context("git stash failed")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git stash push --keep-index failed: {stderr}");
    }

    Ok(())
}

/// Restore unstaged changes from the stash using diff-apply.
/// `git stash pop` doesn't work with `--keep-index` (always conflicts),
/// so we extract the unstaged diff and apply it directly.
/// If hooks modified files, --3way merge may conflict — resolved with checkout --ours.
/// Returns `true` if conflicts were resolved (rescue ref should be kept for recovery).
pub fn restore_unstaged() -> Result<bool> {
    // Extract unstaged-only diff: stash index → stash working tree
    let diff = Command::new("git")
        .args(["diff", "stash@{0}^2", "stash@{0}"])
        .output()
        .context("failed to extract unstaged diff from stash")?;

    // Restore untracked files from stash^3 (before dropping stash)
    restore_untracked_from_stash()?;

    // Drop the stash (rescue ref still holds objects for recovery)
    let _ = Command::new("git")
        .args(["stash", "drop"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if diff.stdout.is_empty() {
        return Ok(false);
    }

    // Try plain apply first — only modifies working tree, preserves index.
    if apply_patch(&diff.stdout, false)? {
        return Ok(false);
    }

    // Plain apply failed (hooks changed base content). Use --3way (implies --index).
    if !apply_patch(&diff.stdout, true)? {
        // --3way produced conflicts. Resolve: keep hook-modified content.
        let conflicted = conflicted_files()?;
        if !conflicted.is_empty() {
            for file in &conflicted {
                let _ = Command::new("git")
                    .args(["checkout", "--ours", file])
                    .status();
            }
            let _ = Command::new("git").arg("add").args(&conflicted).status();

            eprintln!(
                "nizm: {} {} had conflicting unstaged changes — keeping hook-modified content",
                conflicted.len(),
                if conflicted.len() == 1 {
                    "file"
                } else {
                    "files"
                }
            );
            eprintln!(
                "{} recover original state: {}",
                style::bold("nizm:"),
                style::bold("nizm recover")
            );
            return Ok(true);
        }
    }

    Ok(false)
}

fn restore_untracked_from_stash() -> Result<()> {
    let check = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "stash@{0}^3"])
        .output()?;

    if !check.status.success() {
        return Ok(());
    }

    let root = repo_root()?;

    let list = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "stash@{0}^3"])
        .current_dir(&root)
        .output()?;

    let files = String::from_utf8(list.stdout)?;
    for file in files.lines().filter(|l| !l.is_empty()) {
        let abs = root.join(file);
        if abs.exists() {
            continue;
        }

        let content = Command::new("git")
            .args(["show", &format!("stash@{{0}}^3:{file}")])
            .current_dir(&root)
            .output()?;

        if content.status.success() {
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs, &content.stdout)?;
        }
    }

    Ok(())
}

fn apply_patch(patch: &[u8], three_way: bool) -> Result<bool> {
    let mut args = vec!["apply"];
    if three_way {
        args.push("--3way");
    }

    let mut child = Command::new("git")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to run git apply")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(patch)?;
    }

    Ok(child.wait()?.success())
}

fn conflicted_files() -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output()
        .context("failed to list conflicted files")?;

    let stdout = String::from_utf8(output.stdout)?;
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// Return staged files that were modified by hooks (now have unstaged changes).
pub fn modified_staged_files(staged: &[String]) -> Result<Vec<String>> {
    let unstaged = unstaged_files()?;
    Ok(staged
        .iter()
        .filter(|f| unstaged.contains(*f))
        .cloned()
        .collect())
}

/// Stage files that were modified by hooks.
pub fn add_files(files: &[String]) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let status = Command::new("git")
        .arg("add")
        .args(files)
        .status()
        .context("git add failed")?;

    if !status.success() {
        anyhow::bail!("git add failed for hook-modified files");
    }

    Ok(())
}
