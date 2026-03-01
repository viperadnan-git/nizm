mod config;
mod doctor;
mod git;
mod init;
mod installer;
mod knowledge;
mod runner;
mod stash;
mod style;
mod uninstaller;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

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

        /// Run against all tracked files instead of staged
        #[arg(long)]
        all: bool,

        /// Hook type to run (default: pre-commit)
        #[arg(long, default_value = "pre-commit")]
        hook_type: String,
    },
    /// Install pre-commit hook
    Install {
        /// Manifest paths to bake into hook (skips interactive selection)
        #[arg(long)]
        config: Vec<PathBuf>,

        /// Bake --parallel flag into hook script
        #[arg(long)]
        parallel: bool,

        /// Overwrite existing hook without prompting
        #[arg(long)]
        force: bool,
    },
    /// Diagnose pre-commit hook health
    Doctor,
    /// Scan dev-dependencies and suggest hooks
    Init {
        /// Hook names to add (skips interactive selection)
        #[arg(value_name = "HOOK")]
        hooks: Vec<String>,
    },
    /// List configured hooks
    Ls,
    /// Remove nizm from the project
    Uninstall {
        /// Also remove nizm hook config from manifests
        #[arg(long)]
        purge: bool,
    },
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
            all,
            hook_type,
        } => {
            let ht = config::HookType::from_str(&hook_type)
                .ok_or_else(|| anyhow::anyhow!("unknown hook type: {hook_type}"))?;
            run(repo_root, config, hook, parallel, all, ht)
        }
        Commands::Install {
            config,
            parallel,
            force,
        } => {
            installer::install(&repo_root, config, parallel, force)?;
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
        Commands::Ls => {
            ls(&repo_root)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Init { hooks } => {
            init::init(&repo_root, hooks)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Uninstall { purge } => {
            uninstaller::uninstall(&repo_root, purge)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn ls(repo_root: &Path) -> Result<()> {
    let manifests = config::discover_manifests(repo_root)?;

    // Collect all manifests with their hooks
    let parsed: Vec<_> = manifests
        .iter()
        .filter_map(|path| {
            config::parse_manifest(repo_root, path)
                .ok()
                .filter(|c| !c.hooks.is_empty())
        })
        .collect();

    if parsed.is_empty() {
        println!("no hooks configured");
        return Ok(());
    }

    // Compute column widths across all hooks
    let mut w_name = 0usize;
    let mut w_cmd = 0usize;
    let mut w_glob = 0usize;
    for cfg in &parsed {
        for hook in &cfg.hooks {
            w_name = w_name.max(hook.name.len());
            w_cmd = w_cmd.max(hook.cmd.len());
            w_glob = w_glob.max(hook.glob.as_deref().unwrap_or("-").len());
        }
    }

    for (i, cfg) in parsed.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", style::bold(&cfg.path.display().to_string()));
        for hook in &cfg.hooks {
            let glob = hook.glob.as_deref().unwrap_or("-");
            let ht = if hook.hook_type == config::HookType::PreCommit {
                String::new()
            } else {
                format!("  [{}]", hook.hook_type)
            };
            println!(
                "  {:<w_name$}  {:<w_cmd$}  {:<w_glob$}{}",
                hook.name, hook.cmd, glob, ht
            );
        }
    }

    Ok(())
}

fn run(
    repo_root: PathBuf,
    config_paths: Vec<PathBuf>,
    hook_filter: Option<String>,
    parallel: bool,
    all: bool,
    hook_type: config::HookType,
) -> Result<ExitCode> {
    let configs: Vec<_> = if config_paths.is_empty() {
        config::discover_manifests(&repo_root)?
            .into_iter()
            .filter_map(|p| config::parse_manifest(&repo_root, &p).ok())
            .collect()
    } else {
        config_paths
            .iter()
            .map(|p| config::parse_manifest(&repo_root, p))
            .collect::<Result<Vec<_>>>()?
    };

    // Filter hooks by type, keep only manifests with matching hooks
    let configs: Vec<config::ManifestConfig> = configs
        .into_iter()
        .filter_map(|mut c| {
            c.hooks.retain(|h| h.hook_type == hook_type);
            if c.hooks.is_empty() { None } else { Some(c) }
        })
        .collect();

    if configs.is_empty() {
        println!("{} no hooks configured", style::bold("nizm:"));
        return Ok(ExitCode::SUCCESS);
    }

    let files = if all {
        git::tracked_files()?
    } else {
        git::staged_files()?
    };

    if files.is_empty() {
        println!(
            "{} no {} files — skipping",
            style::bold("nizm:"),
            if all { "tracked" } else { "staged" }
        );
        return Ok(ExitCode::SUCCESS);
    }

    println!(
        "{} running against {} {} {}",
        style::bold("nizm:"),
        files.len(),
        if all { "tracked" } else { "staged" },
        if files.len() == 1 { "file" } else { "files" }
    );

    let mut guard = if all {
        None
    } else {
        stash::StashGuard::new(&files)?
    };

    let prepared: Vec<_> = configs
        .iter()
        .map(|c| {
            let dir = c.path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let abs = repo_root.join(dir);
            (c, dir, abs)
        })
        .collect();

    let skip_set: HashSet<String> = std::env::var("NIZM_SKIP")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let run_start = Instant::now();

    let failed = if parallel && prepared.len() > 1 {
        run_parallel(&prepared, &files, hook_filter.as_deref(), &skip_set)
    } else {
        run_sequential(&prepared, &files, hook_filter.as_deref(), &skip_set)
    };

    println!(
        "{} done in {}",
        style::bold("nizm:"),
        runner::format_duration(run_start.elapsed())
    );

    if !all {
        // FR-19: Auto-add files modified by hooks.
        let modified = git::modified_staged_files(&files)?;
        if !modified.is_empty() {
            git::add_files(&modified)?;
            println!(
                "{} {}",
                style::bold("nizm:"),
                style::green(&format!(
                    "auto-staged {} modified {}",
                    modified.len(),
                    if modified.len() == 1 { "file" } else { "files" }
                ))
            );
        }

        if let Some(ref mut g) = guard {
            g.restore()?;
        }
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
    skip_set: &HashSet<String>,
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

            if skip_set.contains(&hook.name) {
                println!(
                    "  {} {}",
                    style::bold(&hook.name),
                    style::yellow("skipped (NIZM_SKIP)")
                );
                continue;
            }

            match runner::exec_hook(hook, files, manifest_dir, abs_cwd) {
                Ok((code, elapsed, count)) if code != 0 => {
                    eprintln!(
                        "  {} {} {}",
                        style::bold(&hook.name),
                        style::red_bold(&format!("failed (exit {code})")),
                        style::dim(&format!(
                            "{count} {} ({})",
                            if count == 1 { "file" } else { "files" },
                            runner::format_duration(elapsed)
                        ))
                    );
                    failed = true;
                }
                Ok((_, elapsed, count)) if count > 0 => {
                    println!(
                        "  {} {}",
                        style::bold(&hook.name),
                        style::dim(&format!(
                            "{count} {} ({})",
                            if count == 1 { "file" } else { "files" },
                            runner::format_duration(elapsed)
                        ))
                    );
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
    count: usize,
    elapsed: std::time::Duration,
}

fn run_parallel(
    prepared: &[PreparedManifest],
    files: &[String],
    hook_filter: Option<&str>,
    skip_set: &HashSet<String>,
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

                        if skip_set.contains(&hook.name) {
                            output.push(HookResult {
                                stdout: format!(
                                    "  {} {}\n",
                                    style::bold(&hook.name),
                                    style::yellow("skipped (NIZM_SKIP)")
                                ),
                                stderr: String::new(),
                                name: hook.name.clone(),
                                code: 0,
                                count: 0,
                                elapsed: std::time::Duration::ZERO,
                            });
                            continue;
                        }

                        match runner::exec_hook_captured(hook, files, manifest_dir, abs_cwd) {
                            Ok((code, elapsed, count, stdout, stderr)) => {
                                if code != 0 {
                                    any_failed.store(true, Ordering::Relaxed);
                                }
                                output.push(HookResult {
                                    stdout,
                                    stderr,
                                    name: hook.name.clone(),
                                    code,
                                    count,
                                    elapsed,
                                });
                            }
                            Err(e) => {
                                any_failed.store(true, Ordering::Relaxed);
                                output.push(HookResult {
                                    stdout: String::new(),
                                    stderr: format!("  {} {e}\n", hook.name),
                                    name: hook.name.clone(),
                                    code: 1,
                                    count: 0,
                                    elapsed: std::time::Duration::ZERO,
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
            let dim_info = if r.count > 0 {
                style::dim(&format!(
                    "{} {} ({})",
                    r.count,
                    if r.count == 1 { "file" } else { "files" },
                    runner::format_duration(r.elapsed)
                ))
            } else {
                String::new()
            };

            if r.code != 0 {
                if !r.stdout.is_empty() {
                    print!("{}", r.stdout);
                }
                if !r.stderr.is_empty() {
                    eprint!("{}", r.stderr);
                }
                eprintln!(
                    "  {} {} {}",
                    style::bold(&r.name),
                    style::red_bold(&format!("failed (exit {})", r.code)),
                    dim_info
                );
            } else if !r.stdout.is_empty() {
                // Skip/success with output
                print!("{}", r.stdout);
            } else if r.count > 0 {
                println!("  {} {}", style::bold(&r.name), dim_info);
            }
        }
    }

    any_failed.load(Ordering::Relaxed)
}
