//! The repo's gates. `cargo xtask check` runs every one; `--fast` runs the quick ones.

mod rules;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: cargo xtask <command>
  check [--fast]      every gate; --fast skips clippy, tests, deny and machete
  commit-msg <file>   lint one commit message (the commit-msg hook)
  commits <range>     lint every commit message in a git range
  map [--check]       regenerate the crate list in README.md
  pictures <dir>      run the tests that draw the app on the GPU, saving their shots in <dir>
  setup               use the repo's git hooks";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let result = match args.first().map(String::as_str) {
        Some("check") => check(&root, flag("--fast")),
        Some("commit-msg") => args
            .get(1)
            .context("commit-msg needs the message file")
            .and_then(|f| commit_msg(Path::new(f))),
        Some("commits") => commits(&root, args.get(1).map_or("HEAD", String::as_str)),
        Some("map") => map(&root, flag("--check")),
        Some("pictures") => args
            .get(1)
            .context("pictures needs a directory for the shots")
            .and_then(|d| pictures(&root, Path::new(d))),
        Some("setup") => run(&root, "git", &["config", "core.hooksPath", "hooks"]),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn check(root: &Path, fast: bool) -> Result<()> {
    let mut failed: Vec<&str> = Vec::new();
    let mut step = |name: &'static str, f: &dyn Fn() -> Result<()>| {
        let t = Instant::now();
        match f() {
            Ok(()) => println!("ok    {name} ({:.1}s)", t.elapsed().as_secs_f32()),
            Err(e) => {
                println!("FAIL  {name} ({:.1}s)\n{e:#}", t.elapsed().as_secs_f32());
                failed.push(name);
            }
        }
    };
    step("rules", &|| rules::check_tree(root));
    step("map", &|| map(root, true));
    step("fmt", &|| {
        run(root, "cargo", &["fmt", "--all", "--", "--check"])
    });
    step("toml", &|| run(root, "taplo", &["fmt", "--check"]));
    step("typos", &|| run(root, "typos", &[]));
    if !fast {
        step("clippy", &|| {
            run(
                root,
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ],
            )
        });
        step("test", &|| {
            run(
                root,
                "cargo-nextest",
                &["nextest", "run", "--workspace", "--all-targets", "--locked"],
            )
        });
        step("deny", &|| run(root, "cargo-deny", &["check"]));
        step("machete", &|| run(root, "cargo-machete", &[]));
        if std::env::var_os("WOW_DATA").is_none() {
            println!("note  WOW_DATA is not set, so the tests on the install were skipped");
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        bail!("failed: {}", failed.join(", "))
    }
}

/// Run a tool from the repo root; on failure, show everything it printed.
fn run(root: &Path, tool: &str, args: &[&str]) -> Result<()> {
    let out = match Command::new(tool).args(args).current_dir(root).output() {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("{tool} is not installed: {}", install_hint(tool))
        }
        Err(e) => return Err(e).with_context(|| format!("running {tool}")),
    };
    if out.status.success() {
        return Ok(());
    }
    bail!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn install_hint(tool: &str) -> &'static str {
    match tool {
        "taplo" => "brew install taplo, or cargo install taplo-cli --locked",
        "typos" => "brew install typos-cli, or cargo install typos-cli --locked",
        "cargo-deny" => "brew install cargo-deny, or cargo install cargo-deny --locked",
        "cargo-machete" => "cargo install cargo-machete --locked",
        "cargo-nextest" => "brew install cargo-nextest, or cargo install cargo-nextest --locked",
        _ => "see the tool's documentation",
    }
}

fn commit_msg(file: &Path) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let problems = rules::commit_message(&text);
    if problems.is_empty() {
        return Ok(());
    }
    bail!("commit message:\n  {}", problems.join("\n  "))
}

fn commits(root: &Path, range: &str) -> Result<()> {
    let range = if range.starts_with("0000000") || range.is_empty() {
        "HEAD"
    } else {
        range
    };
    let out = Command::new("git")
        .args(["log", "--format=%H%x00%B%x1e", range])
        .current_dir(root)
        .output()
        .context("running git log")?;
    if !out.status.success() {
        bail!("git log {range}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let log = String::from_utf8_lossy(&out.stdout);
    let mut bad = Vec::new();
    for entry in log.split('\x1e').map(str::trim).filter(|e| !e.is_empty()) {
        let (hash, msg) = entry.split_once('\0').unwrap_or((entry, ""));
        let problems = rules::commit_message(msg);
        if !problems.is_empty() {
            bad.push(format!(
                "{}: {}",
                &hash[..hash.len().min(10)],
                problems.join("; ")
            ));
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    bail!("{}", bad.join("\n"))
}

const PICTURE_TESTS: [&str; 4] = [
    "scenarios::pictures::",
    "scenarios::together::two_players",
    "scenarios::fight::",
    "scenarios::notes::",
];

fn pictures(root: &Path, dir: &Path) -> Result<()> {
    if std::env::var_os("WOW_DATA").is_none() {
        bail!("pictures needs WOW_DATA, the install's Data directory");
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let dir = std::path::absolute(dir).context("resolving the shots' directory")?;
    let out = Command::new("cargo")
        .args(["test", "-p", "cairn", "--locked", "--", "--ignored"])
        .args(PICTURE_TESTS)
        .args(["--skip", "the_frame_cost"])
        .env("CAIRN_PICTURES", &dir)
        .current_dir(root)
        .output()
        .context("running cargo test")?;
    let text = String::from_utf8_lossy(&out.stdout);
    print!("{text}");
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr));
    }
    for filter in PICTURE_TESTS {
        if !text
            .lines()
            .any(|l| l.contains(filter) && l.ends_with(" ok"))
        {
            bail!("no test matched {filter}");
        }
    }
    Ok(())
}

/// The crate list in README.md, generated from each member's `description`.
fn map(root: &Path, check_only: bool) -> Result<()> {
    let out = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .context("running cargo metadata")?;
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).context("cargo metadata")?;
    let mut rows = Vec::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        let name = p["name"].as_str().unwrap_or_default();
        let Some(desc) = p["description"].as_str() else {
            bail!("crate {name} has no description in its Cargo.toml");
        };
        let dir = Path::new(p["manifest_path"].as_str().unwrap_or_default())
            .parent()
            .and_then(|d| d.strip_prefix(root).ok())
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        rows.push(format!("- [`{name}`]({dir}) — {desc}"));
    }
    rows.sort();
    let readme_path = root.join("README.md");
    let readme = std::fs::read_to_string(&readme_path).context("reading README.md")?;
    let (start, end) = ("<!-- crates:start -->", "<!-- crates:end -->");
    let (Some(a), Some(b)) = (readme.find(start), readme.find(end)) else {
        bail!("README.md lacks the {start} … {end} markers");
    };
    let fresh = format!(
        "{}{start}\n{}\n{}",
        &readme[..a],
        rows.join("\n"),
        &readme[b..]
    );
    if fresh == readme {
        return Ok(());
    }
    if check_only {
        bail!("the crate list in README.md is stale: run `cargo xtask map`");
    }
    std::fs::write(&readme_path, fresh).context("writing README.md")
}
