//! Parse and apply gitignore-style exclusion patterns to root-relative paths.
//!
//! Patterns follow the gitignore conventions without negation:
//!
//! - A pattern without a slash, such as `.git` or `*.tmp`, matches a file or
//!   directory name at any depth.
//! - A pattern with a slash at the beginning or in the middle, such as
//!   `/build` or `data/raw`, matches the path from the scan root.
//! - A trailing slash, as in `cache/`, matches directories only.
//! - `*` and `?` do not cross a slash; `[...]` is a character class, which
//!   may hold POSIX classes such as `[:digit:]`. `**/` matches any number of
//!   leading directories, `/**` everything inside a directory, and `/**/`
//!   zero or more directories.
//! - A backslash makes the next character literal, and matching is
//!   case-sensitive.
//!
//! An excluded directory is not entered, so nothing below it is counted.

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::error::{Result, usage};
use crate::units::hex;

struct Rule {
    expression: String,
    anchored: bool,
    directory_only: bool,
}

/// Compiled exclusion patterns shared by every scanning feature.
pub struct PathMatcher {
    patterns: Vec<String>,
    entry_name: Option<Regex>,
    entry_path: Option<Regex>,
    directory_name: Option<Regex>,
    directory_path: Option<Regex>,
}

impl PathMatcher {
    /// Compile patterns; blank entries are ignored.
    pub fn new(patterns: &[String]) -> Result<Self> {
        for pattern in patterns {
            let trimmed = pattern.trim();
            if !trimmed.is_empty() && trimmed != pattern {
                // The configuration file could not keep the spaces.
                return Err(usage(format!(
                    "exclusion pattern {pattern:?} has spaces around it; remove them, or \
                     write [ ] for a space that is part of a name"
                )));
            }
        }
        let patterns: Vec<String> = patterns
            .iter()
            .map(|pattern| pattern.trim().to_string())
            .filter(|pattern| !pattern.is_empty())
            .collect();
        let rules = patterns
            .iter()
            .map(|pattern| compile(pattern))
            .collect::<Result<Vec<_>>>()?;
        let combine = |directory_only: bool, anchored: bool| -> Result<Option<Regex>> {
            let parts: Vec<String> = rules
                .iter()
                .filter(|rule| rule.directory_only == directory_only && rule.anchored == anchored)
                .map(|rule| format!("(?:{})", rule.expression))
                .collect();
            if parts.is_empty() {
                return Ok(None);
            }
            // Thousands of rules exceed the default 10 MB compiled size.
            regex::RegexBuilder::new(&format!("^(?:{})$", parts.join("|")))
                .size_limit(1 << 30)
                .build()
                .map(Some)
                .map_err(|error| usage(format!("invalid exclusion pattern: {error}")))
        };
        Ok(PathMatcher {
            entry_name: combine(false, false)?,
            entry_path: combine(false, true)?,
            directory_name: combine(true, false)?,
            directory_path: combine(true, true)?,
            patterns,
        })
    }

    /// Patterns as given, without surrounding whitespace or blank entries.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Whether any pattern exists at all.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether a rule needs the whole root-relative path, not only the name.
    pub fn needs_path(&self) -> bool {
        self.entry_path.is_some() || self.directory_path.is_some()
    }

    /// Whether some pattern can only be decided once an entry is a directory.
    pub fn has_directory_rules(&self) -> bool {
        self.directory_name.is_some() || self.directory_path.is_some()
    }

    /// Stable SHA-256 digest used to compare snapshot scan policies.
    pub fn digest(&self) -> String {
        let payload = if self.patterns.is_empty() {
            "[]".to_string()
        } else {
            // Matches Python's json.dumps({"syntax": ..., "patterns": [...]})
            // with compact separators, so digests stay comparable.
            let patterns = serde_json::to_string(&self.patterns).unwrap_or_default();
            format!("{{\"syntax\":\"gitignore\",\"patterns\":{patterns}}}")
        };
        hex(&Sha256::digest(payload.as_bytes()))
    }

