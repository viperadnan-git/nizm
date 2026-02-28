<div align="center">

<br>

<img src="https://res.cloudinary.com/viperadnan/image/upload/v1772287432/nizm.svg" alt="nizm" width="120">

<h1>nizm</h1>

**Lightweight, zero-config pre-commit hooks**

[![Crates.io][crate-badge]][crate-url]
[![npm][npm-badge]][npm-url]
[![CI][ci-badge]][ci-url]
[![License][license-badge]][license-url]

[Features](#features) · [Quick Start](#quick-start) · [Installation](#installation) · [Configuration](#configuration) · [Commands](#commands) · [How It Works](#how-it-works)

</div>

**nizm** (from Arabic _nizam_ — system/order) is a fast, native CLI that runs your formatters and linters at the git pre-commit stage. It reads hook definitions straight from your existing project manifests — no `.yaml` files, no managed environments. Unlike `pre-commit`, nizm doesn't install tools for you; it trusts the ones already in your dev-dependencies and local `PATH`.

```console
$ nizm run
  ruff check --fix (3 file(s))
  mypy (3 file(s))
  ✓ all hooks passed (0.24s)
```

## Features

- **Zero config** — hooks live in your existing manifest files
- **Fast** — native Rust binary, no Python/Node runtime overhead
- **Partial staging** — stashes unstaged changes, runs hooks on staged content only, restores cleanly
- **Scope filtering** — each hook only sees files matching its glob pattern
- **Monorepo-ready** — per-directory CWD isolation, multiple manifests, parallel execution
- **Auto-add** — files modified by formatters are automatically re-staged
- **Smart init** — scans dev-dependencies, suggests hooks it already knows about
- **Self-diagnosing** — `nizm doctor` verifies your setup and offers repairs

## Quick Start

```bash
# Install
npm install --save-dev nizm   # or: cargo install nizm

# Scan your project and inject hooks into your manifest
nizm init

# Install the git hook
nizm install

# That's it. Next git commit triggers your hooks automatically.
```

## Installation

<details>
<summary><strong>Cargo (from source)</strong></summary>

```bash
cargo install nizm
```

</details>

<details>
<summary><strong>npm / bun / pnpm / yarn</strong></summary>

```bash
npm install --save-dev nizm
```

Platform-native binary — zero Node.js overhead at runtime. The postinstall script replaces the JS wrapper with your platform's prebuilt binary.

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
rustfmt = { cmd = "cargo fmt",                    glob = "*.rs" }
clippy  = { cmd = "cargo clippy -- -D warnings",  glob = "*.rs" }
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

### `nizm run [HOOK] [--config <path>...] [--parallel]`

Runs hooks against staged files. This is what the git hook calls.

```console
$ nizm run
  prettier --write (5 file(s))
  eslint --fix (5 file(s))
  ✓ all hooks passed (0.41s)
```

| Flag              | Description                                                             |
| :---------------- | :---------------------------------------------------------------------- |
| `HOOK`            | Run a single hook by name                                               |
| `--config <path>` | Explicit manifest paths (repeatable). Skips auto-discovery.             |
| `--parallel`      | Run manifests concurrently. Hooks within each manifest stay sequential. |

### `nizm install [--config <path>...] [--parallel] [--force]`

Writes a `pre-commit` hook into `.git/hooks/` that calls `nizm run` with baked-in config paths. If a hook already exists, nizm appends its block — existing hooks are preserved.

```console
$ nizm install --config pyproject.toml --config frontend/package.json
  ✓ installed pre-commit hook
```

| Flag              | Description                                                     |
| :---------------- | :-------------------------------------------------------------- |
| `--config <path>` | Bake specific manifests into the hook script (non-interactive). |
| `--parallel`      | Bake `--parallel` flag into the hook script.                    |
| `--force`         | Overwrite modified nizm blocks without prompting.               |

> [!NOTE]
> Without `--config`, nizm discovers manifests and shows an interactive picker.

### `nizm init [HOOK...]`

Scans dev-dependencies across all manifests, matches them against known tools, and injects hook definitions.

```console
$ nizm init
  Found dev-dependencies:
    pyproject.toml: ruff, mypy
    package.json: prettier, eslint
  ? Select hooks to add: [ruff, mypy, prettier, eslint]
  ✓ injected 2 hooks into pyproject.toml
  ✓ injected 2 hooks into package.json
```

Pass hook names directly to skip the interactive prompt:

```bash
nizm init ruff prettier
```

**Known tools:** `ruff` · `black` · `mypy` · `prettier` · `eslint` · `biome` · `rustfmt` · `clippy`

> [!TIP]
> For Rust projects, `rustfmt` and `clippy` are suggested automatically when a `[package]` section exists — no dev-dependency needed.

### `nizm uninstall [--purge]`

Removes nizm from the project. Deletes the nizm block from the pre-commit hook (preserving any foreign hooks in the same file). Optionally purges hook config from all manifests.

```console
$ nizm uninstall --purge
  nizm block removed from pre-commit hook
  cleaned pyproject.toml
  cleaned package.json
```

| Flag      | Description                                                              |
| :-------- | :----------------------------------------------------------------------- |
| `--purge` | Also remove nizm hook config from manifests (interactive prompt if omitted). |

### `nizm doctor`

Diagnoses your hook setup and offers automatic repair.

```console
$ nizm doctor
  ✓ pre-commit hook exists
  ✓ hook is nizm-managed
  ✓ config files valid
  ✓ hook commands found in PATH
```

**Checks performed:**

1. Hook file exists at `.git/hooks/pre-commit`
2. Hook is nizm-managed (not overwritten by another tool)
3. Baked config paths exist and parse successfully
4. Hook commands are resolvable in `PATH`

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
[npm-badge]: https://img.shields.io/npm/v/nizm?style=flat-square&labelColor=1a1a2e&color=0f3460
[npm-url]: https://www.npmjs.com/package/nizm
[ci-badge]: https://img.shields.io/github/actions/workflow/status/viperadnan-git/nizm/ci.yml?style=flat-square&labelColor=1a1a2e&label=CI
[ci-url]: https://github.com/viperadnan-git/nizm/actions/workflows/ci.yml
[license-badge]: https://img.shields.io/badge/license-MIT-16c79a?style=flat-square&labelColor=1a1a2e
[license-url]: https://github.com/viperadnan-git/nizm/blob/main/LICENSE
