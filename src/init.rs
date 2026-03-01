use anyhow::{Context, Result};
use dialoguer::MultiSelect;
use std::collections::HashSet;
use std::path::Path;

use crate::{config, config::detect_json_indent, installer, knowledge, style};

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

    // Deduplicate by (manifest, name)
    let mut seen = HashSet::new();
    suggestions.retain(|s| seen.insert((s.manifest.clone(), s.entry.name)));

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
        let labels: Vec<String> = suggestions
            .iter()
            .map(|s| s.entry.name.to_string())
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

/// Format-preserving JSON injection. Edits the raw string instead of
/// re-serializing, so existing indentation and key order are untouched.
fn inject_json(file_path: &Path, entry: &knowledge::ToolEntry) -> Result<()> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))?;

    let root: serde_json::Value = serde_json::from_str(&content).context("failed to parse JSON")?;

    let indent = detect_json_indent(&content);
    let i = |n: usize| indent.repeat(n);

    // Build the hook value fragment (JSON-escape values to handle quotes/backslashes)
    let cmd_json = serde_json::to_string(entry.cmd)?;
    let hook_val = if let Some(glob) = entry.glob {
        let glob_json = serde_json::to_string(glob)?;
        format!(
            "{{\n{}\"cmd\": {},\n{}\"glob\": {}\n{}}}",
            i(4),
            cmd_json,
            i(4),
            glob_json,
            i(3)
        )
    } else {
        format!("{{ \"cmd\": {} }}", cmd_json)
    };

    let has_nizm = root.get("nizm").is_some();
    let has_hooks = root
        .get("nizm")
        .and_then(|n| n.get("hooks"))
        .and_then(|h| h.as_object())
        .is_some();
    let hooks_non_empty = root
        .get("nizm")
        .and_then(|n| n.get("hooks"))
        .and_then(|h| h.as_object())
        .is_some_and(|m| !m.is_empty());

    let result = if has_hooks {
        let close = json_find_object_close(&content, &["nizm", "hooks"])?;
        let comma = if hooks_non_empty { "," } else { "" };
        format!(
            "{}{}\n{}\"{}\": {}\n{}{}",
            content[..close].trim_end(),
            comma,
            i(3),
            entry.name,
            hook_val,
            i(2),
            &content[close..]
        )
    } else if has_nizm {
        let close = json_find_object_close(&content, &["nizm"])?;
        let nizm_non_empty = root["nizm"].as_object().is_some_and(|m| !m.is_empty());
        let comma = if nizm_non_empty { "," } else { "" };
        format!(
            "{}{}\n{}\"hooks\": {{\n{}\"{}\": {}\n{}}}\n{}{}",
            content[..close].trim_end(),
            comma,
            i(2),
            i(3),
            entry.name,
            hook_val,
            i(2),
            i(1),
            &content[close..]
        )
    } else {
        let close = json_find_object_close(&content, &[])?;
        let root_non_empty = root.as_object().is_some_and(|m| !m.is_empty());
        let comma = if root_non_empty { "," } else { "" };
        format!(
            "{}{}\n{}\"nizm\": {{\n{}\"hooks\": {{\n{}\"{}\": {}\n{}}}\n{}}}\n{}",
            content[..close].trim_end(),
            comma,
            i(1),
            i(2),
            i(3),
            entry.name,
            hook_val,
            i(2),
            i(1),
            &content[close..]
        )
    };

    std::fs::write(file_path, result)
        .with_context(|| format!("failed to write {}", file_path.display()))?;

    Ok(())
}

/// Find the byte position of the closing `}` for a JSON object at the given key path.
/// Empty path = root object.
fn json_find_object_close(content: &str, path: &[&str]) -> Result<usize> {
    let open = if path.is_empty() {
        content.find('{').context("no root JSON object")?
    } else {
        let mut pos = 0;
        for &key in path {
            pos = json_find_key_open_brace(content, pos, key)?;
        }
        pos
    };

    json_find_matching_brace(content, open)
}

/// Find the `{` that is the object value of `"key":` starting search from `start`.
fn json_find_key_open_brace(content: &str, start: usize, key: &str) -> Result<usize> {
    let needle = format!("\"{key}\"");
    let mut pos = start;

    loop {
        let key_pos = content[pos..]
            .find(&needle)
            .map(|p| p + pos)
            .with_context(|| format!("key \"{key}\" not found in JSON"))?;

        let after = key_pos + needle.len();
        // Look for : then {
        let mut found_colon = false;
        for (i, ch) in content[after..].char_indices() {
            if ch.is_whitespace() {
                continue;
            }
            if !found_colon {
                if ch == ':' {
                    found_colon = true;
                    continue;
                }
                break;
            }
            // After colon, expect {
            if ch == '{' {
                return Ok(after + i);
            }
            break;
        }
        pos = after;
    }
}

/// Find the matching `}` for `{` at position `open`, handling strings and nesting.
fn json_find_matching_brace(content: &str, open: usize) -> Result<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, ch) in content[open..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(open + i);
                }
            }
            _ => {}
        }
    }

    anyhow::bail!("unmatched brace in JSON")
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
