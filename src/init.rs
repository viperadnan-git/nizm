use anyhow::{Context, Result};
use dialoguer::MultiSelect;
use std::collections::HashSet;
use std::path::Path;

use crate::{
    config, config::detect_json_indent, config::serialize_json, installer, knowledge, style,
};

pub fn init(repo_root: &Path, explicit_hooks: Vec<String>) -> Result<()> {
    let manifests = config::discover_manifests(repo_root)?;
    if manifests.is_empty() {
        println!(
            "{}",
            style::yellow("no manifests found — run `nizm install` first")
        );
        return Ok(());
    }

    let mut suggestions: Vec<Suggestion> = Vec::new();
    let mut any_deps_found = false;

    for manifest_path in &manifests {
        let full_path = repo_root.join(manifest_path);
        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let filename = manifest_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        // Get existing hook names to avoid duplicates
        let existing: HashSet<String> = config::parse_manifest(repo_root, manifest_path)
            .map(|c| c.hooks.into_iter().map(|h| h.name).collect())
            .unwrap_or_default();

        let (deps, is_cargo_package) = read_devdeps(filename, &content);
        if !deps.is_empty() {
            any_deps_found = true;
        }

        for dep in &deps {
            for entry in knowledge::lookup(dep) {
                if !existing.contains(entry.name) {
                    suggestions.push(Suggestion {
                        manifest: manifest_path.clone(),
                        entry,
                    });
                }
            }
        }

        // Rust implicit tools (clippy, rustfmt)
        if is_cargo_package {
            any_deps_found = true;
            for entry in knowledge::RUST_IMPLICIT {
                if !existing.contains(entry.name) {
                    suggestions.push(Suggestion {
                        manifest: manifest_path.clone(),
                        entry,
                    });
                }
            }
        }
    }

    // Deduplicate by (manifest, name), sort by depth (root first)
    let mut seen = HashSet::new();
    suggestions.retain(|s| seen.insert((s.manifest.clone(), s.entry.name)));
    suggestions.sort_by_key(|s| s.manifest.components().count());

    if suggestions.is_empty() {
        if any_deps_found {
            println!(
                "{}",
                style::yellow("no hooks to suggest — all detected tools are already configured")
            );
        } else {
            println!(
                "{}",
                style::yellow("no dev-dependencies found in any manifest")
            );
        }
        return Ok(());
    }

    let selections: Vec<usize> = if !explicit_hooks.is_empty() {
        let indices: Vec<usize> = explicit_hooks
            .iter()
            .filter_map(|name| suggestions.iter().position(|s| s.entry.name == name))
            .collect();
        if indices.is_empty() {
            println!(
                "{}",
                style::yellow("none of the specified hooks match available suggestions")
            );
            return Ok(());
        }
        indices
    } else {
        let multiple_manifests = {
            let mut seen = HashSet::new();
            suggestions.iter().any(|s| !seen.insert(&s.manifest))
        };
        let labels: Vec<String> = suggestions
            .iter()
            .map(|s| {
                if multiple_manifests {
                    format!(
                        "{} {}",
                        s.entry.name,
                        style::dim(&format!("({})", s.manifest.display()))
                    )
                } else {
                    s.entry.name.to_string()
                }
            })
            .collect();

        let sel = MultiSelect::new()
            .with_prompt("select hooks to add (space = toggle, enter = confirm)")
            .items(&labels)
            .interact()?;

        if sel.is_empty() {
            println!("nothing selected");
            return Ok(());
        }
        sel
    };

    let mut added_ruff = false;
    let mut ruff_pyproject: Option<std::path::PathBuf> = None;
    let mut prettier_dirs: Vec<std::path::PathBuf> = Vec::new();

    for &i in &selections {
        let s = &suggestions[i];
        let full_path = repo_root.join(&s.manifest);
        let filename = s
            .manifest
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        inject_hook(filename, &full_path, s.entry)?;
        println!(
            "  {} {} {}",
            style::green("added"),
            style::bold(&format!("{:<12}", s.entry.name)),
            s.entry.cmd
        );

        if s.entry.name.starts_with("ruff") && filename == "pyproject.toml" && !added_ruff {
            ruff_pyproject = Some(full_path.clone());
            added_ruff = true;
        }
        if s.entry.name == "prettier" {
            let dir = full_path.parent().unwrap_or(repo_root);
            prettier_dirs.push(dir.to_path_buf());
        }
    }

    // Inject ruff config into pyproject.toml
    if let Some(ref pyproject_path) = ruff_pyproject
        && inject_ruff_config(pyproject_path)?
    {
        println!(
            "  {} {}",
            style::green("config"),
            style::bold("[tool.ruff.lint] → pyproject.toml")
        );
    }

    // Create .prettierrc next to package.json
    for dir in &prettier_dirs {
        if create_prettierrc(dir)? {
            println!(
                "  {} {}",
                style::green("config"),
                style::bold(".prettierrc created")
            );
        }
    }

    // Collect unique manifests that received hooks
    let mut used_manifests: Vec<std::path::PathBuf> = selections
        .iter()
        .map(|&i| suggestions[i].manifest.clone())
        .collect();
    used_manifests.sort();
    used_manifests.dedup();

    println!();
    installer::install(repo_root, used_manifests, false, false)?;
    Ok(())
}

