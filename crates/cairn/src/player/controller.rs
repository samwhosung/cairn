//! The per-frame controller, in the client's order.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};
use world::WorldCamera;
use world::collision::{CollisionResidency, Liquids, WorldCollision};
use world::unit::{UnitAlpha, UnitMotion};

use super::camera::{
    self, CameraControl, CameraPivot, CameraRig, FollowInput, Subject, model_pivot_height,
};
use super::camera_dynamics::{CameraOptions, DynamicsInput, SubjectState};
use super::flags::{self, BACKWARD, FORWARD, SWIMMING, WALK_MODE};
use super::input::{self, Binding, Keys};
use super::state::{
    CAPSULE_HEIGHT, CAPSULE_RADIUS, MOUSELOOK_PITCH_CLAMP, Player, RUN_BACK_RATIO, RUN_SPEED,
    TURN_RATE, TURN_RATE_MOVING, WALK_RATIO,
};
use super::{PlayerBody, PlayerCapsule, mover, swim};

/// Wheel pixels per notch, for trackpads.
const PIXELS_PER_NOTCH: f32 = 20.0;

pub fn current_speed(flags: u32) -> f32 {
    let (walk, run, back) = (
        RUN_SPEED * WALK_RATIO,
        RUN_SPEED,
        RUN_SPEED * RUN_BACK_RATIO,
    );
    if flags & WALK_MODE != 0 {
        walk.min(run)
    } else if flags & BACKWARD != 0 {
        back.min(run)
    } else {
        run
    }
}

pub type CameraQuery<'w, 's> =
    Query<'w, 's, (&'static mut Transform, &'static mut CameraRig), With<WorldCamera>>;
pub type BodyQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut UnitMotion,
        &'static mut UnitAlpha,
        Option<&'static CameraPivot>,
    ),
    (With<PlayerBody>, Without<WorldCamera>),
>;