    /// Whether a walker should skip an entry of any type.
    pub fn matches_entry(&self, name: &str, relative_path: &str) -> bool {
        self.entry_name
            .as_ref()
            .is_some_and(|regex| regex.is_match(name))
            || self
                .entry_path
                .as_ref()
                .is_some_and(|regex| regex.is_match(relative_path))
    }

    /// Whether a directory entry matches a directory-only pattern.
    pub fn matches_directory(&self, name: &str, relative_path: &str) -> bool {
        self.directory_name
            .as_ref()
            .is_some_and(|regex| regex.is_match(name))
            || self
                .directory_path
                .as_ref()
                .is_some_and(|regex| regex.is_match(relative_path))
    }

    /// Whether a path, or any directory above it, is excluded.
    pub fn matches(&self, relative_path: &str, is_directory: bool) -> bool {
        let parts: Vec<&str> = relative_path
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .collect();
        for (index, name) in parts.iter().enumerate() {
            let prefix = parts[..=index].join("/");
            let last = index == parts.len() - 1;
            if self.matches_entry(name, &prefix)
                || ((is_directory || !last) && self.matches_directory(name, &prefix))
            {
                return true;
            }
        }
        false
    }
}

fn compile(pattern: &str) -> Result<Rule> {
    if pattern.starts_with(['#', ';']) {
        return Err(usage(format!(
            "exclusion pattern {pattern} would read as a comment in the configuration \
             file; write \\{pattern} for a name that starts with '{}'",
            &pattern[..1]
        )));
    }
    if pattern.starts_with('!') {
        return Err(usage(format!(
            "negated exclusion patterns are not supported: {pattern} \
             (write \\! for a name that starts with '!')"
        )));
    }
    let directory_only = pattern.ends_with('/') && !pattern.ends_with("\\/");
    let mut body = if directory_only {
        pattern.trim_end_matches('/').to_string()
    } else {
        pattern.to_string()
    };
    if let Some(rest) = body.strip_prefix("./") {
        body = format!("/{rest}");
    }
    let anchored = body.contains('/');
    let body = body.trim_start_matches('/');
    if body.is_empty() || body == "." {
        return Err(usage(format!(
            "exclusion pattern matches nothing: {pattern}"
        )));
    }
    Ok(Rule {
        expression: translate(body),
        anchored,
        directory_only,
    })
}

/// Translate a slash-separated glob into a regular expression body.
fn translate(glob: &str) -> String {
    let characters: Vec<char> = glob.chars().collect();
    let length = characters.len();
    let mut output = String::new();
    let mut index = 0;
    while index < length {
        let character = characters[index];
        if character == '*' {
            let mut end = index;
            while end < length && characters[end] == '*' {
                end += 1;
            }
            let whole_segment = (index == 0 || characters[index - 1] == '/')
                && (end == length || characters[end] == '/');
            if end - index >= 2 && whole_segment {
                if end == length {
                    // "dir/**": everything inside dir.
                    output.push_str(".*");
                } else {
                    // "**/": zero or more directories.
                    output.push_str("(?:.*/)?");
                    end += 1;
                }
            } else {
                output.push_str("[^/]*");
            }
            index = end;
        } else if character == '?' {
            output.push_str("[^/]");
            index += 1;
        } else if character == '[' {
            match class_end(&characters, index) {
                None => {
                    output.push_str(&regex::escape("["));
                    index += 1;
                }
                Some(close) => {
                    let body = &characters[index + 1..close];
                    let negated = matches!(body.first(), Some('!' | '^'));
                    let members = class_members(&body[usize::from(negated)..]);
                    // A class never matches the path separator.
                    if negated {
                        output.push_str(&format!("[^{members}/]"));
                    } else {
                        output.push_str(&format!("[[{members}]&&[^/]]"));
                    }
                    index = close + 1;
                }
            }
        } else if character == '\\' && index + 1 < length {
            output.push_str(&regex::escape(&characters[index + 1].to_string()));
            index += 2;
        } else {
            output.push_str(&regex::escape(&character.to_string()));
            index += 1;
        }
    }
    output
}