struct Suggestion {
    manifest: std::path::PathBuf,
    entry: &'static knowledge::ToolEntry,
}

// --- Dev-dependency readers ---

/// Returns (dev_deps, is_cargo_package).
fn read_devdeps(filename: &str, content: &str) -> (Vec<String>, bool) {
    match filename {
        "pyproject.toml" => (read_pyproject_devdeps(content), false),
        "package.json" => (read_packagejson_devdeps(content), false),
        "Cargo.toml" => read_cargo_devdeps(content),
        _ => (Vec::new(), false),
    }
}

fn read_pyproject_devdeps(content: &str) -> Vec<String> {
    let doc: toml_edit::DocumentMut = match content.parse() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    let mut deps = Vec::new();

    // PEP 621: project.optional-dependencies.dev = ["ruff>=0.5"]
    if let Some(list) = doc
        .get("project")
        .and_then(|p| p.get("optional-dependencies"))
        .and_then(|o| o.get("dev"))
        .and_then(|d| d.as_array())
    {
        for item in list.iter() {
            if let Some(s) = item.as_str() {
                deps.push(extract_pkg_name(s));
            }
        }
    }

    // PEP 735: dependency-groups.dev = ["ruff>=0.5"]
    if let Some(list) = doc
        .get("dependency-groups")
        .and_then(|d| d.get("dev"))
        .and_then(|d| d.as_array())
    {
        for item in list.iter() {
            if let Some(s) = item.as_str() {
                deps.push(extract_pkg_name(s));
            }
        }
    }

    // Poetry: tool.poetry.group.dev.dependencies = { ruff = "^0.5" }
    if let Some(table) = doc
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("group"))
        .and_then(|g| g.get("dev"))
        .and_then(|d| d.get("dependencies"))
        .and_then(|d| d.as_table())
    {
        deps.extend(table.iter().map(|(k, _)| k.to_string()));
    }

    deps
}

fn read_packagejson_devdeps(content: &str) -> Vec<String> {
    let val: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    val.get("devDependencies")
        .and_then(|d| d.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default()
}

/// Returns (dev_deps, has_package_section).
fn read_cargo_devdeps(content: &str) -> (Vec<String>, bool) {
    let doc: toml_edit::DocumentMut = match content.parse() {
        Ok(d) => d,
        Err(_) => return (Vec::new(), false),
    };

    let deps = doc
        .get("dev-dependencies")
        .and_then(|d| d.as_table())
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default();
    let has_package = doc.get("package").is_some();

    (deps, has_package)
}

/// Strip version specifiers from a Python dependency string.
/// "ruff>=0.5.0" → "ruff", "black[jupyter]" → "black"
fn extract_pkg_name(spec: &str) -> String {
    let s = spec.trim();
    s.split(&['>', '<', '=', '~', '!', '[', ' ', ';'][..])
        .next()
        .unwrap_or(s)
        .to_string()
}

// --- Hook injection ---

fn inject_hook(filename: &str, full_path: &Path, entry: &knowledge::ToolEntry) -> Result<()> {
    match filename {
        "pyproject.toml" => inject_toml(full_path, &["tool", "nizm", "hooks"], entry),
        "Cargo.toml" => inject_toml(full_path, &["package", "metadata", "nizm", "hooks"], entry),
        ".nizm.toml" => inject_toml(full_path, &["hooks"], entry),
        "package.json" => inject_json(full_path, entry),
        _ => anyhow::bail!("unsupported manifest: {filename}"),
    }
}

fn inject_toml(file_path: &Path, table_path: &[&str], entry: &knowledge::ToolEntry) -> Result<()> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;

    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse TOML")?;

    // Navigate/create the table path
    let mut table = doc.as_table_mut();
    for &key in table_path {
        if !table.contains_key(key) {
            let mut new_table = toml_edit::Table::new();
            new_table.set_implicit(true);
            table.insert(key, toml_edit::Item::Table(new_table));
        }
        table = table[key]
            .as_table_mut()
            .context("expected table in TOML")?;
    }

    // Build inline table: { cmd = "...", glob = "..." }
    let mut hook = toml_edit::InlineTable::new();
    hook.insert("cmd", entry.cmd.into());
    if let Some(glob) = entry.glob {
        hook.insert("glob", glob.into());
    }

    table.insert(
        entry.name,
        toml_edit::Item::Value(toml_edit::Value::InlineTable(hook)),
    );

    std::fs::write(file_path, doc.to_string())
        .with_context(|| format!("failed to write {}", file_path.display()))?;

    Ok(())
}

