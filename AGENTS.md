# Working in cairn

cairn is an engine for World of Warcraft–style worlds, built for AI agents to work in: a client,
an editor agents drive, and a server. It reads WoW 1.12.1 game data from the player's own
install. No game data is ever stored in this repo.

## The loop

- `cargo xtask setup` once per clone, then `cargo xtask check` before every commit. It runs
  every gate; `--fast` runs the quick ones.
- Small commits on a branch, with conventional messages: `feat(mpq): read the patch chain`. A branch
  lands on `main` by fast-forward once `cargo xtask check` passes on it.
- Prove it works: run the real thing and read the real output. "It compiles" is not done.

## What the gates enforce

You don't need to remember these; `cargo xtask check` tells you. They are here so you know why
a check fails.

- rustfmt; clippy with pedantic lints, warnings as errors; tests.
- Dependency licenses and advisories (cargo-deny), unused dependencies (cargo-machete),
  spelling (typos), TOML formatting (taplo).
- No game data and no binary files in the tree.
- Comments say only what the code can't, briefly. No block comments. No references to
  documents, decisions, tickets or other repos: the code has to stand on its own.
- Every crate root opens with one `//!` line saying what the crate is for, and every crate
  has a `description`; the crate list in README.md is generated from them.
- Rust files stay under 800 lines.
- Commit messages: `type(scope): summary`, subject at most 72 characters, no references.

## What the gates can't judge

- Fix the cause, not the symptom. A workaround is a question to answer, not an answer.
- Delete before adding. The smallest change that solves the problem wins.
- If you catch yourself writing the same instruction twice, make it a rule in `xtask` instead.
- Before landing a change with comments in it, have a fresh agent review them
  (`.claude/agents/comment-reviewer.md`). The author defends its comments; a stranger doesn't.
