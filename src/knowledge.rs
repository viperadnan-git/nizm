pub struct ToolEntry {
    /// Package names that identify this tool in dev-dependencies.
    pub packages: &'static [&'static str],
    pub name: &'static str,
    pub cmd: &'static str,
    pub glob: Option<&'static str>,
}

pub static TOOLS: &[ToolEntry] = &[
    // Python — formatters first, then linters
    ToolEntry {
        packages: &["black"],
        name: "black",
        cmd: "black --check {staged_files}",
        glob: Some("*.py"),
    },
    ToolEntry {
        packages: &["ruff"],
        name: "ruff",
        cmd: "ruff check {staged_files}",
        glob: Some("*.py"),
    },
    ToolEntry {
        packages: &["mypy"],
        name: "mypy",
        cmd: "mypy {staged_files}",
        glob: Some("*.py"),
    },
    // JavaScript / TypeScript — formatters first, then linters
    ToolEntry {
        packages: &["prettier"],
        name: "prettier",
        cmd: "npx prettier --check {staged_files}",
        glob: None,
    },
    ToolEntry {
        packages: &["eslint"],
        name: "eslint",
        cmd: "npx eslint {staged_files}",
        glob: Some("*.{js,jsx,ts,tsx}"),
    },
    ToolEntry {
        packages: &["@biomejs/biome"],
        name: "biome",
        cmd: "npx @biomejs/biome check {staged_files}",
        glob: None,
    },
];

/// Implicit Rust tools — suggested when a Cargo.toml with [package] is found.
pub static RUST_IMPLICIT: &[ToolEntry] = &[
    ToolEntry {
        packages: &[],
        name: "rustfmt",
        cmd: "cargo fmt -- --check",
        glob: Some("*.rs"),
    },
    ToolEntry {
        packages: &[],
        name: "clippy",
        cmd: "cargo clippy -- -D warnings",
        glob: Some("*.rs"),
    },
];

pub fn lookup(dep_name: &str) -> Option<&'static ToolEntry> {
    TOOLS.iter().find(|t| t.packages.contains(&dep_name))
}
