use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{
    config::{self, HookResult, HookType, LenientManifest},
    installer, style,
};

const ALL_HOOK_TYPES: &[HookType] = &[
    HookType::PreCommit,
    HookType::PrePush,
    HookType::CommitMsg,
    HookType::PrepareCommitMsg,
];

struct Counters {
    pass: usize,
    fail: usize,
    fixes: Vec<String>,
}

impl Counters {
    fn ok(&mut self) {
        self.pass += 1;
    }
    fn err(&mut self, fix: String) {
        self.fail += 1;
        self.fixes.push(fix);
    }
}

pub fn doctor(repo_root: &Path) -> Result<bool> {
    let mut c = Counters {
        pass: 0,
        fail: 0,
        fixes: Vec::new(),
    };

    let ok = style::green("✓");
    let err = style::red_bold("✗");

    // Collect installed hook types
    let mut installed: Vec<(HookType, String)> = Vec::new();
    for ht in ALL_HOOK_TYPES {
        let hook_path = repo_root.join(format!(".git/hooks/{}", ht.as_str()));
        if let Ok(content) = std::fs::read_to_string(&hook_path)
            && installer::is_nizm_managed(&content)
        {
            installed.push((*ht, content));
        }
    }

    println!("{}", style::bold("hooks"));

    if installed.is_empty() {
        // Parse all manifests leniently
        let manifests = config::discover_manifests(repo_root)?;
        let parsed: Vec<_> = manifests
            .iter()
            .map(|p| (p.clone(), config::parse_manifest_lenient(repo_root, p)))
            .collect();

        // Collect hook types from successful parses
        let mut hook_types = std::collections::HashSet::new();
        let mut has_any_hooks = false;
        for (_, lm) in &parsed {
            if let LenientManifest::Hooks(results) = lm {
                for r in results {
                    has_any_hooks = true;
                    if let HookResult::Ok(hook) = r {
                        hook_types.insert(hook.hook_type);
                    }
                }
            }
        }

        if !has_any_hooks {
            // Check if any manifest had file errors
            let file_errors: Vec<_> = parsed
                .iter()
                .filter(|(_, lm)| matches!(lm, LenientManifest::FileError(_)))
                .collect();

            if file_errors.is_empty() {
                println!("  no config found");
                c.err("run `nizm init` to configure hooks".to_string());
            } else {
                // Show file-level errors
                for (path, lm) in &file_errors {
                    if let LenientManifest::FileError(e) = lm {
                        println!("  └ {} — {} {}", path.display(), style::red_bold(e), err);
                        c.err(format!("fix {}", path.display()));
                    }
                }
            }
        } else {
            // Default to pre-commit if no hook types could be determined
            if hook_types.is_empty() {
                hook_types.insert(HookType::PreCommit);
            }

            for ht in ALL_HOOK_TYPES {
                if !hook_types.contains(ht) {
                    continue;
                }
                println!(
                    "  {} {} {}",
                    style::bold(ht.as_str()),
                    style::red_bold("(not installed)"),
                    err
                );
                c.err(format!("run `nizm install` to set up {} hook", ht.as_str()));
            }

            // Show config tree
            let relevant: Vec<_> = parsed
                .into_iter()
                .filter(|(_, lm)| !matches!(lm, LenientManifest::Hooks(h) if h.is_empty()))
                .collect();
            print_lenient_configs(&relevant, "  ", &ok, &err, &mut c);
        }
    } else {
        for (ht, content) in &installed {
            println!("  {} (nizm-managed) {}", style::bold(ht.as_str()), ok);
            c.ok();

            let config_paths = parse_baked_configs(content);
            if config_paths.is_empty() {
                println!("  └ no nizm run line {}", err);
                c.err(format!("reinstall {} hook: nizm install", ht.as_str()));
                continue;
            }

            let configs: Vec<_> = config_paths
                .iter()
                .map(|p| {
                    let full = repo_root.join(p);
                    if !full.exists() {
                        (
                            p.clone(),
                            LenientManifest::FileError("file not found".to_string()),
                        )
                    } else {
                        (p.clone(), config::parse_manifest_lenient(repo_root, p))
                    }
                })
                .collect();

            print_lenient_configs(&configs, "  ", &ok, &err, &mut c);
        }
    }

    // Summary
    println!();
    if c.fail == 0 {
        println!("{}", style::green(&format!("all {} checks passed", c.pass)));
    } else {
        println!(
            "{} passed, {}",
            c.pass,
            style::red_bold(&format!("{} failed", c.fail))
        );
    }

    if !c.fixes.is_empty() {
        println!("\n{}:", style::bold("suggested fixes"));
        for fix in &c.fixes {
            println!("  → {fix}");
        }
    }

    Ok(c.fail == 0)
}

/// Print config trees with lenient parse results.
fn print_lenient_configs(
    configs: &[(PathBuf, LenientManifest)],
    indent: &str,
    ok: &str,
    err: &str,
    c: &mut Counters,
) {
    for (ci, (path, lm)) in configs.iter().enumerate() {
        let last = ci == configs.len() - 1;
        let branch = if last { "└" } else { "├" };
        let cont = if last { " " } else { "│" };

        match lm {
            LenientManifest::FileError(e) => {
                println!(
                    "{indent}{branch} {} — {} {}",
                    path.display(),
                    style::red_bold(e),
                    err
                );
                c.err(format!("fix {}", path.display()));
            }
            LenientManifest::Hooks(results) if results.is_empty() => {
                println!(
                    "{indent}{branch} {} — no hooks defined {}",
                    path.display(),
                    style::yellow("~")
                );
                c.ok();
            }
            LenientManifest::Hooks(results) => {
                // Config is valid if at least one hook parsed
                let any_ok = results.iter().any(|r| matches!(r, HookResult::Ok(_)));
                let all_ok = results.iter().all(|r| matches!(r, HookResult::Ok(_)));
                if all_ok {
                    println!("{indent}{branch} {} {}", path.display(), ok);
                    c.ok();
                } else if any_ok {
                    println!("{indent}{branch} {} {}", path.display(), style::yellow("~"));
                    c.ok();
                } else {
                    println!("{indent}{branch} {} {}", path.display(), err);
                    c.err(format!("fix hooks in {}", path.display()));
                }

                let hook_indent = format!("{indent}{cont}  ");
                for (hi, result) in results.iter().enumerate() {
                    let last_hook = hi == results.len() - 1;
                    let h_branch = if last_hook { "└" } else { "├" };
                    match result {
                        HookResult::Ok(hook) => {
                            let exe = extract_executable(&hook.cmd);
                            if command_exists(exe) {
                                println!("{hook_indent}{h_branch} {} ({}) {}", hook.name, exe, ok);
                                c.ok();
                            } else {
                                println!(
                                    "{hook_indent}{h_branch} {} — {} {}",
                                    hook.name,
                                    style::red_bold(&format!("\"{}\" not found in PATH", exe)),
                                    err
                                );
                                c.err(format!("install \"{}\"", exe));
                            }
                        }
                        HookResult::Err { name, error } => {
                            println!(
                                "{hook_indent}{h_branch} {} — {} {}",
                                name,
                                style::red_bold(error),
                                err
                            );
                            c.err(format!("fix \"{}\" in {}", name, path.display()));
                        }
                    }
                }
            }
        }
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
