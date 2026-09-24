use world::unit::StandState;

use super::flags::{ANY_MOVE, SWIMMING, TURN_LEFT, TURN_RIGHT};
use super::input::{Binding, Keys};
use super::state::Player;

pub fn update(
    player: &mut Player,
    keys: Keys<'_>,
    moving: bool,
    turned: bool,
    last_live_flags: u32,
) {
    let toggled = if player.stand_state == StandState::STAND {
        StandState::SIT
    } else {
        StandState::STAND
    };
    let mut request = keys.pressed_now(Binding::SitOrStand).then_some(toggled);
    let stands_up = moving || turned || keys.pressed_now(Binding::Jump);
    if stands_up && player.stand_state != StandState::STAND && request.is_none() {
        request = Some(StandState::STAND);
    }
    if let Some(state) = request.filter(|&s| !refused(last_live_flags, s)) {
        player.stand_state = state;
    }
}

fn refused(flags: u32, state: StandState) -> bool {
    if state == StandState::STAND {
        return false;
    }
    if state == StandState::SLEEP && flags & (TURN_LEFT | TURN_RIGHT) != 0 {
        return true;
    }
    flags & (ANY_MOVE | SWIMMING) != 0
}

#[cfg(test)]
mod tests {
    use super::super::flags::FORWARD;
    use super::*;

    #[test]
    fn a_pose_is_refused_moving_or_swimming_and_standing_never_is() {
        let (sit, sleep) = (StandState::SIT, StandState::SLEEP);
        assert!(!refused(0, sit));
        assert!(refused(FORWARD, sit));
        assert!(refused(SWIMMING, sit));
        assert!(!refused(TURN_LEFT, sit));
        assert!(refused(TURN_LEFT, sleep));
        assert!(!refused(FORWARD | SWIMMING, StandState::STAND));
    }
}
