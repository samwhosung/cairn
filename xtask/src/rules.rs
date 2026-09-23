//! The repo's own rules: what the standard linters don't check.

use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;

const MAX_PLAIN_RUN: usize = 3;
const MAX_OUTER_DOC_RUN: usize = 8;
const MAX_INNER_DOC_RUN: usize = 12;
const MAX_COMMENT_RATIO: f32 = 0.3;
const MAX_RS_LINES: usize = 800;
const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_SUBJECT: usize = 72;
const MAX_BODY_LINE: usize = 100;

const DENIED_EXTENSIONS: &[&str] = &[
    "mpq", "blp", "m2", "mdx", "skin", "anim", "wmo", "adt", "wdt", "wdl", "dbc", "db2", "bls",
    "wfx", "lit", "trs", "zmp", "wav", "mp3", "ogg", "png", "jpg", "jpeg", "tga", "dds", "exe",
    "dll", "dylib", "so", "zip", "7z", "gz",
];

const MAGIC: &[(&[u8], &str)] = &[
    (b"MPQ\x1a", "an MPQ archive"),
    (b"BLP1", "a BLP texture"),
    (b"BLP2", "a BLP texture"),
    (b"MD20", "an M2 model"),
    (b"REVM", "a chunked WoW file"),
    (b"WDBC", "a DBC table"),
];

static REFS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (
            r"(?i)\b(?:decision|record|adr|dr)\s*[#-]?\s*\d{2,4}\b",
            "a decision-record reference",
        ),
        (
            r"\(\d{4}(?:\s*[,/]\s*\d{4})*\)",
            "a numbered reference in parentheses",
        ),
        (r"\bQ\d{1,3}\b", "a lab question id"),
        (
            r"(?i)\b(?:drydock|wow-5875-re|wow-re)\b",
            "a reference to a private repo",
        ),
    ]
    .into_iter()
    .map(|(p, why)| (Regex::new(p).expect("valid pattern"), why))
    .collect()
});

static CONVENTIONAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(feat|fix|refactor|perf|test|docs|build|ci|chore|style|revert)(\([a-z0-9-]+\))?!?: \S",
    )
    .expect("valid pattern")
});

