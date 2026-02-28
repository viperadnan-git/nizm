use anyhow::{Context, Result};
use dialoguer::MultiSelect;
use std::collections::HashSet;
use std::path::Path;

use crate::{config, knowledge, style};

pub fn init(repo_root: &Path) -> Result<()> {
    let manifests = config::discover_manifests(repo_root)?;
    if manifests.is_empty() {
        println!(
            "{}",
            style::yellow("no manifests found — run `nizm install` first")
        );
        return Ok(());
    }

    let mut suggestions: Vec<Suggestion> = Vec::new();

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

        let deps = read_devdeps(filename, &content);

        for dep in &deps {
            if let Some(entry) = knowledge::lookup(dep)
                && !existing.contains(entry.name)
            {
                suggestions.push(Suggestion {
                    manifest: manifest_path.clone(),
                    entry,
                });
            }
        }

        // Rust implicit tools (clippy, rustfmt)
        if filename == "Cargo.toml" && has_cargo_package(&content) {
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
        println!(
            "{}",
            style::yellow("no hooks to suggest — all detected tools are already configured")
        );
        return Ok(());
    }

    let labels: Vec<String> = suggestions
        .iter()
        .map(|s| s.entry.name.to_string())
        .collect();

    let selections = MultiSelect::new()
        .with_prompt("select hooks to add (space = toggle, enter = confirm)")
        .items(&labels)
        .interact()?;

    if selections.is_empty() {
        println!("nothing selected");
        return Ok(());
    }

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
    }

    println!("{}", style::green("done — run `nizm install` to activate"));
    Ok(())
}

struct Suggestion {
    manifest: std::path::PathBuf,
    entry: &'static knowledge::ToolEntry,
}

// --- Dev-dependency readers ---

fn read_devdeps(filename: &str, content: &str) -> Vec<String> {
    match filename {
        "pyproject.toml" => read_pyproject_devdeps(content),
        "package.json" => read_packagejson_devdeps(content),
        "Cargo.toml" => read_cargo_devdeps(content),
        _ => Vec::new(),
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

fn read_cargo_devdeps(content: &str) -> Vec<String> {
    let doc: toml_edit::DocumentMut = match content.parse() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };

    doc.get("dev-dependencies")
        .and_then(|d| d.as_table())
        .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        .unwrap_or_default()
}

fn has_cargo_package(content: &str) -> bool {
    content
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|doc| doc.get("package").map(|_| true))
        .unwrap_or(false)
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

    // Build the hook value fragment
    let hook_val = if let Some(glob) = entry.glob {
        format!(
            "{{\n{}\"cmd\": \"{}\",\n{}\"glob\": \"{}\"\n{}}}",
            i(4),
            entry.cmd,
            i(4),
            glob,
            i(3)
        )
    } else {
        format!("{{ \"cmd\": \"{}\" }}", entry.cmd)
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

/// Detect indentation used in a JSON file (first indented line).
fn detect_json_indent(content: &str) -> String {
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