#[derive(bevy::ecs::system::SystemParam)]
pub struct Inputs<'w, 's> {
    keys: Res<'w, ButtonInput<KeyCode>>,
    buttons: Res<'w, ButtonInput<MouseButton>>,
    motion: Res<'w, AccumulatedMouseMotion>,
    scroll: Res<'w, AccumulatedMouseScroll>,
    window: Query<'w, 's, (&'static mut Window, &'static mut CursorOptions), With<PrimaryWindow>>,
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct Surroundings<'w, 's> {
    collide: WorldCollision<'w, 's>,
    liquids: Liquids<'w, 's>,
    residency: Res<'w, CollisionResidency>,
    capsule: Res<'w, PlayerCapsule>,
    options: Res<'w, CameraOptions>,
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments, clippy::float_cmp)]
pub fn control(
    time: Res<'_, Time>,
    mut inputs: Inputs<'_, '_>,
    world: Surroundings<'_, '_>,
    mut player: ResMut<'_, Player>,
    mut rig: ResMut<'_, CameraControl>,
    mut camera: CameraQuery<'_, '_>,
    mut body: BodyQuery<'_, '_>,
    mut settle: Local<'_, SettleClock>,
) {
    let Ok((mut cam_t, mut cam)) = camera.single_mut() else {
        return;
    };
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    let keys = Keys {
        keys: &inputs.keys,
        mouse: &inputs.buttons,
    };
    if player.settling && (world.residency.settled() || settle.stalled(&world.residency, now)) {
        release_settle(&mut player, &world.collide);
    }

    let world_press = rig.look.is_some()
        || inputs
            .window
            .single()
            .map_or(true, |(w, _)| w.cursor_position().is_some());
    rig.world_mouse.update(&inputs.buttons, world_press);
    let input::LookInput {
        both_buttons,
        camera_command,
    } = input::look_input(keys, &player, &rig);
    let dynamics = DynamicsInput {
        options: *world.options,
        smooth_style: rig.follow_config.style,
        subject: SubjectState {
            last_move_flags: player.move_flags,
            facing: player.face_yaw,
            speed: if player.move_flags & flags::ANY_MOVE == 0 {
                0.0
            } else {
                current_speed(player.move_flags)
            },
            camera_command,
        },
        nearclip: world::NEARCLIP,
        surface_y: player.liquid_surface,
    };
    let window = inputs
        .window
        .single_mut()
        .ok()
        .map(|(w, c)| (w.into_inner(), c.into_inner()));
    camera::run_look_session(
        &inputs.buttons,
        inputs.motion.delta,
        both_buttons,
        &mut rig,
        &mut cam,
        &mut player.face_yaw,
        window,
        &dynamics,
    );
    let notches = match inputs.scroll.unit {
        MouseScrollUnit::Line => inputs.scroll.delta.y,
        MouseScrollUnit::Pixel => inputs.scroll.delta.y / PIXELS_PER_NOTCH,
    };
    camera::apply_zoom_scroll(notches, dt, &mut rig);

    let axes = input::move_axes(keys, &mut player, &rig, both_buttons);
    let mut turn_delta = 0.0;
    if axes.turning {
        let turn = f32::from(i8::from(axes.turn_left) - i8::from(axes.turn_right));
        let slowed = axes.translating || player.airborne_since.is_some();
        let rate = TURN_RATE * if slowed { TURN_RATE_MOVING } else { 1.0 };
        turn_delta = turn * rate * dt;
        player.face_yaw += turn_delta;
    }
    let face_rot = Quat::from_rotation_y(player.face_yaw);
    let flat = |v: Vec3| v.with_y(0.0).normalize_or_zero();
    let (move_fwd, move_right) = (flat(face_rot * Vec3::NEG_Z), flat(face_rot * Vec3::X));
    let dir = move_fwd * axes.fwd.signum() as f32 + move_right * axes.side.signum() as f32;
    let moving = dir != Vec3::ZERO;
    let speed = current_speed(
        if axes.fwd < 0 {
            BACKWARD
        } else {
            flags::FORWARD
        } | if player.walking { WALK_MODE } else { 0 },
    );
    let want_jump = keys.pressed_now(Binding::Jump);

    let surface_y = swim::surface_over_feet(&world.liquids, player.pos);
    player.liquid_surface = surface_y;
    let swimming = swim::update_swimming(&mut player, surface_y, now);
    let breach = swimming && want_jump;
    if breach {
        player.swimming = false;
    }
    let swimming = swimming && !breach;
    let swim_amounts = swimming.then(|| swim::translate_amounts(&axes));
    // Pushed only when the mouse moves it, so a still mouse keeps the swim exit's levelling.
    let aim_pitch = cam.pitch + rig.smart_pivot.bias();
    if axes.mouselook && aim_pitch != player.aim_pitch_seen {
        player.aim_pitch_seen = aim_pitch;
        player.mover_pitch = aim_pitch.clamp(-MOUSELOOK_PITCH_CLAMP, MOUSELOOK_PITCH_CLAMP);
    }

    let launch_y = player.pos.y;
    let capsule = &world.capsule.0;
    let (outcome, swim_pitch) = if breach {
        (
            swim::breach_step(&mut player, time.delta(), &world.collide, capsule),
            0.0,
        )
    } else if let Some(amounts) = swim_amounts {
        swim::drive_step(
            &mut player,
            &time,
            &world.collide,
            capsule,
            &world.liquids,
            surface_y,
            (move_fwd, move_right),
            amounts,
        )
    } else {
        let o = mover::step(
            &mut player,
            &time,
            &world.collide,
            capsule,
            moving,
            dir,
            speed,
            want_jump,
        );
        (o, 0.0)
    };
    let airborne = !swimming && !outcome.settling && (!outcome.grounded || outcome.jumped);
    let frame = flags::this_frame(
        &mut player,
        &axes,
        swim_amounts,
        airborne,
        outcome.jumped,
        outcome.settling,
        outcome.air_nudged,
        now,
        launch_y,
    );
    player.move_flags = frame.live;
    let anim_flags = super::gait::drive_body_heading(
        &mut player,
        frame.pose,
        dt,
        swimming,
        moving,
        airborne,
        axes.turning || axes.mouselook,
        TURN_RATE,
    );

    let (scale, pivot) = body
        .single()
        .ok()
        .map_or((1.0, None), |(t, .., pivot)| (t.scale.x, pivot.copied()));
    let subject = Subject {
        feet: player.pos,
        head: player.pos + Vec3::Y * (CAPSULE_HEIGHT - CAPSULE_RADIUS),
        pivot_target: pivot.map(|p| model_pivot_height(p, scale, frame.live & SWIMMING != 0)),
        turn_delta,
    };
    let follow = FollowInput {
        cfg: rig.follow_config,
        face_yaw: player.face_yaw,
        command: camera_command,
    };
    camera::seat_camera(
        dt,
        &subject,
        &mut rig,
        &mut cam,
        &mut cam_t,
        &world.collide,
        &follow,
        &dynamics,
    );
    if let Ok((mut t, mut motion, mut alpha, _)) = body.single_mut() {
        t.translation = player.pos;
        let stroking = frame.live & SWIMMING != 0 && frame.live & (FORWARD | BACKWARD) != 0;
        t.rotation = if stroking {
            Quat::from_rotation_y(player.model_yaw) * Quat::from_rotation_x(swim_pitch)
        } else {
            Quat::from_rotation_y(player.model_yaw)
        };
        *motion = UnitMotion {
            speed: if swimming {
                player.swim_stroke_speed
            } else {
                player.horiz_vel.length()
            },
            vertical_speed: player.vel_y,
            flags: anim_flags,
        };
        alpha.alpha = rig.self_fade_alpha;
    }
}

/// Seconds of no streaming progress after which the settle gives up and lets the body go.
const SETTLE_TIMEOUT: f32 = 6.0;

/// When the collision last made progress.
#[derive(Default)]
pub struct SettleClock {
    pending: Option<usize>,
    since: f32,
}

impl SettleClock {
    fn stalled(&mut self, residency: &CollisionResidency, now: f32) -> bool {
        if self.pending != Some(residency.pending()) {
            self.pending = Some(residency.pending());
            self.since = now;
        }
        let stalled = now - self.since > SETTLE_TIMEOUT;
        if stalled {
            warn!(
                "the collision stalled with {} pending; letting the body go",
                residency.pending()
            );
        }
        stalled
    }
}

fn release_settle(player: &mut Player, collide: &WorldCollision<'_, '_>) {
    const REACH: f32 = 2.0;
    player.settling = false;
    let from = player.pos + Vec3::Y * REACH;
    if let Some(hit) = collide.ray_body(from, Dir3::NEG_Y, 2.0 * REACH) {
        player.pos.y = from.y - hit.distance;
    }
}
