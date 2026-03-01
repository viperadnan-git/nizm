use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, MultiSelect};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::{
    config::{self, HookType},
    style,
};
use std::collections::HashSet;

pub const BLOCK_START: &str = "# nizm-start";
pub const BLOCK_END: &str = "# nizm-end";

pub fn is_nizm_managed(content: &str) -> bool {
    content.lines().any(|l| l.trim() == BLOCK_START)
        && content.lines().any(|l| l.trim() == BLOCK_END)
}

pub fn install(
    repo_root: &Path,
    explicit_configs: Vec<PathBuf>,
    parallel: bool,
    force: bool,
) -> Result<()> {
    let interactive = explicit_configs.is_empty();

    let selected: Vec<PathBuf> = if !interactive {
        explicit_configs
    } else {
        println!("scanning for manifests...");
        let manifests = config::discover_manifests(repo_root)?;

        if manifests.is_empty() {
            println!("no supported manifests found");
            if Confirm::new()
                .with_prompt("create a .nizm.toml?")
                .default(true)
                .interact()?
            {
                create_nizm_toml(repo_root)?;
                println!("created .nizm.toml — add hooks and run `nizm install` again");
            }
            return Ok(());
        }

        if manifests.len() == 1 {
            manifests
        } else {
            let labels: Vec<String> = manifests.iter().map(|p| p.display().to_string()).collect();
            let selections = MultiSelect::new()
                .with_prompt("select manifests (space = toggle, enter = confirm)")
                .items(&labels)
                .interact()?;

            if selections.is_empty() {
                println!("no manifests selected — aborting");
                return Ok(());
            }
            selections
                .into_iter()
                .map(|i| manifests[i].clone())
                .collect()
        }
    };

    // Collect hook types present across selected manifests
    let mut hook_types = HashSet::new();
    for path in &selected {
        let cfg = config::parse_manifest(repo_root, path)?;
        if cfg.hooks.is_empty() {
            println!(
                "  {} {} has no nizm hooks",
                style::yellow("warning:"),
                path.display()
            );
        } else {
            let names: Vec<_> = cfg.hooks.iter().map(|h| h.name.as_str()).collect();
            println!("  {} — [{}]", path.display(), names.join(", "));
            for hook in &cfg.hooks {
                hook_types.insert(hook.hook_type);
            }
        }
    }

    if hook_types.is_empty() {
        println!("no hooks configured — run `nizm init` to set up hooks");
        return Ok(());
    }

    // Install a hook file for each discovered type
    for ht in &hook_types {
        install_hook_file(repo_root, &selected, *ht, parallel, force, interactive)?;
    }

    Ok(())
}

fn install_hook_file(
    repo_root: &Path,
    manifests: &[PathBuf],
    hook_type: HookType,
    parallel: bool,
    force: bool,
    interactive: bool,
) -> Result<()> {
    let hook_name = hook_type.as_str();
    let hook_path = repo_root.join(format!(".git/hooks/{hook_name}"));
    let block = generate_block(manifests, parallel, hook_type);

    if !hook_path.exists() {
        std::fs::create_dir_all(hook_path.parent().unwrap())?;
        write_hook(&hook_path, &format!("#!/bin/sh\n{block}\n"))?;
        println!("{}", style::green(&format!("{hook_name} hook installed")));
        return Ok(());
    }

    let content = std::fs::read_to_string(&hook_path)?;

    if is_nizm_managed(&content) {
        if blocks_match(&content, &block) {
            println!(
                "{}",
                style::green(&format!("{hook_name} hook already up to date"))
            );
            return Ok(());
        }

        if has_custom_block_content(&content) {
            require_overwrite_consent(
                &format!("{hook_name}: nizm block has custom modifications"),
                force,
                interactive,
            )?;
        } else {
            println!("updating {hook_name} nizm block");
        }

        let new_content = replace_block(&content, &block)?;
        write_hook(&hook_path, &new_content)?;
        println!("{}", style::green(&format!("{hook_name} hook updated")));
    } else {
        let mut new_content = content;
        if !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
        new_content.push_str(&block);
        new_content.push('\n');
        write_hook(&hook_path, &new_content)?;
        println!(
            "{} appended to existing {hook_name} hook",
            style::green("nizm block")
        );
    }

    Ok(())
}

