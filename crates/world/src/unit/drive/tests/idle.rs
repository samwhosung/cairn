use super::super::super::motion::anim::READY_1H;
use super::*;

#[test]
fn an_idled_body_stands_in_its_idle_moves_as_it_would_and_stands_once_let_go() {
    let mut app = app();
    let unit = dressed_fighter(&mut app, body_with_upper_nodes);
    frames(&mut app, 2);
    idled(&mut app, unit, Some(READY_UNARMED));
    frames(&mut app, 1);
    assert_eq!(base_and_mode(&app, unit), (Some(READY_UNARMED), Mode::Gait));
    moving(&mut app, unit, FORWARD, 7.0, 0.0);
    frames(&mut app, 1);
    assert_eq!(base_and_mode(&app, unit), (Some(RUN), Mode::Gait));
    moving(&mut app, unit, TURN_LEFT, 0.0, 0.0);
    frames(&mut app, 1);
    assert_eq!(base_and_mode(&app, unit).0, Some(SHUFFLE_LEFT));
    moving(&mut app, unit, 0, 0.0, 0.0);
    frames(&mut app, 1);
    assert_eq!(
        base_and_mode(&app, unit).0,
        Some(READY_UNARMED),
        "the shuffle ends with the turn, as it does only against Stand"
    );
    idled(&mut app, unit, None);
    frames(&mut app, 1);
    assert_eq!(base_and_mode(&app, unit), (Some(STAND), Mode::Gait));
}

#[test]
fn a_weapons_ready_stance_falls_back_to_the_unarmed_one_and_a_pose_or_a_sit_wins() {
    let mut app = app();
    let unit = dressed_fighter(&mut app, body_with_upper_nodes);
    frames(&mut app, 2);
    idled(&mut app, unit, Some(READY_1H));
    frames(&mut app, 1);
    assert_eq!(base_and_mode(&app, unit).0, Some(READY_UNARMED));
    told(&mut app, unit, None, Some(DEAD));
    frames(&mut app, 1);
    assert_eq!(base_and_mode(&app, unit).1, Mode::ShowPosed(DEAD));
    told(&mut app, unit, None, None);
    posed(&mut app, unit, StandState::SIT);
    frames(&mut app, 2);
    assert_eq!(
        base_and_mode(&app, unit),
        (
            Some(SIT_GROUND),
            Mode::Looping(Bracketed::Pose(StandState::SIT))
        )
    );
}
