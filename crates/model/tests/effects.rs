#![allow(clippy::float_cmp)]

use std::path::PathBuf;

use model::{
    ParticleBlend, ParticleEmitterDef, ParticleShape, RibbonEmitterDef, parse_m2_particle_emitters,
    parse_m2_ribbon_emitters,
};
use mpq::Chain;

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn emitters(chain: &Chain, name: &str) -> Vec<ParticleEmitterDef> {
    parse_m2_particle_emitters(&chain.read(name).unwrap_or_else(|e| panic!("{e}")))
}

fn ribbons(chain: &Chain, name: &str) -> Vec<RibbonEmitterDef> {
    parse_m2_ribbon_emitters(&chain.read(name).unwrap_or_else(|e| panic!("{e}")))
}

#[test]
fn a_campfire_pours_a_slow_glow_and_a_fast_flame() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let fire = emitters(
        &chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2",
    );
    assert_eq!(fire.len(), 2);
    for e in &fire {
        assert_eq!(
            (e.shape, e.blend),
            (ParticleShape::Plane, ParticleBlend::Add)
        );
        assert!(e.scale_size_by_instance() && !e.model_space());
        assert!(
            e.params.sample(None, 0.0, 0.0).horizontal_range > 6.0,
            "a full ring"
        );
        assert!(e.texture.is_some());
    }
    let (glow, flame) = (&fire[0], &fire[1]);
    assert!((glow.params.sample(None, 0.0, 0.0).lifespan - 4.0).abs() < 1e-3);
    assert_eq!(glow.timing.constant_rate(), Some(6.0));
    assert_eq!((glow.tile_rows, glow.tile_cols), (1, 1));
    assert_eq!(glow.drag, 0.5);
    assert!((flame.params.sample(None, 0.0, 0.0).lifespan - 1.5).abs() < 1e-3);
    assert_eq!(flame.timing.constant_rate(), Some(20.0));
    assert_eq!((flame.tile_rows, flame.tile_cols), (4, 4));
    assert_eq!(flame.drag, 0.0);
    let ol = &glow.over_life;
    assert!(
        ol.sample(1.0).color[3] <= ol.sample(0.0).color[3],
        "the glow fades"
    );
}

#[test]
fn the_kobold_candle_burns_at_its_ramp_size() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let kobold = emitters(&chain, "Creature\\Kobold\\Kobold.m2");
    assert_eq!(kobold.len(), 1);
    let candle = &kobold[0];
    assert_eq!(candle.flags, 0x01);
    assert!(!candle.lit && !candle.model_space());
    assert_eq!(candle.twinkle(0.7), 1.0, "a {{0,0}} twinkle is steady");
    assert!(candle.over_life.sample(0.5).size > 0.0);
}

#[test]
fn a_decreasing_cell_pair_plays_its_flipbook_backwards() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let brazier = emitters(
        &chain,
        "World\\Generic\\Dwarf\\Passive Doodads\\Braziers\\DwarvenBrazier01.m2",
    );
    let ol = &brazier[1].over_life;
    assert_eq!((ol.head_cells[0].begin, ol.head_cells[0].end), (0, 5));
    assert_eq!((ol.head_cells[1].begin, ol.head_cells[1].end), (6, 5));
    assert!((0..=1000).all(|i| ol.sample(i as f32 / 1000.0).head_cell <= 6));
}

#[test]
fn an_insect_swarm_flaps_its_flipbook_five_times() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let swarm = emitters(&chain, "SPELLS\\InsectSwarm_State_Chest.m2");
    let ol = &swarm[0].over_life;
    assert_eq!(ol.repeat, [5.0, 5.0]);
    let mut wraps = 0;
    let mut prev = ol.sample(0.0).head_cell;
    for i in 1..=2000 {
        let c = ol.sample(ol.mid * (i as f32 / 2000.0)).head_cell;
        wraps += usize::from(c < prev);
        prev = c;
    }
    assert_eq!(wraps, 4);
}

#[test]
fn the_inn_chandelier_candles_ride_its_swing() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let chandelier = emitters(
        &chain,
        "World\\Dungeon\\GoldshireInn\\InnChandelier\\InnChandelier.m2",
    );
    assert!(
        chandelier
            .iter()
            .take(6)
            .all(ParticleEmitterDef::model_space)
    );
}

#[test]
fn waterfall_spray_takes_the_scenes_light() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let fall = emitters(
        &chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Waterfall\\ElwynnTallWaterfall01.m2",
    );
    assert_eq!(fall.len(), 1);
    assert_eq!(
        (fall[0].flags, fall[0].blend),
        (0x0002, ParticleBlend::Alpha)
    );
    assert!(fall[0].lit);
}

#[test]
fn a_quest_barrel_explodes_only_inside_its_clips() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let barrel = emitters(&chain, "World\\Goober\\G_BarrelExplode.m2");
    assert_eq!(barrel.len(), 7);
    for e in &barrel {
        assert!(e.timing.peak_rate() > 0.0);
        for slot in [1usize, 2] {
            for t in [0.0f32, 0.1, 0.3, 5.0] {
                assert!(!e.timing.emitting(Some(slot), t, 0.0), "quiet at rest");
            }
        }
        assert!((0..100).any(|k| e.timing.emitting(Some(0), k as f32 * 0.01, 0.0)));
        assert!(
            !e.timing.emitting(Some(0), 1.0, 0.0),
            "off at the clip's end"
        );
        assert!(!e.timing.emitting(Some(0), 30.0, 0.0));
    }
}

