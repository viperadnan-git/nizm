pub struct ToolEntry {
    /// Package names that identify this tool in dev-dependencies.
    pub packages: &'static [&'static str],
    pub name: &'static str,
    pub cmd: &'static str,
    pub glob: Option<&'static str>,
}

pub static TOOLS: &[ToolEntry] = &[
    // Python
    ToolEntry {
        packages: &["black"],
        name: "black",
        cmd: "black {staged_files}",
        glob: Some("*.py"),
    },
    ToolEntry {
        packages: &["ruff"],
        name: "ruff",
        cmd: "ruff check --fix {staged_files} && ruff format {staged_files}",
        glob: Some("*.py"),
    },
    ToolEntry {
        packages: &["mypy"],
        name: "mypy",
        cmd: "mypy {staged_files}",
        glob: Some("*.py"),
    },
    // JavaScript / TypeScript
    ToolEntry {
        packages: &["prettier"],
        name: "prettier",
        cmd: "prettier --write {staged_files}",
        glob: None,
    },
    ToolEntry {
        packages: &["eslint"],
        name: "eslint",
        cmd: "eslint --fix {staged_files}",
        glob: Some("*.{js,jsx,ts,tsx}"),
    },
    ToolEntry {
        packages: &["@biomejs/biome"],
        name: "biome",
        cmd: "biome check --write {staged_files}",
        glob: None,
    },
];

/// Implicit Rust tools — suggested when a Cargo.toml with [package] is found.
pub static RUST_IMPLICIT: &[ToolEntry] = &[
    ToolEntry {
        packages: &[],
        name: "clippy",
        cmd: "cargo clippy --fix --allow-dirty -- -D warnings",
        glob: Some("*.rs"),
    },
    ToolEntry {
        packages: &[],
        name: "rustfmt",
        cmd: "cargo fmt",
        glob: Some("*.rs"),
    },
];

/// Return all matching tool entries for a given dependency name.
pub fn lookup(dep_name: &str) -> Vec<&'static ToolEntry> {
    TOOLS
        .iter()
        .filter(|t| t.packages.contains(&dep_name))
        .collect()
}
