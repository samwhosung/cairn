//! The pose the body holds standing still: X sits it down and stands it up, and moving, a
//! keyboard turn or a jump stands it up. A pose is refused while the body moves or swims.

use world::unit::stand_state::{SIT, SLEEP, STAND};

use super::flags::{ANY_MOVE, SWIMMING, TURN_LEFT, TURN_RIGHT};
use super::input::{Binding, Keys};
use super::state::Player;

/// Runs before the body moves; `live_flags` are the last frame's movement flags.
pub fn update(player: &mut Player, keys: Keys<'_>, moving: bool, turned: bool, live_flags: u32) {
    let toggled = if player.stand_state == STAND {
        SIT
    } else {
        STAND
    };
    let mut request = keys.pressed_now(Binding::SitOrStand).then_some(toggled);
    let stands_up = moving || turned || keys.pressed_now(Binding::Jump);
    if stands_up && player.stand_state != STAND && request.is_none() {
        request = Some(STAND);
    }
    if let Some(state) = request.filter(|&s| !refused(live_flags, s)) {
        player.stand_state = state;
    }
}

/// Standing up is never refused; sleeping is refused while turning too.
fn refused(flags: u32, state: u8) -> bool {
    if state == STAND {
        return false;
    }
    if state == SLEEP && flags & (TURN_LEFT | TURN_RIGHT) != 0 {
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
        assert!(!refused(0, SIT));
        assert!(refused(FORWARD, SIT));
        assert!(refused(SWIMMING, SIT));
        assert!(!refused(TURN_LEFT, SIT));
        assert!(refused(TURN_LEFT, SLEEP));
        assert!(!refused(FORWARD | SWIMMING, STAND));
    }
}