/// The members of a bracket expression as members of a regex class.
///
/// A `[:name:]` POSIX class is kept, a hyphen between two members keeps
/// its range meaning, and a hyphen at either end is literal. Everything
/// else that is special inside a regex class is escaped, including the set
/// operators `&&` and `~~`.
fn class_members(body: &[char]) -> String {
    let mut output = String::new();
    let mut index = 0;
    while index < body.len() {
        if let Some(end) = posix_class_end(body, index) {
            output.extend(&body[index..=end]);
            index = end + 1;
            continue;
        }
        let member = body[index];
        let at_either_end = index == 0 || index == body.len() - 1;
        if matches!(member, '\\' | '[' | ']' | '^' | '&' | '~') || (member == '-' && at_either_end)
        {
            output.push('\\');
        }
        output.push(member);
        index += 1;
    }
    output
}

/// The index of the `]` that ends a `[:name:]` class starting at `start`.
fn posix_class_end(characters: &[char], start: usize) -> Option<usize> {
    const NAMES: [&str; 14] = [
        "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print", "punct",
        "space", "upper", "word", "xdigit",
    ];
    if characters.get(start) != Some(&'[') || characters.get(start + 1) != Some(&':') {
        return None;
    }
    let name_start = start + 2;
    let colon = (name_start..characters.len()).find(|&index| characters[index] == ':')?;
    let name: String = characters[name_start..colon].iter().collect();
    (characters.get(colon + 1) == Some(&']') && NAMES.contains(&name.as_str())).then_some(colon + 1)
}

fn class_end(characters: &[char], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if matches!(characters.get(index), Some('!' | '^')) {
        index += 1;
    }
    if characters.get(index) == Some(&']') {
        index += 1;
    }
    while index < characters.len() && characters[index] != ']' {
        // The `]` of a `[:name:]` class does not end the expression.
        index = posix_class_end(characters, index).unwrap_or(index) + 1;
    }
    (index < characters.len()).then_some(index)
}

/// A `--match` filter: a glob when it has `*?[`, otherwise case-insensitive
/// text.
pub enum PathPattern {
    Glob(Regex),
    Text(String),
}

impl PathPattern {
    pub fn new(pattern: &str) -> Result<Self> {
        if pattern.contains(['*', '?', '[']) {
            Regex::new(&fnmatch_translate(pattern))
                .map(PathPattern::Glob)
                .map_err(|error| usage(format!("invalid --match pattern: {error}")))
        } else {
            Ok(PathPattern::Text(pattern.to_lowercase()))
        }
    }

    pub fn matches(&self, path: &str) -> bool {
        match self {
            PathPattern::Glob(regex) => regex.is_match(path),
            PathPattern::Text(text) => path.to_lowercase().contains(text.as_str()),
        }
    }
}

