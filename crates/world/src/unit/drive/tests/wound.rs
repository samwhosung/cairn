use super::super::super::motion::is_wound;
use super::*;

#[derive(Debug, PartialEq)]
struct Flinch {
    id: u16,
    upper_body_only: bool,
    weight: f32,
    seek: f32,
}

fn flinch(app: &App, unit: Entity) -> Option<Flinch> {
    let e = app.world().entity(unit);
    let player = e.get::<AnimationPlayer>()?;
    let anims = e.get::<ModelAnimations>()?;
    anims
        .clips
        .iter()
        .filter(|c| is_wound(c.anim_id))
        .find_map(|c| {
            let nodes = std::iter::once((c.node, false)).chain(c.upper_node.map(|n| (n, true)));
            nodes.into_iter().find_map(|(node, upper_body_only)| {
                let active = player.animation(node)?;
                Some(Flinch {
                    id: c.anim_id,
                    upper_body_only,
                    weight: active.weight(),
                    seek: active.seek_time(),
                })
            })
        })
}

#[test]
fn a_combat_wound_eases_out_over_the_upper_body_and_leaves_the_base_to_the_gait() {
    let mut app = app();
    let unit = dressed_fighter(&mut app, body_with_upper_nodes);
    frames(&mut app, 2);
    told(&mut app, unit, Some(COMBAT_WOUND), None);
    frames(&mut app, 1);
    assert_eq!(base_and_mode(&app, unit), (Some(STAND), Mode::Gait));
    let hit = flinch(&app, unit).expect("the wound");
    assert!(
        hit.id == COMBAT_WOUND
            && hit.upper_body_only
            && (hit.weight - 3.0).abs() < 1e-4
            && (hit.seek - 0.01).abs() < 1e-6,
        "three to the stand's one, three quarters of the pose: {hit:?}"
    );
    frames(&mut app, 50);
    let half_way = flinch(&app, unit).expect("the wound");
    assert!(
        (half_way.weight - 0.6).abs() < 1e-3,
        "three eighths of the pose half way: {half_way:?}"
    );
    frames(&mut app, 60);
    assert_eq!(flinch(&app, unit), None, "eased out and let go");
    assert_eq!(base_and_mode(&app, unit), (Some(STAND), Mode::Gait));
}

#[test]
fn a_stand_wound_on_a_still_body_takes_the_whole_body_until_the_gait_moves_on() {
    let mut app = app();
    let unit = dressed_fighter(&mut app, body_with_upper_nodes);
    frames(&mut app, 2);
    told(&mut app, unit, Some(STAND_WOUND), None);
    frames(&mut app, 1);
    let hit = flinch(&app, unit).expect("the wound");
    assert!(hit.id == STAND_WOUND && !hit.upper_body_only, "{hit:?}");
    assert_eq!(base_and_mode(&app, unit), (Some(STAND), Mode::Gait));
    frames(&mut app, 20);
    moving(&mut app, unit, FORWARD, 7.0, 0.0);
    frames(&mut app, 1);
    assert_eq!(base_and_mode(&app, unit), (Some(RUN), Mode::Gait));
    assert_eq!(flinch(&app, unit), None, "the run's arm takes its slot");
}

#[test]
fn a_swing_above_the_spine_takes_a_wounds_slot_there_and_a_whole_body_one_does_not() {
    let mut app = app();
    let unit = dressed_fighter(&mut app, body_with_upper_nodes);
    moving(&mut app, unit, FORWARD, 7.0, 0.0);
    frames(&mut app, 2);
    told(&mut app, unit, Some(COMBAT_WOUND), None);
    frames(&mut app, 10);
    assert!(flinch(&app, unit).is_some_and(|hit| hit.upper_body_only));
    told(&mut app, unit, Some(ATTACK_UNARMED), None);
    frames(&mut app, 1);
    assert_eq!(flinch(&app, unit), None, "a swing told on the run takes it");
    assert_eq!(upper_body(&app, unit).map(|s| s.id), Some(ATTACK_UNARMED));

    frames(&mut app, 20);
    told(&mut app, unit, Some(COMBAT_WOUND), None);
    frames(&mut app, 1);
    let hit = flinch(&app, unit).expect("the wound over the swing");
    assert!(
        (hit.weight - 27.0).abs() < 1e-3,
        "three quarters against the gait's one and the swing's eight: {hit:?}"
    );
    assert_eq!(upper_body(&app, unit).map(|s| s.id), Some(ATTACK_UNARMED));

    let unit = dressed_fighter(&mut app, body_with_upper_nodes);
    frames(&mut app, 2);
    told(&mut app, unit, Some(COMBAT_WOUND), None);
    frames(&mut app, 10);
    told(&mut app, unit, Some(ATTACK_UNARMED), None);
    frames(&mut app, 1);
    assert_eq!(
        base_and_mode(&app, unit),
        (Some(ATTACK_UNARMED), Mode::ShowPlayed(ATTACK_UNARMED))
    );
    assert!(
        flinch(&app, unit).is_some(),
        "a swing on the whole body leaves the wound easing out over it"
    );
}

#[test]
fn a_wound_told_again_starts_over_and_one_the_model_lacks_changes_nothing() {
    let mut app = app();
    let unit = dressed_fighter(&mut app, body_with_upper_nodes);
    frames(&mut app, 2);
    told(&mut app, unit, Some(COMBAT_WOUND), None);
    frames(&mut app, 30);
    told(&mut app, unit, Some(COMBAT_WOUND), None);
    frames(&mut app, 1);
    let again = flinch(&app, unit).expect("the wound");
    assert!(
        (again.seek - 0.01).abs() < 1e-6 && (again.weight - 3.0).abs() < 1e-4,
        "{again:?}"
    );

    let bare = body(&mut app, &WALKER);
    frames(&mut app, 2);
    told(&mut app, bare, Some(COMBAT_WOUND), None);
    frames(&mut app, 1);
    assert_eq!(flinch(&app, bare), None);
    assert_eq!(base_and_mode(&app, bare), (Some(STAND), Mode::Gait));
}

#[test]
fn over_a_ready_stance_a_combat_wound_takes_the_whole_body() {
    let mut app = app();
    let unit = dressed_fighter(&mut app, body_with_upper_nodes);
    frames(&mut app, 2);
    idled(&mut app, unit, Some(READY_UNARMED));
    frames(&mut app, 2);
    told(&mut app, unit, Some(COMBAT_WOUND), None);
    frames(&mut app, 1);
    let hit = flinch(&app, unit).expect("the wound");
    assert!(!hit.upper_body_only, "{hit:?}");
    assert_eq!(base_and_mode(&app, unit), (Some(READY_UNARMED), Mode::Gait));
}
