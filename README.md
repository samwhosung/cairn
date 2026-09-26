# cairn

An engine for World of Warcraft–style worlds, built for AI agents to work in: a client to play,
an editor that agents drive, and a server that scales from one player to a crowded open world.

It uses the look, the movement and the assets of World of Warcraft 1.12.1, read at runtime from
your own copy of the game. No game data is included here, and none ever will be.

It is not a game. cairn shows the world as WoW shows it and decides only how you move; combat,
loot, quests and every other rule belong to the games built on it.

**Status:** early. `WOW_DATA=<your install>/Data cargo run -p cairn` walks a character through the
world as the client walks, swims and collides, lit by the hour under its sky and heard as the
client hears it; `-- --help` shows the controls, the looks, the maps, the cameras, the headless
shot, the viewer that stays open for shot after shot, the atlas, which draws a zone from above,
and the catalog, which lays the install's ground textures, doodads, buildings and zones out in a
git-ignored directory for an agent to search and look through. `--host` serves the world from the
window for others to `--connect` to, and the `server` and `bots` crates serve it alone and walk a
crowd against it; `cargo run -p bots -- scenario FILE` runs one of `crates/bots/scenarios` headless
on the server's clock and prints its verdict.

## Working on it

    cargo xtask setup   # once per clone: use the repo's git hooks
    cargo xtask check   # every gate (--fast for the quick ones)

Agents and people follow the same rules; [AGENTS.md](AGENTS.md) has the short version, and the
gates enforce the rest.

## Crates

<!-- crates:start -->
- [`adt`](crates/adt) — Reads World of Warcraft 1.12.1 ADT terrain tiles: heights, textures, liquids and placements
- [`atlas`](crates/atlas) — Draws a zone of World of Warcraft 1.12.1 from above: the ground in its textures' colours, hill shading and water, and doodads and buildings as marks
- [`blp`](crates/blp) — Decodes World of Warcraft 1.12.1 BLP2 textures to RGBA8, or keeps their DXT blocks for the GPU
- [`bots`](crates/bots) — Bots for the server: a crowd over TCP that checks what it is shown, and scenarios run in process on the server's clock, each ending in one verdict
- [`cairn`](crates/cairn) — The client: walks a window through your WoW 1.12.1 install, renders shots of it to PNG files, alone or from a viewer that stays open, draws a zone of it from above, or writes a catalog of its assets
- [`catalog`](crates/catalog) — The games cairn carries, looked up by name for the server, the client and the bots
- [`character`](crates/character) — World of Warcraft 1.12.1 characters and creatures: customization, geosets, the composited skin, and the item and creature displays they wear
- [`dbc`](crates/dbc) — Reads World of Warcraft 1.12.1 DBC tables, given a schema for their columns
- [`game`](crates/game) — The rule API a game on the server is written on: kinds of rows in typed tables, rules that see the world as the last tick left it and change only the row they run for, letters between rows, timers, spawning and knobs, and the tick that runs them
- [`light`](crates/light) — Reads World of Warcraft 1.12.1 lighting tables into fog, sun, sky and water colour by place, weather and time, and traces the sun and moon through the day
- [`m2`](crates/m2) — Reads World of Warcraft 1.12.1 M2 models: mesh, skins, bones, attachments and tracks
- [`melee`](games/melee) — A game on the rule API: every player fights every other hand to hand, and the dead rise at their spawn
- [`model`](crates/model) — The render-ready view of World of Warcraft 1.12.1 M2 and WMO models: batches, bounds, collision, animation
- [`mpq`](crates/mpq) — Reads World of Warcraft 1.12.1 MPQ archives, the patch chain that stacks them, and a directory laid over it
- [`protocol`](crates/protocol) — The native wire between cairn's client and server: frames, the messages each side sends, and their encoding
- [`server`](crates/server) — The world server: a 20 Hz bulk-synchronous tick that checks and relays movement, over TCP or in-process, and keeps the world in an SQLite file
- [`sound`](crates/sound) — World of Warcraft 1.12.1 sound as the client picks and schedules it: its sound tables, the kit player, zone music and ambience and the world's emitters, mixed by kira on the device or offline
- [`survey`](crates/survey) — What the maps of a World of Warcraft 1.12.1 install paint and place, by zone: every ground texture, doodad and building, with where and how often, written out as text and pictures an agent can search and look through
- [`terrain`](crates/terrain) — Meshes World of Warcraft 1.12.1 ADT terrain and liquids, and answers point queries on them
- [`wdl`](crates/wdl) — Reads World of Warcraft 1.12.1 WDL maps: the coarse heights the horizon is drawn from
- [`wdt`](crates/wdt) — Reads World of Warcraft 1.12.1 WDT map tables and maps world coordinates to tiles
- [`wmo`](crates/wmo) — Reads World of Warcraft 1.12.1 WMO world objects: the root file and its group files
- [`world`](crates/world) — World of Warcraft 1.12.1 in Bevy: its files as assets, its world drawn as the client draws it
- [`wowfile`](crates/wowfile) — Bounds-checked little-endian reads and the chunk walk the WoW file formats share
- [`xtask`](xtask) — The repo's gates, all run by `cargo xtask check`
<!-- crates:end -->

## License

MIT or Apache-2.0, at your option.
