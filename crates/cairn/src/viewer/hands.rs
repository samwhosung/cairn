mod ask;

use std::fmt::Write as _;
use std::time::Instant;

use bevy::asset::uuid::Uuid;
use bevy::camera::RenderTarget;
use bevy::ecs::system::SystemParam;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::{Location, PointerId, PointerLocation};
use bevy::prelude::*;
use world::collision::CollisionResidency;
use world::coords::{bevy_to_wow, facing_of, heading_of, wow_to_bevy};
use world::hands::{
    Ghost, GhostShown, OnScreen, OnTheWorld, PICKS_FRAMES_LATE, Pointed, Selection, SightPicking,
};
use world::sight::{Seen, Sight, Sighting};
use world::{Filed, Install, PlacementEdits, Placements, Residency, WorldCamera};

use ask::{GhostAsk, Model, Move, Spot, Turn};
pub use ask::{HandsAsk, parse};

use super::{Answers, GIVE_UP_AFTER, Step, Viewer};

pub const VIEWER_POINTER: PointerId =
    PointerId::Custom(Uuid::from_u128(0x6361_6972_6e5f_7669_6577_5f70_6f69_6e74));
pub const PICK_POINTER: PointerId =
    PointerId::Custom(Uuid::from_u128(0x6361_6972_6e5f_7669_6577_5f70_6963_6b21));
const SETTLED_FRAMES: u32 = 1 + PICKS_FRAMES_LATE;
const GROUND_CAST_TOP_YD: f32 = 5000.0;

pub(super) struct Handling {
    ask: HandsAsk,
    asked: Instant,
    started: bool,
    frames: u32,
    settled: u32,
    drawn_after: Option<(u32, f32)>,
    collides_after: Option<(u32, f32)>,
}

impl Handling {
    pub(super) fn new(ask: HandsAsk) -> Self {
        Self {
            ask,
            asked: Instant::now(),
            started: false,
            frames: 0,
            settled: 0,
            drawn_after: None,
            collides_after: None,
        }
    }
}

#[derive(SystemParam)]
pub(super) struct Hands<'w, 's> {
    pointers: Query<'w, 's, (&'static PointerId, &'static mut PointerLocation)>,
    camera: Query<'w, 's, &'static RenderTarget, With<WorldCamera>>,
    picking: ResMut<'w, SightPicking>,
    pointed: Res<'w, Pointed>,
    hover: Res<'w, HoverMap>,
    world: Query<'w, 's, Entity, With<OnTheWorld>>,
    sight: Sight<'w, 's>,
    placements: Res<'w, Placements>,
    edits: ResMut<'w, PlacementEdits>,
    selection: ResMut<'w, Selection>,
    ghost: ResMut<'w, Ghost>,
    shown: Res<'w, GhostShown>,
    on_screen: Res<'w, OnScreen>,
    install: Res<'w, Install>,
    residency: Res<'w, Residency>,
    collision: Res<'w, CollisionResidency>,
}

pub(super) fn handle(
    mut viewer: ResMut<'_, Viewer>,
    answers: Option<Res<'_, Answers>>,
    mut hands: Hands<'_, '_>,
) {
    let size = viewer.shot.size;
    let answer = {
        let Step::Handling(h) = &mut viewer.step else {
            return;
        };
        hands.step(h, size)
    };
    if let Some(answer) = answer {
        if let Some(answers) = &answers {
            answers.say(&answer);
        }
        viewer.step = Step::Idle;
    }
}

impl Hands<'_, '_> {
    fn step(&mut self, h: &mut Handling, size: UVec2) -> Option<String> {
        let answer = self.answer(h, size);
        if answer.is_some() {
            *self.picking = SightPicking::Models;
            self.put_pointer(PICK_POINTER, None);
        }
        answer
    }

    fn answer(&mut self, h: &mut Handling, size: UVec2) -> Option<String> {
        if !h.started {
            if let Err(e) = self.start(h, size) {
                return Some(format!("error: {e}"));
            }
            h.started = true;
            return None;
        }
        h.frames += 1;
        let drawn = self.residency.settled() && self.shown.0 == self.ghost.filed;
        let collides = self.collision.settled();
        let now = (h.frames, h.asked.elapsed().as_secs_f32());
        if drawn && h.drawn_after.is_none() {
            h.drawn_after = Some(now);
        }
        if collides && h.collides_after.is_none() {
            h.collides_after = Some(now);
        }
        let ready = drawn && collides;
        h.settled = if ready { h.settled + 1 } else { 0 };
        if h.settled >= SETTLED_FRAMES {
            return Some(self.finish(h));
        }
        (h.asked.elapsed() > GIVE_UP_AFTER).then(|| {
            format!(
                "error: the world did not settle in {} s",
                GIVE_UP_AFTER.as_secs()
            )
        })
    }

