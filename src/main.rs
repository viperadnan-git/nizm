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
use std::sync::atomic::{AtomicBool, Ordering};

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

        /// Run manifests in parallel
        #[arg(long)]
        parallel: bool,
    },
    /// Install pre-commit hook
    Install {
        /// Bake --parallel flag into hook script
        #[arg(long)]
        parallel: bool,
    },
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
        Commands::Run {
            config,
            hook,
            parallel,
        } => run(repo_root, config, hook, parallel),
        Commands::Install { parallel } => {
            installer::install(&repo_root, parallel)?;
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
    parallel: bool,
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

    let mut guard = stash::StashGuard::new(&files)?;

    let prepared: Vec<_> = configs
        .iter()
        .map(|c| {
            let dir = c.path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let abs = repo_root.join(dir);
            (c, dir, abs)
        })
        .collect();

    let failed = if parallel && prepared.len() > 1 {
        run_parallel(&prepared, &files, hook_filter.as_deref())
    } else {
        run_sequential(&prepared, &files, hook_filter.as_deref())
    };

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

type PreparedManifest<'a> = (&'a config::ManifestConfig, &'a std::path::Path, PathBuf);

fn run_sequential(
    prepared: &[PreparedManifest],
    files: &[String],
    hook_filter: Option<&str>,
) -> bool {
    let mut failed = false;
    'hooks: for (manifest, manifest_dir, abs_cwd) in prepared {
        for hook in &manifest.hooks {
            if stash::was_interrupted() {
                failed = true;
                break 'hooks;
            }

            if let Some(filter) = hook_filter
                && hook.name != filter
            {
                continue;
            }

            match runner::exec_hook(hook, files, manifest_dir, abs_cwd) {
                Ok(code) if code != 0 => {
                    eprintln!(
                        "  {} {}",
                        style::bold(&hook.name),
                        style::red_bold(&format!("failed (exit {code})"))
                    );
                    failed = true;
                }
                Err(e) => {
                    eprintln!(
                        "  {} {}",
                        style::bold(&hook.name),
                        style::red_bold(&format!("error: {e}"))
                    );
                    failed = true;
                }
                _ => {}
            }
        }
    }
    failed
}

struct HookResult {
    stdout: String,
    stderr: String,
    name: String,
    code: i32,
}

fn run_parallel(
    prepared: &[PreparedManifest],
    files: &[String],
    hook_filter: Option<&str>,
) -> bool {
    let any_failed = AtomicBool::new(false);

    let results: Vec<Vec<HookResult>> = std::thread::scope(|s| {
        let handles: Vec<_> = prepared
            .iter()
            .map(|(manifest, manifest_dir, abs_cwd)| {
                let any_failed = &any_failed;
                s.spawn(move || {
                    let mut output = Vec::new();

                    for hook in &manifest.hooks {
                        if stash::was_interrupted() || any_failed.load(Ordering::Relaxed) {
                            break;
                        }

                        if let Some(filter) = hook_filter
                            && hook.name != filter
                        {
                            continue;
                        }

                        match runner::exec_hook_captured(hook, files, manifest_dir, abs_cwd) {
                            Ok((code, stdout, stderr)) => {
                                if code != 0 {
                                    any_failed.store(true, Ordering::Relaxed);
                                }
                                output.push(HookResult {
                                    stdout,
                                    stderr,
                                    name: hook.name.clone(),
                                    code,
                                });
                            }
                            Err(e) => {
                                any_failed.store(true, Ordering::Relaxed);
                                output.push(HookResult {
                                    stdout: String::new(),
                                    stderr: format!("  {} {e}\n", hook.name),
                                    name: hook.name.clone(),
                                    code: 1,
                                });
                            }
                        }
                    }
                    output
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });

    for manifest_output in &results {
        for r in manifest_output {
            if !r.stdout.is_empty() {
                print!("{}", r.stdout);
            }
            if !r.stderr.is_empty() {
                eprint!("{}", r.stderr);
            }
            if r.code != 0 {
                eprintln!(
                    "  {} {}",
                    style::bold(&r.name),
                    style::red_bold(&format!("failed (exit {})", r.code))
                );
            }
        }
    }

    any_failed.load(Ordering::Relaxed)
}