/// Translate a shell glob the way Python's `fnmatch` does: unlike in an
/// exclusion pattern, `*`, `?`, and a class also match `/`.
fn fnmatch_translate(pattern: &str) -> String {
    let characters: Vec<char> = pattern.chars().collect();
    let mut output = String::from("(?s)^");
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        match character {
            '*' => {
                while characters.get(index + 1) == Some(&'*') {
                    index += 1;
                }
                output.push_str(".*");
            }
            '?' => output.push('.'),
            '[' => match class_end(&characters, index) {
                None => output.push_str("\\["),
                Some(close) => {
                    let body = &characters[index + 1..close];
                    let negated = matches!(body.first(), Some('!' | '^'));
                    let members = class_members(&body[usize::from(negated)..]);
                    output.push_str(&format!("[{}{members}]", if negated { "^" } else { "" }));
                    index = close;
                }
            },
            other => output.push_str(&regex::escape(&other.to_string())),
        }
        index += 1;
    }
    output.push('$');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_patterns_follow_fnmatch() {
        let glob = PathPattern::new("models/*").unwrap();
        assert!(glob.matches("models/a/b"));
        assert!(!glob.matches("x/models/a"));
        let text = PathPattern::new("Cache").unwrap();
        assert!(text.matches("a/.cache/x"));
        let class = PathPattern::new("[!a]x").unwrap();
        assert!(class.matches("bx"));
        assert!(!class.matches("ax"));
        // Classes read like those of exclusion patterns.
        assert!(PathPattern::new("*[[:digit:]]").unwrap().matches("run7"));
        assert!(PathPattern::new("[!a-]x").unwrap().matches("bx"));
    }

    // (pattern, path, the path is a directory, excluded)
    const CONTRACT: &[(&str, &str, bool, bool)] = &[
        ("cache", "cache", true, true),
        ("cache", "a/cache/x", false, true),
        ("cache", "cached", false, false),
        ("*.tmp", "w/x.tmp", false, true),
        ("*.tmp", "a.tmp.gz", false, false),
        ("/build", "build", true, true),
        ("/build", "src/build", true, false),
        ("./build", "build", true, true),
        ("data/raw", "data/raw/x", false, true),
        ("data/raw", "p/data/raw", true, false),
        ("**/models/*.bin", "models/a.bin", false, true),
        ("**/models/*.bin", "run/models/a.bin", false, true),
        ("**/models/*.bin", "models/sub/a.bin", false, false),
        ("cache/**", "cache/a/b", false, true),
        ("cache/**", "cache", true, false),
        ("a/**/c", "a/c", true, true),
        ("a/**/c", "a/b/x/c", true, true),
        ("a/**/c", "x/a/b/c", false, false),
        ("logs/", "logs", true, true),
        ("logs/", "logs", false, false),
        ("logs/", "a/logs/x", false, true),
        ("[ab]?", "ab", false, true),
        ("[!a]x", "bx", false, true),
        ("[!a]x", "ax", false, false),
        ("a[/]b", "a/b", false, false),
        ("\\*.txt", "*.txt", false, true),
        ("\\*.txt", "a.txt", false, false),
        ("Cache", "cache", true, false),
        ("[a-]x", "-x", false, true),
        ("[!a-]x", "-x", false, false),
        ("[!a-]x", "bx", false, true),
        ("[-a]x", "-x", false, true),
        ("[[:digit:]]*.log", "1.log", false, true),
        ("[[:digit:]]*.log", "a.log", false, false),
        ("[![:digit:]]x", "ax", false, true),
        ("[![:digit:]]x", "1x", false, false),
        ("[[:alpha:]_]x", "_x", false, true),
    ];

    #[test]
    fn patterns_follow_the_documented_contract() {
        for (pattern, path, is_directory, excluded) in CONTRACT {
            let matcher = PathMatcher::new(&[pattern.to_string()]).unwrap();
            assert_eq!(
                matcher.matches(path, *is_directory),
                *excluded,
                "{pattern} vs {path}"
            );
        }
    }

    #[test]
    fn negation_and_empty_patterns_are_rejected() {
        for pattern in ["!keep", "/", "./", ".", "#tmp", ";tmp", " lead", "trail "] {
            assert!(
                PathMatcher::new(&[pattern.to_string()]).is_err(),
                "{pattern}"
            );
        }
        assert!(PathMatcher::new(&["\\!keep".to_string()]).is_ok());
        assert!(PathMatcher::new(&["\\#tmp".to_string()]).is_ok());
        assert!(PathMatcher::new(&["a[ ]b".to_string()]).is_ok());
    }

    #[test]
    fn digests_match_the_python_implementation() {
        // sha256 of '[]' and of the compact JSON payload, as Python computed.
        let empty = PathMatcher::new(&[]).unwrap();
        assert_eq!(
            empty.digest(),
            "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
        );
    }
}
