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
- [`blp`](crates/blp) — Decodes World of Warcraft 1.12.1 BLP2 textures to RGBA8, or keeps their DXT blocks for the GPU
- [`dbc`](crates/dbc) — Reads World of Warcraft 1.12.1 DBC tables, given a schema for their columns
- [`m2`](crates/m2) — Reads World of Warcraft 1.12.1 M2 models: mesh, skins, bones, attachments and tracks
- [`mpq`](crates/mpq) — Reads World of Warcraft 1.12.1 MPQ archives and the patch chain that stacks them
- [`wdt`](crates/wdt) — Reads World of Warcraft 1.12.1 WDT map tables and maps world coordinates to tiles
- [`wmo`](crates/wmo) — Reads World of Warcraft 1.12.1 WMO world objects: the root file and its group files
- [`wowfile`](crates/wowfile) — Bounds-checked little-endian reads and the chunk walk the WoW file formats share
- [`xtask`](xtask) — The repo's gates: `cargo xtask check` runs every one CI runs
<!-- crates:end -->

## License

MIT or Apache-2.0, at your option.
