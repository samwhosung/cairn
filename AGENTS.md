# Working in cairn

cairn is an engine for World of Warcraft–style worlds, built for AI agents to work in: a client,
an editor agents drive, and a server. It reads WoW 1.12.1's files from the player's own install
at run time.

## What cairn is

- cairn shows the world as WoW shows it: every model, animation, effect, sound, sky, water and
  character. It decides only how a body moves. Combat, death, experience, loot, quests, the
  economy, and what lives in the world and what it does, belong to the games built on cairn, never
  to the engine. Of anything new, ask: does it show, or does it decide?
- A game is a crate in `games/`, written on the `game` crate alone, that rolls only through its
  `World`; the server, the client and the bots find it by name in `catalog`, and nothing else names
  it.
- A game shows a body only in the install's animations, through `Out::play` and `Out::hold`; the
  client draws those alike for every game and never reads a game's own state to decide what to
  draw.
- What a game's players save, declared with `game::saved!`, is a table in every world's file that
  game has kept. A new field migrates by itself; renaming or retyping one leaves those worlds
  unable to start until a migration is written for it.
- No game data is stored in this repo: no game file, and nothing computed from one, such as a
  palette or a baked table. What the tools need, they compute from the player's install. A
  constant the game's own code uses, such as a speed or a noise table, is code.
- Everyone in view is shown. With distance a crowd loses detail, never presence: nothing hides a
  player in range to save a frame or a byte.
- Movement and the world's state are deterministic: the same inputs give the same result in any
  process, on any number of threads, in a debug or a release build. Where they change, use
  `bevy::math::ops` rather than the platform's float maths, take no order from a hash map's seed,
  from what loaded first or from a time budget, and break ties by value.

## The loop

- `cargo xtask setup` once per clone, then `cargo xtask check` before every commit. It runs
  every gate; `--fast` runs the quick ones. It needs taplo, typos, cargo-deny, cargo-machete and
  cargo-nextest, and says how to install any that is missing.
- Set `WOW_DATA` to the install's `Data` directory. Without it the tests that read the install
  skip, and a green check says nothing about them.
- A change to the client: `cargo xtask pictures <dir>` runs the tests that start the app on the
  GPU and saves their shots in `<dir>`. Look at them.
- Small commits on a branch, with conventional messages: `feat(mpq): read the patch chain`. A branch
  lands on `main` by fast-forward once `cargo xtask check` passes on it.
- Prove it works: run the real thing and read the real output. "It compiles" is not done.

## What the gates enforce

You don't need to remember these; `cargo xtask check` tells you. They are here so you know why
a check fails.

- rustfmt; clippy with pedantic lints, warnings as errors; tests.
- Dependency licenses and advisories (cargo-deny), unused dependencies (cargo-machete),
  spelling (typos), TOML formatting (taplo).
- No game data and no binary files in the tree, untracked files included.
- Comments say only what the code can't, briefly. No block comments. No comment points at a
  document: the code has to stand on its own. No file refers to a decision record, a private repo
  or a path on someone's machine.
- Every crate root opens with a `//!` line saying what the crate is for, and every crate has a
  `description`; the crate list in README.md is generated from them (`cargo xtask map`).
- Rust files stay under 800 lines.
- Commit messages: `type(scope): summary`, subject at most 72 characters, no references.

## What the gates can't judge

- Fix the cause, not the symptom. A workaround is a question to answer, not an answer.
- Delete before adding. The smallest change that solves the problem wins.
- A new file reader gets a test that truncated and bit-flipped copies of the install's files
  give an error, never a panic or a runaway.
- If you catch yourself writing the same instruction twice, make it a rule in `xtask` instead.
  What the next agent must know to avoid a real mistake, and no gate can check, goes in this file
  in a line.
- Before a branch with comments in it lands, a fresh agent reviews them
  (`.claude/agents/comment-reviewer.md`). The author defends its comments; a stranger doesn't.