#[test]
fn a_rock_elementals_chips_are_alpha_keyed() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let rocks = emitters(&chain, "Creature\\ElementalEarth\\ElementalEarth.m2");
    assert_eq!(rocks.len(), 13);
    assert_eq!(rocks[3].blend, ParticleBlend::AlphaKey);
    assert_eq!(rocks[12].blend, ParticleBlend::AlphaKey);
    assert_eq!(rocks[11].blend, ParticleBlend::Alpha);
}

#[test]
fn a_blood_spurt_fires_on_its_keys() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let spurt = emitters(&chain, "Particles\\BloodSpurts\\BloodSpurt.m2");
    assert_eq!(spurt.len(), 4);
    assert_eq!(spurt[0].timing.rate(None, 0.0, 0.0), 100.0);
    let flash = &spurt[3];
    assert_eq!(flash.blend, ParticleBlend::Add);
    assert_eq!(flash.timing.rate(None, 0.0, 0.0), 0.0);
    assert_eq!(flash.timing.peak_rate(), 20.0);
    assert_eq!(flash.timing.rate(None, 0.080, 0.0), 20.0);
    assert!(spurt[0].timing.emitting(None, 0.4, 0.0));
    assert!(!spurt[0].timing.emitting(None, 0.6, 0.0));
}

#[test]
fn a_flare_whose_gate_closes_as_its_rate_opens_never_fires() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let impact = emitters(&chain, "Spells\\Strike_Impact_Chest.m2");
    assert_eq!(impact.len(), 2);
    assert!(impact.iter().all(ParticleEmitterDef::burst));
    assert_eq!(impact[0].timing.first_burst(Some(0)), None);
    assert!(impact[0].timing.peak_rate() > 0.0);
    let (t, n) = impact[1]
        .timing
        .first_burst(Some(0))
        .expect("the plume fires");
    assert_eq!(n, 30.0);
    assert!((0.0..=0.067).contains(&t));
}

#[test]
fn flamestrike_columns_stand_on_their_splines() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let strike = emitters(&chain, "Spells\\FlameStrike_Area.m2");
    let splines: Vec<_> = strike
        .iter()
        .filter(|d| d.shape == ParticleShape::Spline)
        .collect();
    assert_eq!(splines.len(), 4);
    for d in splines {
        let s = d.spline.as_ref().expect("a chain");
        assert_eq!(s.points.len() % 3, 1);
        let now = d.params.sample(None, 0.0, 0.0);
        assert_eq!((now.area_length, now.area_width), (0.0, 1.0));
        assert!(d.burst());
        assert_eq!(s.eval(0.0), s.points[0]);
    }
}

#[test]
fn a_smite_slash_flares_and_fades() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let slash = ribbons(&chain, "Spells\\HolySmite_Low_Chest.m2");
    assert_eq!(slash.len(), 2);
    for r in &slash {
        assert_eq!(r.height_above.first(), 0.0);
        assert!((r.height_above.peak() - 0.167).abs() < 1e-3);
        assert!((r.height_above.sample_ms(200.0) - 0.167).abs() < 1e-3);
        assert_eq!(r.height_above.sample_ms(400.0), 0.0);
        assert!(r.alpha.sample_ms(600.0) < 1e-6);
        assert_eq!(r.visible, None);
    }
}

#[test]
fn a_thrown_dagger_trails_only_in_flight() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let dagger = ribbons(
        &chain,
        "Item\\ObjectComponents\\Weapon\\Thrown_1H_Dagger_A_01.m2",
    );
    let vis = dagger[0].visible.as_ref().expect("gated");
    assert!(!vis.at(0, 0.0) && vis.at(144, 0.0) && !vis.at(191, 0.0));
}

#[test]
fn a_frost_trap_lights_its_column_only_when_sprung() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let trap = ribbons(&chain, "World\\Goober\\G_FrostTrap.m2");
    assert_eq!(trap.len(), 16);
    for r in &trap[0..4] {
        let v = r.visible.as_ref().expect("gated");
        assert!(!v.at(145, 0.0) && v.at(145, 0.6) && v.at(147, 0.0));
    }
    for r in &trap[4..16] {
        let v = r.visible.as_ref().expect("gated");
        assert!(!v.at(147, 0.0) && !v.at(153, 0.0) && v.at(153, 0.5) && !v.at(153, 1.45));
    }
}

#[test]
fn wisp_and_blade_trails_resolve() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let wisp = ribbons(&chain, "Creature\\WISP\\WispRed.m2");
    assert_eq!(wisp.len(), 3);
    for r in &wisp {
        assert!(r.edges_per_second > 0.0 && r.edge_lifetime >= 0.25);
        assert!(r.height_above.peak() + r.height_below.peak() > 0.0);
        assert!(r.texture.is_some());
    }
    let blade = ribbons(
        &chain,
        "ITEM\\ObjectComponents\\WEAPON\\Sword_1H_Thunderblade_A_01.m2",
    );
    assert_eq!(blade.len(), 3);
    let torch = ribbons(&chain, "World\\Generic\\PassiveDoodads\\Lights\\Torch.m2");
    assert!(torch.is_empty());
}
