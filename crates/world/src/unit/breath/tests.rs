use std::time::Duration;

use super::*;

#[test]
fn a_sub_area_takes_its_zones_climate_one_step_up_unless_it_has_its_own() {
    let areas: HashMap<u32, (u32, u32)> = [
        (1, (0, COLD)),
        (131, (1, 0x40)),
        (999, (131, 0x40)),
        (5, (1, OWN_CLIMATE)),
        (12, (0, 0)),
        (6, (12, COLD | OWN_CLIMATE)),
        (7, (12, COLD)),
    ]
    .into();
    assert!(is_cold(&areas, 1), "a cold zone");
    assert!(is_cold(&areas, 131), "its sub-area");
    assert!(!is_cold(&areas, 999), "one step up, never two");
    assert!(!is_cold(&areas, 5), "a sub-area with its own mild climate");
    assert!(is_cold(&areas, 6), "a cold sub-area of a mild zone");
    assert!(!is_cold(&areas, 7), "a cold flag its zone overrides");
    assert!(!is_cold(&areas, 12) && !is_cold(&areas, 404));
}

fn step(app: &mut App, secs: f32) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(secs));
    app.update();
}

fn cold(app: &App, unit: Entity) -> Option<bool> {
    app.world().get::<BreathEnv>(unit).map(|e| e.cold)
}

#[test]
fn the_viewers_climate_is_its_areas_and_holds_for_ten_seconds() {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<CurrentArea>()
        .init_resource::<Streamer>()
        .init_resource::<Assets<AdtTile>>()
        .insert_resource(BreathTables {
            cold: [131].into(),
            puff: Handle::default(),
        })
        .add_systems(Update, classify_breath);
    let unit = app
        .world_mut()
        .spawn((
            UnitBody {
                display: 49,
                model: String::new(),
                skins: [None, None, None],
                character: None,
            },
            GlobalTransform::default(),
            ViewerUnit,
        ))
        .id();
    step(&mut app, 0.0);
    assert_eq!(cold(&app, unit), None, "no area yet, so asked again");
    app.insert_resource(CurrentArea(Some(131)));
    step(&mut app, 0.1);
    assert_eq!(cold(&app, unit), Some(true));
    app.insert_resource(CurrentArea(Some(12)));
    step(&mut app, 9.0);
    assert_eq!(cold(&app, unit), Some(true), "held");
    step(&mut app, 1.0);
    assert_eq!(
        cold(&app, unit),
        Some(false),
        "looked up again ten seconds on"
    );
}

fn install() -> Option<crate::Install> {
    let Some(data) = std::env::var_os("WOW_DATA") else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(crate::Install::open(std::path::Path::new(&data)).expect("open the install"))
}

#[test]
fn the_cold_is_where_the_client_breathes_and_the_puff_is_its_breath() {
    let Some(install) = install() else { return };
    let mut url = String::new();
    let tables = BreathTables::read(&install.0, |u| {
        url = u;
        Handle::default()
    })
    .expect("the tables read");
    let cold = &tables.cold;
    for (area, name) in [(1, "Dun Morogh"), (131, "Kharanos"), (618, "Winterspring")] {
        assert!(cold.contains(&area), "{name}");
    }
    for (area, name) in [(12, "Elwynn Forest"), (87, "Goldshire")] {
        assert!(!cold.contains(&area), "{name}");
    }
    assert_eq!(cold.len(), 45, "the four cold zones and their sub-areas");
    assert_eq!(url, "mpq://particles/coldbreath.m2");
}

fn unit(app: &mut App, cold: bool) -> Entity {
    app.world_mut()
        .spawn((
            UnitBody {
                display: 49,
                model: String::new(),
                skins: [None, None, None],
                character: None,
            },
            BreathEnv {
                cold,
                stale_at: f32::MAX,
            },
        ))
        .id()
}

fn key(app: &mut App, unit: Entity, ident: [u8; 4]) {
    app.world_mut().write_message(AnimEvent {
        entity: unit,
        ident,
        data: 0,
        anim_id: 0,
        pos: Vec3::ZERO,
    });
}

fn puffs_on(app: &mut App, unit: Entity) -> Vec<Entity> {
    let world = app.world_mut();
    world
        .query::<(Entity, &OneShot)>()
        .iter(world)
        .filter(|(_, s)| s.host == unit)
        .map(|(e, _)| e)
        .collect()
}

#[test]
fn a_cold_unit_puffs_at_each_breath_key_but_never_over_a_puff_still_playing() {
    let Some(install) = install() else { return };
    let mut app = App::new();
    app.insert_resource(CharacterTables::load(&install).expect("the character tables"))
        .insert_resource(BreathTables {
            cold: HashSet::new(),
            puff: Handle::default(),
        })
        .add_message::<AnimEvent>()
        .add_systems(Update, fire_breath);
    let (cold, mild) = (unit(&mut app, true), unit(&mut app, false));
    key(&mut app, cold, BREATH_KEY);
    key(&mut app, cold, BREATH_KEY);
    key(&mut app, cold, *b"$FSD");
    key(&mut app, mild, BREATH_KEY);
    app.update();
    let first = puffs_on(&mut app, cold);
    assert_eq!(first.len(), 1, "one puff for two keys in one frame");
    assert!(
        puffs_on(&mut app, mild).is_empty(),
        "no breath where it is mild"
    );
    key(&mut app, cold, BREATH_KEY);
    app.update();
    assert_eq!(
        puffs_on(&mut app, cold),
        first,
        "none over the one still playing"
    );
    app.world_mut().despawn(first[0]);
    key(&mut app, cold, BREATH_KEY);
    app.update();
    assert_eq!(
        puffs_on(&mut app, cold).len(),
        1,
        "the next loop's key puffs again"
    );
}