/// Inject a hook into a package.json using serde (preserve_order keeps key order).
fn inject_json(file_path: &Path, entry: &knowledge::ToolEntry) -> Result<()> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;

    let mut root: serde_json::Value =
        serde_json::from_str(&content).context("failed to parse JSON")?;

    let indent = detect_json_indent(&content);

    // Ensure nizm.hooks exists
    let nizm = root
        .as_object_mut()
        .context("expected JSON object")?
        .entry("nizm")
        .or_insert_with(|| serde_json::json!({}));
    let hooks = nizm
        .as_object_mut()
        .context("expected nizm to be an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .context("expected hooks to be an object")?;

    // Build hook value
    let mut hook = serde_json::Map::new();
    hook.insert("cmd".into(), entry.cmd.into());
    if let Some(glob) = entry.glob {
        hook.insert("glob".into(), glob.into());
    }
    hooks_obj.insert(entry.name.into(), serde_json::Value::Object(hook));

    let output = serialize_json(&root, &indent)?;

    std::fs::write(file_path, &output)
        .with_context(|| format!("failed to write {}", file_path.display()))?;

    Ok(())
}

// --- Tool config injection ---

/// Returns `true` if config was injected, `false` if skipped (already exists).
fn inject_ruff_config(pyproject_path: &Path) -> Result<bool> {
    let content = std::fs::read_to_string(pyproject_path)
        .with_context(|| format!("failed to read {}", pyproject_path.display()))?;

    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse TOML")?;

    // Skip if any [tool.ruff] config already exists
    if doc.get("tool").and_then(|t| t.get("ruff")).is_some() {
        return Ok(false);
    }

    // Navigate/create tool.ruff.lint
    let mut table = doc.as_table_mut();
    for &key in &["tool", "ruff", "lint"] {
        if !table.contains_key(key) {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            table.insert(key, toml_edit::Item::Table(t));
        }
        table = table[key]
            .as_table_mut()
            .context("expected table in TOML")?;
    }
    let lint = table;

    // select = ["F", "I"]
    let mut select = toml_edit::Array::new();
    select.push("F");
    select.push("I");
    lint.insert("select", toml_edit::value(select));

    // fixable = ["F401", "I"]
    let mut fixable = toml_edit::Array::new();
    fixable.push("F401");
    fixable.push("I");
    lint.insert("fixable", toml_edit::value(fixable));

    std::fs::write(pyproject_path, doc.to_string())
        .with_context(|| format!("failed to write {}", pyproject_path.display()))?;

    Ok(true)
}

const PRETTIERRC_CONTENT: &str = r#"{
    "tabWidth": 4,
    "useTabs": false,
    "singleQuote": false,
    "trailingComma": "es5",
    "endOfLine": "lf",
    "arrowParens": "always",
    "bracketSameLine": true,
    "printWidth": 120
}
"#;

const PRETTIER_CONFIG_FILES: &[&str] = &[
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.yml",
    ".prettierrc.yaml",
    ".prettierrc.toml",
    ".prettierrc.js",
    ".prettierrc.cjs",
    ".prettierrc.mjs",
    "prettier.config.js",
    "prettier.config.cjs",
    "prettier.config.mjs",
];

/// Returns `true` if file was created, `false` if skipped (config already exists).
fn create_prettierrc(dir: &Path) -> Result<bool> {
    if PRETTIER_CONFIG_FILES.iter().any(|f| dir.join(f).exists()) {
        return Ok(false);
    }
    let path = dir.join(".prettierrc");
    std::fs::write(&path, PRETTIERRC_CONTENT)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}
