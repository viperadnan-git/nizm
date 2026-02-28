mod config;
mod doctor;
mod git;
mod init;
mod installer;
mod knowledge;
mod runner;
mod stash;
mod style;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "nizm",
    version,
    about = "Lightweight, zero-config pre-commit hooks"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run hooks against staged files
    Run {
        /// Config file paths
        #[arg(long)]
        config: Vec<PathBuf>,

        /// Run only this hook
        #[arg(value_name = "HOOK")]
        hook: Option<String>,
    },
    /// Install pre-commit hook
    Install,
    /// Diagnose pre-commit hook health
    Doctor,
    /// Scan dev-dependencies and suggest hooks
    Init,
}

fn main() -> ExitCode {
    match try_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {e:#}", style::red_bold("Error:"));
            ExitCode::FAILURE
        }
    }
}

fn try_main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let repo_root = git::repo_root()?;

    match cli.command {
        Commands::Run { config, hook } => run(repo_root, config, hook),
        Commands::Install => {
            installer::install(&repo_root)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Doctor => {
            let all_passed = doctor::doctor(&repo_root)?;
            Ok(if all_passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Commands::Init => {
            init::init(&repo_root)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn run(
    repo_root: PathBuf,
    config_paths: Vec<PathBuf>,
    hook_filter: Option<String>,
) -> Result<ExitCode> {
    let configs = if config_paths.is_empty() {
        config::discover_manifests(&repo_root)?
            .into_iter()
            .filter_map(|p| config::parse_manifest(&repo_root, &p).ok())
            .filter(|c| !c.hooks.is_empty())
            .collect()
    } else {
        config_paths
            .iter()
            .map(|p| config::parse_manifest(&repo_root, p))
            .collect::<Result<Vec<_>>>()?
    };

    if configs.is_empty() {
        println!("no hooks configured");
        return Ok(ExitCode::SUCCESS);
    }

    let files = git::staged_files()?;
    if files.is_empty() {
        println!("no staged files — skipping");
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "{} running against {} file(s)",
        style::bold("nizm:"),
        files.len()
    );

    // FR-17: Stash unstaged changes if partial staging detected.
    // StashGuard restores on Drop (scope exit, panic, or after Ctrl+C breaks the loop).
    let mut guard = stash::StashGuard::new(&files)?;

    let mut failed = false;
    'hooks: for manifest in &configs {
        let manifest_dir = manifest
            .path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let abs_cwd = repo_root.join(manifest_dir);

        for hook in &manifest.hooks {
            if stash::was_interrupted() {
                failed = true;
                break 'hooks;
            }

            if let Some(ref filter) = hook_filter
                && hook.name != *filter
            {
                continue;
            }

            let code = runner::exec_hook(hook, &files, manifest_dir, &abs_cwd)?;
            if code != 0 {
                eprintln!(
                    "  {} {}",
                    style::bold(&hook.name),
                    style::red_bold(&format!("failed (exit {code})"))
                );
                failed = true;
            }
        }
    }

    // FR-19: Auto-add files modified by hooks.
    let modified = git::modified_staged_files(&files)?;
    if !modified.is_empty() {
        git::add_files(&modified)?;
        println!(
            "{} {}",
            style::bold("nizm:"),
            style::green(&format!("auto-staged {} modified file(s)", modified.len()))
        );
    }

    // Explicit restore — Drop is the fallback.
    if let Some(ref mut g) = guard {
        g.restore()?;
    }

    if stash::was_interrupted() {
        return Ok(ExitCode::from(130));
    }

    if failed {
        return Ok(ExitCode::FAILURE);
    }

    Ok(ExitCode::SUCCESS)
}
