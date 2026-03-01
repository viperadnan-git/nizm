use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookType {
    PreCommit,
    PrePush,
    CommitMsg,
    PrepareCommitMsg,
}

impl HookType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pre-commit" => Some(Self::PreCommit),
            "pre-push" => Some(Self::PrePush),
            "commit-msg" => Some(Self::CommitMsg),
            "prepare-commit-msg" => Some(Self::PrepareCommitMsg),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreCommit => "pre-commit",
            Self::PrePush => "pre-push",
            Self::CommitMsg => "commit-msg",
            Self::PrepareCommitMsg => "prepare-commit-msg",
        }
    }
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub const ALL_HOOK_TYPES: &[HookType] = &[
    HookType::PreCommit,
    HookType::PrePush,
    HookType::CommitMsg,
    HookType::PrepareCommitMsg,
];

#[derive(Debug, Clone, Deserialize)]
pub struct HookConfig {
    pub cmd: String,
    pub glob: Option<String>,
    pub r#type: Option<String>,
}

#[derive(Debug)]
pub struct Hook {
    pub name: String,
    pub cmd: String,
    pub glob: Option<String>,
    pub hook_type: HookType,
}

#[derive(Debug)]
pub struct ManifestConfig {
    pub path: PathBuf,
    pub hooks: Vec<Hook>,
}

/// Per-hook parse result for doctor's lenient parsing.
#[derive(Debug)]
pub enum HookResult {
    Ok(Hook),
    Err { name: String, error: String },
}

/// Manifest parsed leniently — file-level error or per-hook results.
#[derive(Debug)]
pub enum LenientManifest {
    FileError(String),
    Hooks(Vec<HookResult>),
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
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if ft.is_file() && MANIFEST_NAMES.contains(&name_str.as_ref()) {
            found.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        } else if ft.is_dir() && !SKIP_DIRS.contains(&name_str.as_ref()) {
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
        .map(|(name, cfg)| {
            let hook_type = cfg
                .r#type
                .as_deref()
                .map(|t| {
                    HookType::from_str(t).unwrap_or_else(|| {
                        eprintln!("warning: unknown hook type '{t}' for '{name}', defaulting to pre-commit");
                        HookType::PreCommit
                    })
                })
                .unwrap_or(HookType::PreCommit);
            Hook {
                name,
                cmd: cfg.cmd,
                glob: cfg.glob,
                hook_type,
            }
        })
        .collect();

    Ok(ManifestConfig {
        path: manifest_path.to_path_buf(),
        hooks,
    })
}

/// Parse a manifest leniently — per-hook errors instead of failing the whole file.
pub fn parse_manifest_lenient(repo_root: &Path, manifest_path: &Path) -> LenientManifest {
    let full_path = repo_root.join(manifest_path);
    let filename = match full_path.file_name().and_then(|f| f.to_str()) {
        Some(f) => f,
        None => return LenientManifest::FileError("invalid manifest path".to_string()),
    };

    let content = match std::fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) => return LenientManifest::FileError(e.to_string()),
    };

    let raw_hooks = match filename {
        "pyproject.toml" => parse_raw_toml_hooks(&content, &["tool", "nizm", "hooks"]),
        "Cargo.toml" => parse_raw_toml_hooks(&content, &["package", "metadata", "nizm", "hooks"]),
        ".nizm.toml" => parse_raw_toml_hooks(&content, &["hooks"]),
        "package.json" => parse_raw_json_hooks(&content),
        _ => return LenientManifest::FileError(format!("unsupported manifest: {filename}")),
    };

    let entries = match raw_hooks {
        Some(entries) => entries,
        None => return LenientManifest::Hooks(Vec::new()),
    };

    let results = entries
        .into_iter()
        .map(
            |(name, value)| match serde_json::from_value::<HookConfig>(value) {
                Ok(cfg) => {
                    let hook_type = cfg
                        .r#type
                        .as_deref()
                        .map(|t| HookType::from_str(t).unwrap_or(HookType::PreCommit))
                        .unwrap_or(HookType::PreCommit);
                    HookResult::Ok(Hook {
                        name,
                        cmd: cfg.cmd,
                        glob: cfg.glob,
                        hook_type,
                    })
                }
                Err(e) => HookResult::Err {
                    name,
                    error: e.to_string(),
                },
            },
        )
        .collect();

    LenientManifest::Hooks(results)
}

/// Extract hooks table as raw entries from TOML, navigating a key path.
/// Each entry is serialized back to a TOML string and deserialized individually.
fn parse_raw_toml_hooks(
    content: &str,
    key_path: &[&str],
) -> Option<Vec<(String, serde_json::Value)>> {
    let doc: toml_edit::DocumentMut = content.parse().ok()?;
    let mut table = doc.as_table() as &dyn toml_edit::TableLike;
    for &key in key_path {
        table = table.get(key)?.as_table_like()?;
    }
    let mut entries = Vec::new();
    for (key, item) in table.iter() {
        // Wrap each hook value as `x = <value>` and parse via toml_edit::de
        let wrapped = format!("x = {item}");
        match toml_edit::de::from_str::<std::collections::HashMap<String, serde_json::Value>>(
            &wrapped,
        ) {
            Ok(mut map) => {
                if let Some(v) = map.remove("x") {
                    entries.push((key.to_string(), v));
                }
            }
            Err(e) => {
                // Push a value that will fail HookConfig deserialization with the original error
                entries.push((
                    key.to_string(),
                    serde_json::json!({"__parse_error": e.to_string()}),
                ));
            }
        }
    }
    Some(entries)
}

/// Extract hooks from JSON, per-entry.
fn parse_raw_json_hooks(content: &str) -> Option<Vec<(String, serde_json::Value)>> {
    let root: serde_json::Value = serde_json::from_str(content).ok()?;
    let hooks = root.get("nizm")?.get("hooks")?.as_object()?;
    Some(hooks.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
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
    let mut root: serde_json::Value = serde_json::from_str(content)?;
    let hooks = root
        .get_mut("nizm")
        .and_then(|n| n.get_mut("hooks"))
        .and_then(|h| h.as_object_mut())
        .map(std::mem::take);

    match hooks {
        Some(map) => {
            let mut parsed = IndexMap::with_capacity(map.len());
            for (k, v) in map {
                parsed.insert(k, serde_json::from_value(v)?);
            }
            Ok(parsed)
        }
        None => Ok(IndexMap::new()),
    }
}

/// Serialize a JSON value with the given indentation, trailing newline included.
pub fn serialize_json(value: &serde_json::Value, indent: &str) -> Result<String> {
    use serde::Serialize;
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut ser)?;
    let mut output = String::from_utf8(buf)?;
    output.push('\n');
    Ok(output)
}

/// Detect indentation used in a JSON file (first indented line).
pub fn detect_json_indent(content: &str) -> String {
    for line in content.lines().skip(1) {
        let trimmed = line.trim_start();
        if !trimmed.is_empty() {
            let leading = &line[..line.len() - trimmed.len()];
            if !leading.is_empty() {
                return leading.to_string();
            }
        }
    }
    "  ".to_string()
}
