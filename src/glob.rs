use anyhow::{Context, Result};
use glob::{MatchOptions, Pattern};

const MATCH_OPTS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// Pre-compiled glob matcher with include/exclude patterns.
///
/// Each input pattern entry supports:
/// - `*`, `?`, `**`, `[abc]`, `[!abc]`, `[a-z]` — via the `glob` crate
/// - `{a,b,c}` — brace alternation, expanded at build time (nested allowed)
/// - Patterns without `/` auto-match at any depth (e.g. `*.rs` ≡ `**/*.rs`)
/// - `!pattern` prefix marks an exclude
///
/// Negation semantics: any matching exclude removes the file regardless of
/// include order. If includes are present, a file must match at least one.
#[derive(Debug)]
pub struct Matcher {
    includes: Vec<Pattern>,
    excludes: Vec<Pattern>,
}

impl Matcher {
    /// Build a matcher from a list of patterns. Entries prefixed with `!` are
    /// treated as excludes; others as includes. Each entry may use `{a,b}`
    /// brace alternation, which is pre-expanded.
    pub fn new(patterns: &[String]) -> Result<Self> {
        let mut includes = Vec::new();
        let mut excludes = Vec::new();
        for raw in patterns {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let (target, pat) = if let Some(rest) = trimmed.strip_prefix('!') {
                (&mut excludes, rest.trim())
            } else {
                (&mut includes, trimmed)
            };
            for expanded in expand_braces(pat) {
                let normalized = normalize(&expanded);
                let compiled = Pattern::new(&normalized)
                    .with_context(|| format!("invalid glob pattern: {normalized}"))?;
                target.push(compiled);
            }
        }
        Ok(Self { includes, excludes })
    }

    pub fn is_match(&self, path: &str) -> bool {
        let m = |p: &Pattern| p.matches_with(path, MATCH_OPTS);
        if !self.includes.is_empty() && !self.includes.iter().any(m) {
            return false;
        }
        !self.excludes.iter().any(m)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.includes.is_empty() && self.excludes.is_empty()
    }
}

/// Patterns without `/` match at any depth.
fn normalize(pattern: &str) -> String {
    if pattern.contains('/') {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    }
}

/// Expand `{a,b,c}` alternations into a list of patterns. Supports nesting.
fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(close) = find_matching_close(pattern, open) else {
        return vec![pattern.to_string()];
    };
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    let alts = split_top_level(&pattern[open + 1..close]);
    let mut out = Vec::new();
    for alt in alts {
        let combined = format!("{prefix}{alt}{suffix}");
        out.extend(expand_braces(&combined));
    }
    out
}

fn find_matching_close(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(patterns: &[&str]) -> Matcher {
        let v: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        Matcher::new(&v).unwrap()
    }

    #[test]
    fn bare_pattern_matches_any_depth() {
        assert!(m(&["*.rs"]).is_match("foo.rs"));
        assert!(m(&["*.rs"]).is_match("src/foo.rs"));
        assert!(m(&["*.rs"]).is_match("a/b/c/foo.rs"));
    }

    #[test]
    fn anchored_pattern_respects_depth() {
        assert!(m(&["src/*.rs"]).is_match("src/foo.rs"));
        assert!(!m(&["src/*.rs"]).is_match("src/a/foo.rs"));
        assert!(m(&["src/**/*.rs"]).is_match("src/a/foo.rs"));
    }

    #[test]
    fn brace_alternation() {
        let mc = m(&["*.{rs,toml}"]);
        assert!(mc.is_match("foo.rs"));
        assert!(mc.is_match("foo.toml"));
        assert!(!mc.is_match("foo.md"));
    }

    #[test]
    fn nested_braces() {
        let mc = m(&["{src,tests}/*.{rs,toml}"]);
        assert!(mc.is_match("src/foo.rs"));
        assert!(mc.is_match("tests/foo.toml"));
        assert!(!mc.is_match("docs/foo.rs"));
    }

    #[test]
    fn char_classes() {
        assert!(m(&["[abc].rs"]).is_match("a.rs"));
        assert!(!m(&["[abc].rs"]).is_match("d.rs"));
        assert!(m(&["[!abc].rs"]).is_match("d.rs"));
        assert!(m(&["[a-z].rs"]).is_match("x.rs"));
    }

    #[test]
    fn multiple_patterns_as_list() {
        let mc = m(&["*.py", "!**/migrations/**"]);
        assert!(mc.is_match("src/foo.py"));
        assert!(!mc.is_match("src/migrations/0001.py"));
    }

    #[test]
    fn exclude_only_means_match_unless_excluded() {
        let mc = m(&["!**/vendor/**"]);
        assert!(mc.is_match("src/foo.rs"));
        assert!(!mc.is_match("src/vendor/x.rs"));
    }

    #[test]
    fn empty_matcher_matches_everything() {
        let mc = Matcher::new(&[]).unwrap();
        assert!(mc.is_empty());
        assert!(mc.is_match("anything"));
    }

    #[test]
    fn comma_in_string_is_literal() {
        let mc = m(&["file,with,comma.txt"]);
        assert!(mc.is_match("file,with,comma.txt"));
        assert!(!mc.is_match("file.txt"));
    }
}
