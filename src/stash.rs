use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use crate::git;

/// Whether a stash is active and needs restoring.
static STASH_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Set by Ctrl+C handler — main thread checks this to break out of hook loop.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn was_interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// RAII guard ensuring stash is restored on all exit paths:
/// - Normal scope exit (Drop)
/// - Panic/unwind (Drop)
/// - Ctrl+C sets INTERRUPTED flag, main thread breaks and runs restore via Drop
pub struct StashGuard;

impl StashGuard {
    /// Returns `None` if no partial staging detected (no stash needed).
    /// NOTE: Guard only activates for partial staging. Non-stash failures (e.g. hook
    /// crashes with fully staged files) are not guarded — no destructive state to undo.
    /// If future features need broader crash protection, extend this guard accordingly.
    pub fn new(staged: &[String]) -> Result<Option<Self>> {
        if !git::has_partial_staging(staged)? {
            return Ok(None);
        }

        println!("nizm: partial staging detected — stashing unstaged changes");

        // FR-18: rescue snapshot before any destructive operation
        git::create_rescue_ref()?;

        // Ctrl+C handler just sets a flag — main thread does the actual restore.
        let _ = ctrlc::set_handler(|| {
            INTERRUPTED.store(true, Ordering::SeqCst);
            eprintln!("\nnizm: interrupted");
        });

        git::stash_keep_index()?;
        STASH_ACTIVE.store(true, Ordering::SeqCst);

        Ok(Some(Self))
    }

    pub fn restore(&mut self) -> Result<()> {
        if STASH_ACTIVE.swap(false, Ordering::SeqCst) {
            git::restore_unstaged()?;
            git::drop_rescue_ref()?;
        }
        Ok(())
    }
}

impl Drop for StashGuard {
    fn drop(&mut self) {
        if STASH_ACTIVE.swap(false, Ordering::SeqCst) {
            // catch_unwind prevents double-panic abort if restore panics during unwind.
            let result = panic::catch_unwind(|| {
                if let Err(e) = git::restore_unstaged() {
                    eprintln!("nizm: failed to restore stash: {e}");
                    eprintln!("nizm: rescue snapshot: git stash apply refs/nizm-backup");
                    return;
                }
                let _ = git::drop_rescue_ref();
            });
            if result.is_err() {
                eprintln!("nizm: panic during stash restore");
                eprintln!("nizm: rescue snapshot: git stash apply refs/nizm-backup");
            }
        }
    }
}
