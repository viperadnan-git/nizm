use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    pub cmd: String,
    pub glob: Option<String>,
}

#[derive(Debug)]
pub struct Hook {
    pub name: String,
    pub cmd: String,
    pub glob: Option<String>,
}

#[derive(Debug)]
pub struct ManifestConfig {
    pub path: PathBuf,
    pub hooks: Vec<Hook>,
}

const MAX_DEPTH: usize = 5;
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
];
const MANIFEST_NAMES: &[&str] = &["pyproject.toml", "package.json", "Cargo.toml", ".nizm.toml"];

pub fn discover_manifests(repo_root: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    walk_manifests(repo_root, repo_root, &mut found, 0)?;
    found.sort();
    Ok(found)
}

fn walk_manifests(dir: &Path, root: &Path, found: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if path.is_file() && MANIFEST_NAMES.contains(&name_str.as_ref()) {
            found.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        } else if path.is_dir() && !SKIP_DIRS.contains(&name_str.as_ref()) {
            walk_manifests(&path, root, found, depth + 1)?;
        }
    }

    Ok(())
}

pub fn parse_manifest(repo_root: &Path, manifest_path: &Path) -> Result<ManifestConfig> {
    let full_path = repo_root.join(manifest_path);
    let filename = full_path
        .file_name()
        .and_then(|f| f.to_str())
        .context("invalid manifest path")?;

    let content = std::fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read {}", full_path.display()))?;

    let hook_map = match filename {
        "pyproject.toml" => parse_pyproject(&content)?,
        "Cargo.toml" => parse_cargo(&content)?,
        "package.json" => parse_package_json(&content)?,
        ".nizm.toml" => parse_nizm_toml(&content)?,
        _ => anyhow::bail!("unsupported manifest: {filename}"),
    };

    let hooks = hook_map
        .into_iter()
        .map(|(name, cfg)| Hook {
            name,
            cmd: cfg.cmd,
            glob: cfg.glob,
        })
        .collect();

    Ok(ManifestConfig {
        path: manifest_path.to_path_buf(),
        hooks,
    })
}

// --- TOML parsers ---

#[derive(Deserialize)]
struct PyprojectRoot {
    tool: Option<ToolSection>,
}

#[derive(Deserialize)]
struct ToolSection {
    nizm: Option<NizmSection>,
}

#[derive(Deserialize)]
struct NizmSection {
    hooks: Option<IndexMap<String, HookConfig>>,
}

fn parse_pyproject(content: &str) -> Result<IndexMap<String, HookConfig>> {
    let root: PyprojectRoot = toml_edit::de::from_str(content)?;
    Ok(root
        .tool
        .and_then(|t| t.nizm)
        .and_then(|n| n.hooks)
        .unwrap_or_default())
}

#[derive(Deserialize)]
struct CargoRoot {
    package: Option<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    metadata: Option<CargoMetadata>,
}

#[derive(Deserialize)]
struct CargoMetadata {
    nizm: Option<NizmSection>,
}

fn parse_cargo(content: &str) -> Result<IndexMap<String, HookConfig>> {
    let root: CargoRoot = toml_edit::de::from_str(content)?;
    Ok(root
        .package
        .and_then(|p| p.metadata)
        .and_then(|m| m.nizm)
        .and_then(|n| n.hooks)
        .unwrap_or_default())
}

fn parse_nizm_toml(content: &str) -> Result<IndexMap<String, HookConfig>> {
    let section: NizmSection = toml_edit::de::from_str(content)?;
    Ok(section.hooks.unwrap_or_default())
}

// --- JSON parser ---

fn parse_package_json(content: &str) -> Result<IndexMap<String, HookConfig>> {
    let root: serde_json::Value = serde_json::from_str(content)?;
    let hooks = root
        .get("nizm")
        .and_then(|n| n.get("hooks"))
        .and_then(|h| h.as_object());

    match hooks {
        Some(map) => {
            let parsed: IndexMap<String, HookConfig> =
                serde_json::from_value(serde_json::Value::Object(map.clone()))?;
            Ok(parsed)
        }
        None => Ok(IndexMap::new()),
    }
}