fn require_overwrite_consent(message: &str, force: bool, interactive: bool) -> Result<()> {
    if force {
        println!("{} {} (--force)", style::yellow("warning:"), message);
    } else if interactive {
        println!("{} {}", style::yellow("warning:"), message);
        if !Confirm::new()
            .with_prompt("overwrite?")
            .default(false)
            .interact()?
        {
            println!("{}", style::yellow("aborting — existing hook preserved"));
            bail!("aborted by user");
        }
    } else {
        bail!("{message} — use --force to overwrite");
    }
    Ok(())
}

fn generate_block(manifests: &[PathBuf], parallel: bool, hook_type: HookType) -> String {
    let config_args: String = manifests
        .iter()
        .map(|p| {
            let s = p.display().to_string();
            if s.contains(|c: char| c.is_ascii_whitespace() || c == '\'') {
                format!(" --config '{}'", s.replace('\'', "'\\''"))
            } else {
                format!(" --config {s}")
            }
        })
        .collect();

    let parallel_flag = if parallel { " --parallel" } else { "" };
    let type_flag = if hook_type == HookType::PreCommit {
        String::new()
    } else {
        format!(" --hook-type {}", hook_type)
    };

    format!(
        "{BLOCK_START}\n\
         # auto-generated by nizm — do not edit\n\
         if ! command -v nizm >/dev/null 2>&1; then\n\
         \x20 echo \"nizm: not found in PATH — install it or run: cargo install nizm\" >&2\n\
         \x20 exit 1\n\
         fi\n\
         nizm run{config_args}{parallel_flag}{type_flag} || exit $?\n\
         {BLOCK_END}"
    )
}

/// Find the line indices of the nizm block markers.
fn find_block_bounds(lines: &[&str]) -> Option<(usize, usize)> {
    let start = lines.iter().position(|l| l.trim() == BLOCK_START)?;
    let end = lines
        .iter()
        .rposition(|l| l.trim() == BLOCK_END)
        .filter(|&e| e > start)?;
    Some((start, end))
}

fn blocks_match(content: &str, expected_block: &str) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    let (start, end) = match find_block_bounds(&lines) {
        Some(b) => b,
        None => return false,
    };
    let existing: Vec<&str> = lines[start..=end].iter().map(|l| l.trim()).collect();
    let expected: Vec<&str> = expected_block.lines().map(|l| l.trim()).collect();
    existing == expected
}

fn has_custom_block_content(content: &str) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    let (start, end) = match find_block_bounds(&lines) {
        Some(b) => b,
        None => return false,
    };

    lines[start + 1..end].iter().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty()
            && !trimmed.starts_with("if ! command -v nizm")
            && !trimmed.starts_with("echo \"nizm:")
            && trimmed != "exit 1"
            && trimmed != "fi"
            && !trimmed.starts_with("nizm run")
            && !trimmed.starts_with('#')
    })
}

fn replace_block(content: &str, new_block: &str) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    let (start, end) = find_block_bounds(&lines).context("nizm block markers not found")?;

    let mut result: Vec<&str> = Vec::new();
    result.extend_from_slice(&lines[..start]);
    for line in new_block.lines() {
        result.push(line);
    }
    if end + 1 < lines.len() {
        result.extend_from_slice(&lines[end + 1..]);
    }

    let mut out = result.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn write_hook(hook_path: &Path, content: &str) -> Result<()> {
    std::fs::write(hook_path, content).context("failed to write pre-commit hook")?;
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(hook_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(hook_path, perms)?;
    }
    Ok(())
}

fn create_nizm_toml(repo_root: &Path) -> Result<()> {
    let content = r#"[hooks]
# example = { cmd = "echo {staged_files}" }
"#;
    std::fs::write(repo_root.join(".nizm.toml"), content)?;
    Ok(())
}
