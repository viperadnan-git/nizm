use anyhow::{Context, Result};
use dialoguer::{Confirm, MultiSelect};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::{config, style};

pub const HOOK_MARKER: &str = "# nizm-managed";

pub fn install(repo_root: &Path, parallel: bool) -> Result<()> {
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

    let labels: Vec<String> = manifests.iter().map(|p| p.display().to_string()).collect();
    let selections = MultiSelect::new()
        .with_prompt("select manifests (space = toggle, enter = confirm)")
        .items(&labels)
        .interact()?;

    if selections.is_empty() {
        println!("no manifests selected — aborting");
        return Ok(());
    }

    let selected: Vec<_> = selections.iter().map(|&i| &manifests[i]).collect();

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
        }
    }

    let hook_path = repo_root.join(".git/hooks/pre-commit");
    if hook_path.exists() {
        let content = std::fs::read_to_string(&hook_path)?;
        if content.contains(HOOK_MARKER) {
            println!("existing nizm hook found — updating");
        } else if !Confirm::new()
            .with_prompt("pre-commit hook exists (not nizm) — overwrite?")
            .default(false)
            .interact()?
        {
            println!("{}", style::yellow("aborting — existing hook preserved"));
            return Ok(());
        }
    }

    bake_hook(repo_root, &selected, parallel)?;
    println!("{}", style::green("pre-commit hook installed"));
    Ok(())
}

fn bake_hook(repo_root: &Path, manifests: &[&std::path::PathBuf], parallel: bool) -> Result<()> {
    let hooks_dir = repo_root.join(".git/hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    let config_args: String = manifests
        .iter()
        .map(|p| format!(" --config {}", p.display()))
        .collect();

    let parallel_flag = if parallel { " --parallel" } else { "" };

    let script = format!(
        "#!/bin/sh\n\
         {HOOK_MARKER}\n\
         if ! command -v nizm >/dev/null 2>&1; then\n\
         \x20 echo \"nizm: not found in PATH — install it or run: cargo install nizm\" >&2\n\
         \x20 exit 1\n\
         fi\n\
         exec nizm run{config_args}{parallel_flag}\n"
    );

    let hook_path = hooks_dir.join("pre-commit");
    std::fs::write(&hook_path, &script).context("failed to write pre-commit hook")?;

    let mut perms = std::fs::metadata(&hook_path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&hook_path, perms)?;

    Ok(())
}

fn create_nizm_toml(repo_root: &Path) -> Result<()> {
    let content = r#"[hooks]
# example = { cmd = "echo {staged_files}" }
"#;
    std::fs::write(repo_root.join(".nizm.toml"), content)?;
    Ok(())
}