    fn start(&mut self, h: &Handling, size: UVec2) -> Result<(), String> {
        match &h.ask {
            HandsAsk::Pick { at, ground } => {
                let location = self.pixel(*at, size)?;
                self.put_pointer(PICK_POINTER, Some(location));
                if *ground {
                    *self.picking = SightPicking::ThroughToTheGround;
                }
            }
            HandsAsk::Pointer(at) => {
                let location = self.pixel(*at, size)?;
                self.put_pointer(VIEWER_POINTER, Some(location));
            }
            HandsAsk::Select(ids) => {
                if let Some(id) = ids.iter().find(|&&id| self.placements.get(id).is_none()) {
                    return Err(format!("no placement {id} stands in the world held"));
                }
                self.selection.0 = ids.iter().copied().collect();
            }
            HandsAsk::Ghost(None) => *self.ghost = Ghost::default(),
            HandsAsk::Ghost(Some(GhostAsk { model, at })) => {
                let filed = self.model(model, 0)?;
                let (rotation, scale) = (filed.rotation(), filed.scale());
                let (position, follows) = if let Some(spot) = at {
                    (self.place(*spot)?, None)
                } else {
                    let under = self
                        .pointed
                        .of(VIEWER_POINTER)
                        .and_then(|at| at.ground.as_ref())
                        .ok_or("the pointer is on no ground: pointer X Y puts it there")?;
                    (bevy_to_wow(under.point), Some(VIEWER_POINTER))
                };
                *self.ghost = Ghost {
                    filed: Some(filed.stood(position, rotation, scale)),
                    follows,
                };
            }
            HandsAsk::Add { id, model, at } => {
                if self.placements.get(*id).is_some() {
                    return Err(format!("{id} is placed already"));
                }
                let filed = self.model(model, *id)?;
                let (rotation, scale) = (filed.rotation(), filed.scale());
                let position = self.place(*at)?;
                self.edits.place(filed.stood(position, rotation, scale));
            }
            HandsAsk::Move(m) => {
                let stood = self.moved(m)?;
                self.edits.place(stood);
            }
            HandsAsk::Remove(ids) => {
                if let Some(id) = ids.iter().find(|&&id| self.placements.get(id).is_none()) {
                    return Err(format!("no placement {id} stands in the world held"));
                }
                for &id in ids {
                    self.edits.remove(id);
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self, h: &Handling) -> String {
        let ((frames, secs), (collision_frames, collision_secs)) = (
            h.drawn_after.unwrap_or_default(),
            h.collides_after.unwrap_or_default(),
        );
        let drawn = format!("; drawn after {frames} frames, {secs:.3} s");
        let changed = format!(
            "{drawn}, the collision after {collision_frames} frames, {collision_secs:.3} s"
        );
        match &h.ask {
            HandsAsk::Pick { .. } => {
                let met = self.world.single().ok().and_then(|world| {
                    self.pointed
                        .over_world(PICK_POINTER, &self.hover, world)
                        .and_then(|at| at.first.clone())
                });
                met.map_or_else(
                    || "ok nothing".to_owned(),
                    |m| format!("ok {}", met_text(&m)),
                )
            }
            HandsAsk::Pointer(at) => {
                let mut answer = format!("ok pointer {} {}", at.x, at.y);
                match self
                    .pointed
                    .of(VIEWER_POINTER)
                    .and_then(|p| p.ground.as_ref())
                {
                    Some(under) => {
                        let _ = write!(answer, " on the ground at {}", xyz(under.point));
                    }
                    None => answer += " on no ground",
                }
                if let Some(ghost) = &self.ghost.filed {
                    let on = on_frame(self.on_screen.ghost);
                    let _ = write!(answer, ", the ghost {}, {on}", filed_text(ghost));
                }
                answer
            }
            HandsAsk::Ghost(_) => match &self.ghost.filed {
                Some(ghost) => {
                    let on = on_frame(self.on_screen.ghost);
                    format!("ok ghost {}, {on}{drawn}", filed_text(ghost))
                }
                None => "ok no ghost".to_owned(),
            },
            HandsAsk::Add { id, .. } | HandsAsk::Move(Move { id, .. }) => {
                match self.placements.get(*id).and_then(|p| p.filed.as_ref()) {
                    Some(filed) => format!("ok placement {id} {}{changed}", filed_text(filed)),
                    None => format!("error: placement {id} stands nowhere"),
                }
            }
            HandsAsk::Remove(ids) => {
                let mut answer = "ok removed".to_owned();
                for id in ids {
                    let _ = write!(answer, " {id}");
                }
                answer + &changed
            }
            HandsAsk::Select(ids) => {
                let mut answer = "ok selected".to_owned();
                if ids.is_empty() {
                    answer += " nothing";
                }
                for id in &self.selection.0 {
                    let on = on_frame(self.on_screen.selected.get(id).copied());
                    match self.placements.get(*id).and_then(|p| p.filed.as_ref()) {
                        Some(filed) => {
                            let _ = write!(answer, "; {id} {}, {on}", filed_text(filed));
                        }
                        None => {
                            let _ = write!(answer, "; {id} {on}");
                        }
                    }
                }
                answer
            }
        }
    }

    fn pixel(&self, at: UVec2, size: UVec2) -> Result<Location, String> {
        if at.x >= size.x || at.y >= size.y {
            return Err(format!(
                "pixel {} {} is off the {}x{} frame",
                at.x, at.y, size.x, size.y
            ));
        }
        let target = self
            .camera
            .single()
            .ok()
            .and_then(|t| t.normalize(None))
            .ok_or("the viewer's camera draws to no frame")?;
        Ok(Location {
            target,
            position: at.as_vec2() + 0.5,
        })
    }

    fn put_pointer(&mut self, which: PointerId, location: Option<Location>) {
        for (id, mut pointer) in &mut self.pointers {
            if *id == which && pointer.location != location {
                pointer.location.clone_from(&location);
            }
        }
    }

    fn model(&self, model: &Model, id: u32) -> Result<Filed, String> {
        let filed = Filed::of(&model.file, id, model.set).ok_or_else(|| {
            format!(
                "{} is no model: a model ends .m2, a building .wmo",
                model.file
            )
        })?;
        let mut path = model.file.replace('/', "\\");
        if let Filed::Doodad(_) = filed
            && let Some(stem) = path.rsplit_once('.').map(|(stem, _)| stem.to_owned())
        {
            path = stem + ".m2";
        }
        if !self.install.0.contains(&path) {
            return Err(format!("the install has no {}", model.file));
        }
        let rotation = [0.0, heading_of(model.facing_deg), 0.0];
        let scale = held_scale(&filed, model.scale)?.unwrap_or(1.0);
        let position = filed.position();
        Ok(filed.stood(position, rotation, scale))
    }

    fn moved(&self, m: &Move) -> Result<Filed, String> {
        let filed = self
            .placements
            .get(m.id)
            .and_then(|p| p.filed.clone())
            .ok_or_else(|| format!("no placement {} stands in the world held", m.id))?;
        let mut position = match m.to {
            Some(spot) => self.place(spot)?,
            None => filed.position(),
        };
        if let Some(by) = m.by {
            position = (Vec3::from(position) + by).to_array();
        }
        let mut rotation = filed.rotation();
        match m.turn {
            Turn::Keep => {}
            Turn::Facing(f) => rotation[1] = heading_of(f),
            Turn::By(d) => rotation[1] += d,
        }
        let scale = held_scale(&filed, m.scale)?.unwrap_or(filed.scale());
        Ok(filed.stood(position, rotation, scale))
    }

    fn place(&self, spot: Spot) -> Result<[f32; 3], String> {
        match spot {
            Spot::At(at) => Ok(at.to_array()),
            Spot::Ground(at) => {
                let from = wow_to_bevy([at.x, at.y, GROUND_CAST_TOP_YD]);
                let under = self
                    .sight
                    .cast(from, Dir3::NEG_Y, 2.0 * GROUND_CAST_TOP_YD)
                    .ground()
                    .ok_or_else(|| {
                        format!("no ground the world holds lies under {},{}", at.x, at.y)
                    })?;
                Ok([at.x, at.y, bevy_to_wow(under.point)[2]])
            }
        }
    }
}

fn on_frame(at: Option<Rect>) -> String {
    at.map_or_else(
        || "off the frame".to_owned(),
        |r| {
            format!(
                "on the frame at {:.1},{:.1} to {:.1},{:.1}",
                r.min.x, r.min.y, r.max.x, r.max.y
            )
        },
    )
}

fn held_scale(filed: &Filed, scale: Option<f32>) -> Result<Option<f32>, String> {
    match (filed, scale) {
        (Filed::Building(_), Some(_)) => Err("a building's record holds no scale".into()),
        (_, Some(s)) if !Filed::holds_scale(s) => Err(format!(
            "a doodad's record holds a scale from 1/1024 to 65535/1024, not {s}"
        )),
        (_, scale) => Ok(scale),
    }
}

fn xyz(bevy: Vec3) -> String {
    let [x, y, z] = bevy_to_wow(bevy);
    format!("{x:.3},{y:.3},{z:.3}")
}

fn filed_text(f: &Filed) -> String {
    let [x, y, z] = f.position();
    let facing = facing_of(f.rotation()[1]);
    format!(
        "{} at {x},{y},{z} facing {facing} scale {}",
        f.model(),
        f.scale()
    )
}

fn met_text(met: &Sighting) -> String {
    let at = xyz(met.point);
    match &met.seen {
        Seen::Terrain { column, row } => format!(
            "ground at {at}, chunk {column},{row} of tile {},{}",
            met.tile.0, met.tile.1
        ),
        Seen::Doodad { file, unique_id } => format!("placement {unique_id} {file} at {at}"),
        Seen::Building {
            file,
            unique_id,
            group,
        } => format!("placement {unique_id} {file} group {group} at {at}"),
        Seen::Prop {
            file,
            building_file,
            building_unique_id,
            doodad,
        } => format!(
            "placement {building_unique_id} {building_file}, its doodad {doodad} {file}, at {at}"
        ),
    }
}
