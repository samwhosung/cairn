# cairn

An engine for World of Warcraft–style worlds, built for AI agents to work in: a client to play,
an editor that agents drive, and a server that scales from one player to a crowded open world.

It uses the look, the movement and the assets of World of Warcraft 1.12.1, read at runtime from
your own copy of the game. No game data is included here, and none ever will be.

**Status:** just started. `WOW_DATA=<your install>/Data cargo run -p cairn` walks a window over the
terrain as the client walks, swims and collides, lit by the hour under its sky and horizon;
`-- --help` shows the controls, the maps, the cameras and the headless shot.

## Working on it

    cargo xtask setup   # once per clone: use the repo's git hooks
    cargo xtask check   # every gate CI runs (--fast for the quick ones)

Agents and people follow the same rules; [AGENTS.md](AGENTS.md) has the short version, and the
gates enforce the rest.

## Crates

<!-- crates:start -->
- [`adt`](crates/adt) — Reads World of Warcraft 1.12.1 ADT terrain tiles: heights, textures, liquids and placements
- [`blp`](crates/blp) — Decodes World of Warcraft 1.12.1 BLP2 textures to RGBA8, or keeps their DXT blocks for the GPU
- [`cairn`](crates/cairn) — The client: walks a window through your WoW 1.12.1 install, or renders one shot of it to a PNG
- [`character`](crates/character) — World of Warcraft 1.12.1 characters and creatures: customization, geosets, the composited skin, and the item and creature displays they wear
- [`dbc`](crates/dbc) — Reads World of Warcraft 1.12.1 DBC tables, given a schema for their columns
- [`light`](crates/light) — Reads World of Warcraft 1.12.1 lighting tables into fog, sun, sky and water colour by place, weather and time, and traces the sun and moon through the day
- [`m2`](crates/m2) — Reads World of Warcraft 1.12.1 M2 models: mesh, skins, bones, attachments and tracks
- [`model`](crates/model) — The render-ready view of World of Warcraft 1.12.1 M2 and WMO models: batches, bounds, collision, animation
- [`mpq`](crates/mpq) — Reads World of Warcraft 1.12.1 MPQ archives and the patch chain that stacks them
- [`sound`](crates/sound) — World of Warcraft 1.12.1 sound as the client picks and schedules it: its sound tables, the kit player, zone music and ambience and the world's emitters, mixed by kira on the device or offline
- [`terrain`](crates/terrain) — Meshes World of Warcraft 1.12.1 ADT terrain and liquids, and answers point queries on them
- [`wdl`](crates/wdl) — Reads World of Warcraft 1.12.1 WDL maps: the coarse heights the horizon is drawn from
- [`wdt`](crates/wdt) — Reads World of Warcraft 1.12.1 WDT map tables and maps world coordinates to tiles
- [`wmo`](crates/wmo) — Reads World of Warcraft 1.12.1 WMO world objects: the root file and its group files
- [`world`](crates/world) — World of Warcraft 1.12.1 in Bevy: its files as assets, its world drawn as the client draws it
- [`wowfile`](crates/wowfile) — Bounds-checked little-endian reads and the chunk walk the WoW file formats share
- [`xtask`](xtask) — The repo's gates: `cargo xtask check` runs every one CI runs
<!-- crates:end -->

## License

MIT or Apache-2.0, at your option.
