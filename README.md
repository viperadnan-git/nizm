<div align="center">

<br>

<img src="https://res.cloudinary.com/viperadnan/image/upload/v1772287432/nizm.svg" alt="nizm" width="120">

<h1>nizm</h1>

**Lightweight, zero-config pre-commit hooks**

[![Crates.io][crate-badge]][crate-url]
[![npm][npm-badge]][npm-url]
[![CI][ci-badge]][ci-url]
[![License][license-badge]][license-url]

[Quick Start](#quick-start) · [Installation](#installation) · [Configuration](#configuration) · [Commands](#commands) · [How It Works](#how-it-works)

</div>

**nizm** (from Arabic _nizam_ — system/order) is a fast, native CLI that runs your formatters and linters at the git pre-commit stage. It reads hook definitions straight from your existing project manifests — no `.yaml` files, no managed environments. Unlike `pre-commit`, nizm doesn't install tools for you; it trusts the ones already in your dev-dependencies and local `PATH`.

```console
$ nizm run
nizm: running against 3 staged files
  ruff 3 files (120ms)
  mypy 3 files (340ms)
nizm: done in 461ms
```

## Features

- **Zero config** — hooks live in your existing manifest files
- **Fast** — native Rust binary, no Python/Node runtime overhead
- **Partial staging** — stashes unstaged changes, runs hooks on staged content only, restores cleanly
- **Scope filtering** — each hook only sees files matching its glob pattern
- **Monorepo-ready** — per-directory CWD isolation, multiple manifests, parallel execution
- **Auto-add** — files modified by formatters are automatically re-staged
- **Smart init** — scans dev-dependencies, suggests hooks it already knows about
- **Self-diagnosing** — `nizm doctor` verifies your setup and suggests fixes

## Quick Start

```bash
npm install -g nizm-cli   # or: cargo install nizm
nizm init                 # scans dev-deps, injects hooks, installs git hook
```

That's it. Your next `git commit` runs your hooks automatically.

## Installation

<details open>
<summary><strong>npm / bun / pnpm / yarn</strong></summary>

```bash
npm install -g nizm-cli
```

Platform-native binary — zero Node.js overhead at runtime.

</details>

<details>
<summary><strong>Cargo (from source)</strong></summary>

```bash
cargo install nizm
```

</details>

<details>
<summary><strong>Prebuilt binaries</strong></summary>

Download the latest archive for your platform from [GitHub Releases](https://github.com/viperadnan-git/nizm/releases), extract, and place the binary somewhere on your `PATH`.

</details>

## Configuration

nizm discovers hooks from your project manifests. No separate config file needed.

### pyproject.toml

```toml
[tool.nizm.hooks]
ruff  = { cmd = "ruff check --fix {staged_files}", glob = "*.py" }
black = { cmd = "black {staged_files}",             glob = "*.py" }
mypy  = { cmd = "mypy {staged_files}",              glob = "*.py" }
```

### package.json

```json
{
  "nizm": {
    "hooks": {
      "prettier": { "cmd": "prettier --write {staged_files}" },
      "eslint": {
        "cmd": "eslint --fix {staged_files}",
        "glob": "*.{js,jsx,ts,tsx}"
      }
    }
  }
}
```

### Cargo.toml

```toml
[package.metadata.nizm.hooks]
clippy  = { cmd = "cargo clippy --fix --allow-dirty -- -D warnings", glob = "*.rs" }
rustfmt = { cmd = "cargo fmt",                                       glob = "*.rs" }
```

### .nizm.toml

Standalone config for projects that don't use any of the above, or for repo-root overrides:

```toml
[hooks]
check = { cmd = "make lint" }
test  = { cmd = "make test" }
```

### Hook fields

| Field  | Required | Description                                                          |
| :----- | :------- | :------------------------------------------------------------------- |
| `cmd`  | yes      | Shell command to run. Use `{staged_files}` to receive the file list. |
| `glob` | no       | Filter staged files by pattern (`*.py`, `*.{js,ts}`, `src/**/*.rs`). |

> [!TIP]
> If `{staged_files}` is omitted, the command runs unconditionally when any file in scope is staged.

## Commands

### `nizm init`

Scans dev-dependencies, suggests hooks, and injects them into your manifest.

```console
$ nizm init
  added clippy       cargo clippy --fix --allow-dirty -- -D warnings
  added rustfmt      cargo fmt

  Cargo.toml — [clippy, rustfmt]
pre-commit hook installed
```

Pass hook names directly for non-interactive use: `nizm init ruff prettier`

**Known tools:** `ruff` · `black` · `mypy` · `prettier` · `eslint` · `biome` · `rustfmt` · `clippy`

> [!TIP]
> For Rust projects, `rustfmt` and `clippy` are suggested automatically when a `[package]` section exists — no dev-dependency needed.

### `nizm install`

Writes a git hook into `.git/hooks/` that calls `nizm run`. Existing hooks are preserved.

```console
$ nizm install
scanning for manifests...
  pyproject.toml — [ruff, mypy]
pre-commit hook installed
```

| Flag | Description |
| :--- | :---------- |
| `--config <path>` | Bake specific manifests (repeatable, skips interactive picker) |
| `--parallel` | Bake `--parallel` flag into the hook script |
| `--force` | Overwrite modified nizm blocks without prompting |

### `nizm run`

Runs hooks against staged files. This is what the git hook calls.

```console
$ nizm run
nizm: running against 5 staged files
  clippy 5 files (780ms)
  rustfmt 5 files (210ms)
nizm: done in 991ms
```

| Flag | Description |
| :--- | :---------- |
| `HOOK` | Run a single hook by name |
| `--config <path>` | Explicit manifest paths (repeatable, skips auto-discovery) |
| `--parallel` | Run manifests concurrently |
| `--all` | Run against all tracked files instead of staged |
| `--hook-type <TYPE>` | Hook type to run (default: `pre-commit`) |

### `nizm doctor`

Diagnoses hook health — checks hook files, config validity, and tool availability.

```console
$ nizm doctor
hooks
  pre-commit (nizm-managed) ✓
  └ Cargo.toml ✓
     ├ clippy (cargo) ✓
     └ rustfmt (cargo) ✓

all 4 checks passed
```

### `nizm ls`

Lists all configured hooks across discovered manifests.

```console
$ nizm ls
Cargo.toml
  clippy   cargo clippy --fix --allow-dirty -- -D warnings  *.rs
  rustfmt  cargo fmt                                        *.rs
```

### `nizm uninstall`

Removes nizm hook blocks from `.git/hooks/`. Use `--purge` to also strip hook config from manifests.

```console
$ nizm uninstall --purge
pre-commit hook removed
  cleaned Cargo.toml
```

## How It Works

```
git commit
    │
    ▼
.git/hooks/pre-commit        ← baked by `nizm install`
    │
    ▼
nizm run --config pyproject.toml --config package.json
    │
    ├─ detect partially staged files
    ├─ stash unstaged changes (StashGuard)
    │
    ├─ for each manifest:
    │     ├─ cd to manifest directory
    │     ├─ for each hook:
    │     │     ├─ scope-filter staged files by glob
    │     │     └─ execute cmd with {staged_files}
    │     └─ next hook
    │
    ├─ auto-add files modified by hooks
    ├─ restore unstaged changes from stash
    └─ exit 0 (pass) or exit 1 (fail → commit blocked)
```

### Partial staging

When you stage only part of a file (`git add -p`), nizm stashes the unstaged changes before running hooks. This ensures formatters and linters see exactly what will be committed — not your working tree. After hooks complete, unstaged changes are restored cleanly.

A rescue ref is saved at `refs/nizm-backup` before every stash operation. If anything goes wrong:

```bash
git stash apply refs/nizm-backup
```

### Parallel execution

With `--parallel`, each manifest's hooks run in a separate thread. Hooks within a single manifest stay sequential (so your formatter runs before your linter). Output is captured per-manifest and printed in order — no interleaving.

### Monorepo support

nizm walks your repo (up to 5 levels deep) to discover manifests. Each manifest's hooks run with `cwd` set to that manifest's directory, so tools resolve paths correctly:

```
repo/
├── pyproject.toml          ← ruff, mypy run here
├── frontend/
│   └── package.json        ← prettier, eslint run here
└── services/api/
    └── package.json        ← separate hooks, separate cwd
```

## Building from source

```bash
git clone https://github.com/viperadnan-git/nizm.git
cd nizm
cargo build --release
# Binary at target/release/nizm
```

### Running checks

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
```

## License

[MIT](LICENSE)

---

<div align="center">

Built with Rust. No runtime dependencies. Just fast hooks.

</div>

<!-- link references -->

[crate-badge]: https://img.shields.io/crates/v/nizm?style=flat-square&labelColor=1a1a2e&color=e94560
[crate-url]: https://crates.io/crates/nizm
[npm-badge]: https://img.shields.io/npm/v/nizm-cli?style=flat-square&labelColor=1a1a2e&color=0f3460
[npm-url]: https://www.npmjs.com/package/nizm-cli
[ci-badge]: https://img.shields.io/github/actions/workflow/status/viperadnan-git/nizm/ci.yml?style=flat-square&labelColor=1a1a2e&label=CI
[ci-url]: https://github.com/viperadnan-git/nizm/actions/workflows/ci.yml
[license-badge]: https://img.shields.io/badge/license-MIT-16c79a?style=flat-square&labelColor=1a1a2e
[license-url]: https://github.com/viperadnan-git/nizm/blob/main/LICENSE
