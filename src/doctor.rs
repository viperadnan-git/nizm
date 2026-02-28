use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{config, installer, style};

pub fn doctor(repo_root: &Path) -> Result<bool> {
    let mut pass = 0usize;
    let mut fail = 0usize;

    // Check 1: Hook file exists
    let hook_path = repo_root.join(".git/hooks/pre-commit");
    let content = match std::fs::read_to_string(&hook_path) {
        Ok(c) => c,
        Err(_) => {
            println!(
                "  {}: {}",
                style::bold("hook"),
                style::red_bold("NOT FOUND")
            );
            println!("        run `nizm install` to set up");
            fail += 1;
            print_summary(pass, fail);
            return Ok(false);
        }
    };

    // Check 2: nizm block present?
    if !installer::is_nizm_managed(&content) {
        println!(
            "  {}: {} — no nizm block found",
            style::bold("hook"),
            style::red_bold("MISSING")
        );
        println!("        run `nizm install` to add nizm block");
        fail += 1;

        if dialoguer::Confirm::new()
            .with_prompt("  install now?")
            .default(true)
            .interact()?
        {
            installer::install(repo_root, Vec::new(), false, false)?;
            println!();
            return doctor(repo_root);
        }
    } else {
        println!(
            "  {}: {} (nizm-managed)",
            style::bold("hook"),
            style::green("ok")
        );
        pass += 1;
    }

    // Check 3 & 4: Baked config paths exist and parse
    let configs = parse_baked_configs(&content);
    if configs.is_empty() {
        println!(
            "  {}: {} — no nizm run line in hook script",
            style::bold("configs"),
            style::red_bold("BROKEN")
        );
        println!("          run `nizm install` to repair");
        fail += 1;

        if dialoguer::Confirm::new()
            .with_prompt("  repair now?")
            .default(true)
            .interact()?
        {
            installer::install(repo_root, Vec::new(), false, true)?;
            println!();
            return doctor(repo_root);
        }

        print_summary(pass, fail);
        return Ok(false);
    }

    println!("  {}:", style::bold("configs"));
    let mut parsed_configs = Vec::new();
    for path in &configs {
        let full = repo_root.join(path);
        if !full.exists() {
            println!(
                "    {} {} — file not found",
                style::red_bold("ERR"),
                path.display()
            );
            fail += 1;
            continue;
        }

        match config::parse_manifest(repo_root, path) {
            Ok(cfg) if cfg.hooks.is_empty() => {
                println!(
                    "    {} {} — no hooks defined",
                    style::yellow("WARN"),
                    path.display()
                );
                pass += 1;
            }
            Ok(cfg) => {
                println!("    {} {}", style::green("ok "), path.display());
                pass += 1;
                parsed_configs.push(cfg);
            }
            Err(e) => {
                println!(
                    "    {} {} — parse error: {e}",
                    style::red_bold("ERR"),
                    path.display()
                );
                fail += 1;
            }
        }
    }

    // Check 5: Hook commands resolvable
    if !parsed_configs.is_empty() {
        println!("  {}:", style::bold("commands"));
    }
    for cfg in &parsed_configs {
        for hook in &cfg.hooks {
            let exe = extract_executable(&hook.cmd);
            if command_exists(exe) {
                println!("    {} {} ({})", style::green("ok "), hook.name, exe);
                pass += 1;
            } else {
                println!(
                    "    {} {} — \"{}\" not found in PATH",
                    style::red_bold("ERR"),
                    hook.name,
                    exe
                );
                fail += 1;
            }
        }
    }

    print_summary(pass, fail);
    Ok(fail == 0)
}

fn print_summary(pass: usize, fail: usize) {
    println!();
    if fail == 0 {
        println!("  {}", style::green(&format!("all {pass} checks passed")));
    } else {
        println!(
            "  {} passed, {}",
            pass,
            style::red_bold(&format!("{fail} failed"))
        );
    }
}

/// Extract --config paths from the baked hook script.
fn parse_baked_configs(content: &str) -> Vec<PathBuf> {
    let mut configs = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("nizm") {
            continue;
        }
        let mut parts = trimmed.split_whitespace().peekable();
        while let Some(token) = parts.next() {
            if token == "--config"
                && let Some(path) = parts.next()
            {
                configs.push(PathBuf::from(path));
            }
        }
    }
    configs
}

/// Extract the first executable from a command string.
fn extract_executable(cmd: &str) -> &str {
    cmd.split_whitespace().next().unwrap_or(cmd)
}

/// Check if an executable exists in PATH using POSIX `command -v`.
fn command_exists(exe: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {exe}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
