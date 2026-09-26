# cairn

An engine for World of Warcraft 1.12–style worlds: a client, a server, and an editor to come.

It reads the game's models, textures and sounds from your own copy of WoW 1.12.1 at run time. No
game data is included here.

cairn draws the world and moves you through it. Everything else, such as combat, quests or loot,
belongs to the games built on it.

## Status

Early. Set `WOW_DATA` to your install's `Data` folder, then:

    cargo run -p cairn -- --help

It lists what the client does: walk the world, host or join others, open a zone of your own, take
shots, and more.

## Working on it

    cargo xtask setup   # once per clone
    cargo xtask check   # every check (--fast for the quick ones)

[AGENTS.md](AGENTS.md) has the rules, for people and agents alike.

## Crates

<!-- crates:start -->
- [`adt`](crates/adt) — Reads WoW 1.12.1 ADT terrain tiles
- [`atlas`](crates/atlas) — Draws a zone from above
- [`blp`](crates/blp) — Decodes WoW 1.12.1 BLP textures
- [`bots`](crates/bots) — Bots that load the server, and scripted scenarios with a verdict
- [`cairn`](crates/cairn) — The client: the game window, shots, the viewer, the atlas, the asset catalog and the zone verbs
- [`catalog`](crates/catalog) — The games cairn can run, by name
- [`character`](crates/character) — Characters and creatures: their looks, skins and gear
- [`dbc`](crates/dbc) — Reads WoW 1.12.1 DBC tables
- [`document`](crates/document) — A zone of its own as a document: typed commands, a journal, undo by author, and the tiles cairn draws
- [`fits`](crates/fits) — What fits a spot: models ranked by what the install places together
- [`game`](crates/game) — The API a game is written on
- [`light`](crates/light) — Lighting: sky, fog, sun and water colour by place and time
- [`m2`](crates/m2) — Reads WoW 1.12.1 M2 models
- [`melee`](games/melee) — A small sample game: everyone fights hand to hand
- [`model`](crates/model) — Models ready to draw: M2 and WMO batches, bounds, collision and animation
- [`mpq`](crates/mpq) — Reads WoW 1.12.1 MPQ archives
- [`protocol`](crates/protocol) — The wire between client and server
- [`server`](crates/server) — The world server
- [`sound`](crates/sound) — Sound: music, ambience and effects, played as the game plays them
- [`survey`](crates/survey) — What each zone's maps paint and place, for the catalog
- [`terrain`](crates/terrain) — Terrain and water meshes, and height queries
- [`wdl`](crates/wdl) — Reads WoW 1.12.1 WDL horizon maps
- [`wdt`](crates/wdt) — Reads WoW 1.12.1 WDT map tables
- [`wmo`](crates/wmo) — Reads WoW 1.12.1 WMO buildings
- [`world`](crates/world) — Draws the world in Bevy
- [`wowfile`](crates/wowfile) — Shared reading code for WoW's file formats
- [`xtask`](xtask) — The repo's checks
<!-- crates:end -->

## License

MIT or Apache-2.0, at your option.