/// Every rule over every file git tracks or would track.
pub fn check_tree(root: &Path) -> Result<()> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()
        .context("running git ls-files")?;
    let mut problems = Vec::new();
    for file in String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|f| !f.is_empty())
    {
        let Ok(bytes) = std::fs::read(root.join(file)) else {
            continue;
        };
        if assets(file, &bytes, &mut problems) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        if Path::new(file).extension().is_some_and(|e| e == "rs") {
            rust(file, &text, &mut problems);
        } else if !file.starts_with("LICENSE") && file != "Cargo.lock" {
            for (i, line) in text.lines().enumerate() {
                refs(file, i + 1, line, &mut problems);
            }
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    bail!("{}", problems.join("\n"))
}

/// Game data and binaries never enter the tree. True when the file is binary (no text rules apply).
fn assets(file: &str, bytes: &[u8], out: &mut Vec<String>) -> bool {
    let ext = Path::new(file)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if DENIED_EXTENSIONS.contains(&ext.as_str()) {
        out.push(format!("{file}: .{ext} files are not allowed in the repo"));
    }
    for (magic, what) in MAGIC {
        if bytes.starts_with(magic) {
            out.push(format!(
                "{file}: looks like {what}; game data never enters the repo"
            ));
        }
    }
    if bytes.len() > MAX_FILE_BYTES && file != "Cargo.lock" {
        out.push(format!(
            "{file}: {} KiB is over the {} KiB limit",
            bytes.len() / 1024,
            MAX_FILE_BYTES / 1024
        ));
    }
    let binary = bytes.iter().take(8000).any(|&b| b == 0);
    if binary {
        out.push(format!("{file}: binary files are not allowed in the repo"));
    }
    binary
}

fn refs(file: &str, line: usize, text: &str, out: &mut Vec<String>) {
    for (re, why) in REFS.iter() {
        if let Some(m) = re.find(text) {
            out.push(format!(
                "{file}:{line}: `{}` is {why}; say it in the code or drop it",
                m.as_str()
            ));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Plain,
    OuterDoc,
    InnerDoc,
}

/// One line of Rust source: its comment (if any), whether code is on it, and block comments.
#[derive(Default)]
struct Line {
    comment: Option<(Kind, String)>,
    code: bool,
    block: bool,
}

/// Split Rust source into lines of code and comments, stepping over string and char literals.
fn lex(src: &str) -> Vec<Line> {
    let mut lines = vec![Line::default()];
    let chars: Vec<char> = src.chars().collect();
    let (mut i, mut depth) = (0, 0usize);
    let mut in_str: Option<usize> = None;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '\n' {
            lines.push(Line::default());
            i += 1;
            continue;
        }
        let line = lines.last_mut().expect("at least one line");
        if depth > 0 {
            line.block = true;
            if c == '*' && next == Some('/') {
                depth -= 1;
                i += 2;
            } else if c == '/' && next == Some('*') {
                depth += 1;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(hashes) = in_str {
            line.code = true;
            let (step, closed) = string_step(&chars, i, hashes);
            i += step;
            if closed {
                in_str = None;
            }
            continue;
        }
        match (c, next) {
            ('/', Some('/')) => {
                let rest: String = chars[i..].iter().take_while(|&&ch| ch != '\n').collect();
                let kind = if rest.starts_with("///") && !rest.starts_with("////") {
                    Kind::OuterDoc
                } else if rest.starts_with("//!") {
                    Kind::InnerDoc
                } else {
                    Kind::Plain
                };
                i += rest.chars().count();
                line.comment = Some((kind, rest));
            }
            ('/', Some('*')) => {
                depth = 1;
                line.block = true;
                i += 2;
            }
            ('"', _) => {
                in_str = Some(usize::MAX);
                line.code = true;
                i += 1;
            }
            ('r', Some('"' | '#')) if i == 0 || !chars[i - 1].is_alphanumeric() => {
                let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
                if chars.get(i + 1 + hashes) == Some(&'"') {
                    in_str = Some(hashes);
                    i += 2 + hashes;
                } else {
                    i += 1;
                }
                line.code = true;
            }
            ('\'', _) => {
                line.code = true;
                i += char_literal_len(&chars, i);
            }
            _ => {
                if !c.is_whitespace() {
                    line.code = true;
                }
                i += 1;
            }
        }
    }
    lines
}

/// Inside a string literal (`hashes` is `usize::MAX` for a plain one): how far to step, and
/// whether the literal closed.
fn string_step(chars: &[char], i: usize, hashes: usize) -> (usize, bool) {
    match chars[i] {
        '\\' if hashes == usize::MAX => (2, false),
        '"' if hashes == usize::MAX => (1, true),
        '"' if chars[i + 1..]
            .iter()
            .take(hashes)
            .filter(|&&h| h == '#')
            .count()
            == hashes =>
        {
            (1 + hashes, true)
        }
        _ => (1, false),
    }
}

/// Length of a char literal at `i`, or 1 when the quote opens a lifetime.
fn char_literal_len(chars: &[char], i: usize) -> usize {
    match (chars.get(i + 1), chars.get(i + 2)) {
        (Some('\\'), _) => 2 + chars[i + 2..].iter().take_while(|&&ch| ch != '\'').count() + 1,
        (Some(_), Some('\'')) => 3,
        _ => 1,
    }
}

fn rust(file: &str, src: &str, out: &mut Vec<String>) {
    let lines = lex(src);
    let n_lines = src.lines().count();
    if n_lines > MAX_RS_LINES {
        out.push(format!(
            "{file}: {n_lines} lines is over the {MAX_RS_LINES}-line limit; split it"
        ));
    }
    let is_root = file.ends_with("src/lib.rs") || file.ends_with("src/main.rs");
    if is_root {
        let first = src
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default();
        if !first.trim_start().starts_with("//!") {
            out.push(format!(
                "{file}:1: a crate starts with one `//!` line saying what it is for"
            ));
        }
    }
    let (mut code_lines, mut comment_lines) = (0usize, 0usize);
    let mut run: Option<(Kind, usize, usize)> = None;
    let close_run = |run: Option<(Kind, usize, usize)>, out: &mut Vec<String>| {
        if let Some((kind, start, len)) = run {
            let max = match kind {
                Kind::Plain => MAX_PLAIN_RUN,
                Kind::OuterDoc => MAX_OUTER_DOC_RUN,
                Kind::InnerDoc => MAX_INNER_DOC_RUN,
            };
            if len > max {
                out.push(format!("{file}:{start}: {len} comment lines in a row (max {max} for {kind:?}); say less, or let the code say it"));
            }
        }
    };
    for (i, line) in lines.iter().enumerate() {
        if line.block {
            out.push(format!(
                "{file}:{}: block comments are not used here",
                i + 1
            ));
        }
        if let Some((_, text)) = &line.comment {
            refs(file, i + 1, text, out);
        }
        let comment_only = line.comment.is_some() && !line.code;
        if line.code {
            code_lines += 1;
        }
        match (comment_only, line.comment.as_ref().map(|c| c.0), run) {
            (true, Some(kind), Some((k, start, len))) if k == kind => {
                run = Some((k, start, len + 1));
            }
            (true, Some(kind), _) => {
                close_run(run, out);
                run = Some((kind, i + 1, 1));
            }
            _ => {
                close_run(run, out);
                run = None;
            }
        }
        if comment_only {
            comment_lines += 1;
        }
    }
    close_run(run, out);
    if code_lines >= 40 && comment_lines as f32 > MAX_COMMENT_RATIO * code_lines as f32 {
        out.push(format!(
            "{file}: {comment_lines} comment lines to {code_lines} lines of code (max {MAX_COMMENT_RATIO}); let the code carry more"
        ));
    }
}

/// What a commit message must look like: `type(scope): summary`, short, no references.
pub fn commit_message(msg: &str) -> Vec<String> {
    let lines: Vec<&str> = msg.lines().filter(|l| !l.starts_with('#')).collect();
    let subject = lines.first().copied().unwrap_or_default().trim_end();
    let mut out = Vec::new();
    if !CONVENTIONAL.is_match(subject) {
        out.push(format!("`{subject}` is not `type(scope): summary` (feat, fix, refactor, perf, test, docs, build, ci, chore, style, revert)"));
    }
    if subject.chars().count() > MAX_SUBJECT {
        out.push(format!("the subject is over {MAX_SUBJECT} characters"));
    }
    if subject.ends_with('.') {
        out.push("the subject ends with a period".into());
    }
    if lines.get(1).is_some_and(|l| !l.trim().is_empty()) {
        out.push("the second line must be blank".into());
    }
    for (i, line) in lines.iter().enumerate() {
        let trailer = line.contains(": ")
            && line
                .split(": ")
                .next()
                .is_some_and(|k| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        if i > 1 && !trailer && !line.contains("://") && line.chars().count() > MAX_BODY_LINE {
            out.push(format!("line {} is over {MAX_BODY_LINE} characters", i + 1));
        }
        let mut found = Vec::new();
        refs("message", i + 1, line, &mut found);
        out.extend(found);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust_problems(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        rust("a.rs", src, &mut out);
        out
    }

    #[test]
    fn references_in_comments_fail_and_in_strings_pass() {
        assert_eq!(rust_problems("let a = 1; // see decision 0421\n").len(), 1);
        assert_eq!(
            rust_problems("let a = \"decision 0421 (0012)\";\n").len(),
            0
        );
        assert_eq!(
            rust_problems("let a = r#\"// Q12\"#; let b = 2;\n").len(),
            0
        );
        assert_eq!(rust_problems("let c = '\"'; // fine\n").len(), 0);
    }

    #[test]
    fn long_comment_runs_fail() {
        let three = "// one\n// two\n// three\nfn f() {}\n";
        let four = "// one\n// two\n// three\n// four\nfn f() {}\n";
        assert!(rust_problems(three).is_empty());
        assert_eq!(rust_problems(four).len(), 1);
        assert_eq!(rust_problems("/* no */ fn f() {}\n").len(), 1);
    }

    #[test]
    fn crate_roots_start_with_their_purpose() {
        let mut out = Vec::new();
        rust("x/src/lib.rs", "use a::b;\n", &mut out);
        assert_eq!(out.len(), 1);
        out.clear();
        rust("x/src/lib.rs", "//! Reads things.\nuse a::b;\n", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn game_files_and_binaries_fail() {
        let mut out = Vec::new();
        assets("a/Elwynn_32_48.adt", b"REVM\x04\0\0\0", &mut out);
        assert_eq!(out.len(), 3);
        out.clear();
        assets("notes.txt", b"MPQ\x1a renamed", &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn commit_messages() {
        assert!(
            commit_message(
                "feat(mpq): read the patch chain\n\nBody.\n\nCo-Authored-By: A <a@b.c>\n"
            )
            .is_empty()
        );
        assert!(!commit_message("Added stuff").is_empty());
        assert!(!commit_message("fix: the thing (0421)").is_empty());
        assert!(!commit_message("fix: the thing\nno blank line").is_empty());
    }
}
