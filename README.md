# cairn

An engine for World of Warcraft–style worlds, built for AI agents to work in: a client to play,
an editor that agents drive, and a server that scales from one player to a crowded open world.

It uses the look, the movement and the assets of World of Warcraft 1.12.1, read at runtime from
your own copy of the game. No game data is included here, and none ever will be.

**Status:** just started. Nothing runs yet.

## Working on it

    cargo xtask setup   # once per clone: use the repo's git hooks
    cargo xtask check   # every gate CI runs (--fast for the quick ones)

Agents and people follow the same rules; [AGENTS.md](AGENTS.md) has the short version, and the
gates enforce the rest.

## Crates

<!-- crates:start -->
- [`mpq`](crates/mpq) — Reads World of Warcraft 1.12.1 MPQ archives and the patch chain that stacks them
- [`wowfile`](crates/wowfile) — Bounds-checked little-endian reads and the chunk walk the WoW file formats share
- [`xtask`](xtask) — The repo's gates: `cargo xtask check` runs every one CI runs
<!-- crates:end -->

## License

MIT or Apache-2.0, at your option.
