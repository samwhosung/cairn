use bevy::ecs::entity::EntityHashMap;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use world::M2Model;
use world::rig::RigPose;
use world::rig_events::AnimEvent;
use world::unit::{BodyModel, Outcome, UnitAttack, UnitBody, UnitShow};

use crate::config::SoundConfig;
use crate::kit::{Bus, KitRef, PlayExtras, SoundCategory, SoundKits, play_kit_ext};
use crate::liquid_loop::Listening;
use crate::tables::{CreatureVoices, Voice, WeaponSounds, impact_slot};
use crate::{AudioListener, SoundOutput};

const DEATH: u16 = 1;

const COMBAT_MISS_1H: u32 = 7080;
const ABSORB_GET_HIT: u32 = 3334;

/// Every body swings bare-handed, and a fist is a light weapon.
const FIST: u32 = 13;
const LIGHT: usize = 0;

const MISS_ATTACHMENT: u16 = 1;
const STUB_LIFT_YD: f32 = 2.0;

const EXERTION_CHANCE_CREATURE: u32 = 70;
const EXERTION_CHANCE_PLAYER: u32 = 35;
const INJURY_CHANCE_CREATURE: u32 = 60;
const INJURY_CHANCE_PLAYER: u32 = 30;

/// The client's roll of a chance out of 100, on a 32-bit draw: `threshold + 1` in 101 pass.
fn chance_passes(threshold: u32, roll: u32) -> bool {
    ((101 * u64::from(roll)) >> 32) as u32 <= threshold
}

fn grunts(outcome: Outcome) -> bool {
    outcome != Outcome::Miss
}

fn whiffs(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Miss | Outcome::Dodge)
}

fn makes_contact(outcome: Outcome) -> bool {
    !matches!(outcome, Outcome::Miss | Outcome::Dodge | Outcome::Immune)
}

fn defended(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Parry | Outcome::Block)
}

fn absorbed(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Absorb | Outcome::Immune)
}

fn wounds(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::Hit | Outcome::Crit | Outcome::Crushing)
}

/// A creature's `CreatureSoundData` impact type, as the client maps it to a weapon row's slot; a
/// type past 3 strikes flesh.
fn creature_impact_slot(impact_type: u32) -> usize {
    match impact_type {
        1 => impact_slot::STONE,
        2 => impact_slot::WOOD,
        3 => impact_slot::ETHEREAL,
        _ => impact_slot::FLESH,
    }
}

/// The keys that land a blow: a character's, whose weapon sounds it, and a creature's, whose
/// voice's `CustomAttack` column does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Blow {
    Weapon,
    Natural(usize),
}

fn blow_key(ident: [u8; 4]) -> Option<Blow> {
    match &ident {
        b"$CAH" => Some(Blow::Weapon),
        b"$AH0" => Some(Blow::Natural(0)),
        b"$AH1" => Some(Blow::Natural(1)),
        b"$AH2" => Some(Blow::Natural(2)),
        b"$AH3" => Some(Blow::Natural(3)),
        _ => None,
    }
}

pub(crate) fn load_weapons(
    mut commands: Commands<'_, '_>,
    install: Option<Res<'_, world::Install>>,
) {
    let Some(install) = install else { return };
    match WeaponSounds::load(&install.0) {
        Ok(weapons) => commands.insert_resource(weapons),
        Err(e) => warn!("sound: no weapon sounds, so fights sound only their voices: {e}"),
    }
}

type Body = (
    &'static GlobalTransform,
    Option<&'static UnitBody>,
    Has<Listening>,
    Option<&'static BodyModel>,
    Option<&'static RigPose>,
);

#[derive(SystemParam)]
pub(crate) struct Fighters<'w, 's> {
    bodies: Query<'w, 's, Body>,
    joints: Query<'w, 's, &'static GlobalTransform>,
    models: Option<Res<'w, Assets<M2Model>>>,
    voices: Option<Res<'w, CreatureVoices>>,
}

impl Fighters<'_, '_> {
    fn at(&self, body: Entity) -> Option<Vec3> {
        Some(self.bodies.get(body).ok()?.0.translation())
    }

    fn voice(&self, body: Entity) -> Option<&Voice> {
        let display = self.bodies.get(body).ok()?.1?.display;
        self.voices.as_deref()?.voice(display)
    }

    fn is_player(&self, body: Entity) -> bool {
        self.bodies
            .get(body)
            .is_ok_and(|b| b.1.is_some_and(|u| u.character.is_some()))
    }

    fn is_listening(&self, body: Entity) -> bool {
        self.bodies.get(body).is_ok_and(|b| b.2)
    }

    fn attachment(&self, body: Entity, id: u16) -> Option<Vec3> {
        let (_, _, _, model, pose) = self.bodies.get(body).ok()?;
        let (model, pose) = (model?, pose?);
        let point = self
            .models
            .as_deref()?
            .get(&model.0)?
            .attachments
            .iter()
            .find(|a| a.id == id)?;
        pose.posed_point(
            self.joints.get(pose.joints_root).ok()?,
            point.bone,
            point.offset,
        )
    }
}

struct Sounding<'a> {
    kits: &'a mut SoundKits,
    out: &'a mut SoundOutput,
    config: &'a SoundConfig,
    listener: Vec3,
}

impl Sounding<'_> {
    fn play(&mut self, kit: u32, at: Vec3, bus: Bus, what: &str) {
        if kit == 0 {
            return;
        }
        let extras = PlayExtras {
            bus,
            ..PlayExtras::default()
        };
        if let Err(e) = play_kit_ext(
            self.kits,
            self.out,
            self.config,
            self.listener,
            KitRef::Id(kit),
            Some(at),
            SoundCategory::Sfx,
            extras,
        ) {
            warn!("{what} (kit {kit}): {e:#}");
        }
    }

    fn passes(&mut self, threshold: u32) -> bool {
        chance_passes(threshold, self.kits.roll())
    }
}

/// The client keeps one attack per attacker: a swing's `$CSS` key whooshes by it, the key that
/// lands the blow sounds it and lets it go, and a newer attack takes its place before either.
#[allow(clippy::too_many_arguments)]
pub(crate) fn combat_sounds(
    mut attacks: MessageReader<'_, '_, UnitAttack>,
    mut keys: MessageReader<'_, '_, AnimEvent>,
    mut pending: Local<'_, EntityHashMap<UnitAttack>>,
    fighters: Fighters<'_, '_>,
    weapons: Option<Res<'_, WeaponSounds>>,
    kits: Option<ResMut<'_, SoundKits>>,
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    listener: Res<'_, AudioListener>,
) {
    let (Some(mut kits), Some(weapons)) = (kits, weapons) else {
        attacks.clear();
        keys.clear();
        return;
    };
    let mut sound = Sounding {
        kits: &mut kits,
        out: &mut out,
        config: &config,
        listener: listener.pos,
    };
    for &attack in attacks.read() {
        pending.insert(attack.attacker, attack);
        if grunts(attack.outcome) {
            exertion(&mut sound, &fighters, attack);
        }
    }
    for key in keys.read() {
        if key.ident == *b"$CSS" {
            if let Some(&attack) = pending.get(&key.entity) {
                whoosh(&mut sound, &fighters, &weapons, attack, key.pos);
            }
        } else if let Some(blow) = blow_key(key.ident)
            && let Some(attack) = pending.remove(&key.entity)
        {
            contact(&mut sound, &fighters, &weapons, attack, (key.pos, blow));
        }
    }
    if pending.len() > 128 {
        pending.retain(|attacker, _| fighters.bodies.contains(*attacker));
    }
}

fn exertion(sound: &mut Sounding<'_>, fighters: &Fighters<'_, '_>, attack: UnitAttack) {
    let Some(at) = fighters.at(attack.attacker) else {
        return;
    };
    let critical = attack.outcome == Outcome::Crit;
    let chance = if fighters.is_player(attack.attacker) {
        EXERTION_CHANCE_PLAYER
    } else {
        EXERTION_CHANCE_CREATURE
    };
    if !critical && !sound.passes(chance) {
        return;
    }
    let kit = fighters
        .voice(attack.attacker)
        .map_or(0, |v| v.exertion[usize::from(critical)]);
    sound.play(kit, at, Bus::EXERTION, "exertion");
}

fn whoosh(
    sound: &mut Sounding<'_>,
    fighters: &Fighters<'_, '_>,
    weapons: &WeaponSounds,
    attack: UnitAttack,
    key_at: Vec3,
) {
    if whiffs(attack.outcome) {
        let at = fighters
            .attachment(attack.attacker, MISS_ATTACHMENT)
            .or_else(|| fighters.at(attack.attacker));
        if let Some(at) = at {
            sound.play(COMBAT_MISS_1H, at, Bus::DEFAULT, "miss whoosh");
        }
        return;
    }
    if let Some(kit) = weapons.swing(LIGHT, attack.outcome == Outcome::Crit) {
        sound.play(kit, key_at, Bus::WEAPON_SWING, "swing");
    }
}

fn contact(
    sound: &mut Sounding<'_>,
    fighters: &Fighters<'_, '_>,
    weapons: &WeaponSounds,
    attack: UnitAttack,
    (key_at, blow): (Vec3, Blow),
) {
    let (outcome, victim) = (attack.outcome, attack.target);
    let critical = outcome == Outcome::Crit;
    if makes_contact(outcome) {
        if let Blow::Natural(n) = blow {
            let kit = fighters
                .voice(attack.attacker)
                .map_or(0, |v| v.custom_attack[n]);
            sound.play(kit, key_at, Bus::MELEE_IMPACT, "natural impact");
        } else if !defended(outcome)
            && let Some(row) = weapons.impact(FIST, false)
        {
            let slot = match victim {
                Some(v) if !fighters.is_player(v) => {
                    creature_impact_slot(fighters.voice(v).map_or(0, |v| v.impact_type))
                }
                _ => impact_slot::FLESH,
            };
            let kit = if critical {
                row.critical[slot]
            } else {
                row.normal[slot]
            };
            sound.play(kit, key_at, Bus::MELEE_IMPACT, "impact");
        }
    }
    let Some((victim, at)) = victim.and_then(|v| Some((v, fighters.at(v)?))) else {
        return;
    };
    if absorbed(outcome) {
        let lifted = at + Vec3::Y * STUB_LIFT_YD;
        sound.play(ABSORB_GET_HIT, lifted, Bus::DEFAULT, "absorb");
    }
    if wounds(outcome) {
        injury(sound, fighters, outcome, victim, at);
    }
}

fn injury(
    sound: &mut Sounding<'_>,
    fighters: &Fighters<'_, '_>,
    outcome: Outcome,
    victim: Entity,
    at: Vec3,
) {
    let class = match outcome {
        Outcome::Crushing => 2,
        Outcome::Crit => 1,
        _ => 0,
    };
    let chance = if fighters.is_player(victim) {
        INJURY_CHANCE_PLAYER
    } else {
        INJURY_CHANCE_CREATURE
    };
    if class == 0 && !sound.passes(chance) {
        return;
    }
    let kit = fighters.voice(victim).map_or(0, |v| v.injury[class]);
    let bus = if fighters.is_listening(victim) {
        Bus::SELF_INJURY
    } else {
        Bus::INJURY
    };
    sound.play(kit, at, bus, "injury");
}

/// Reads the play before the body's driver takes it.
#[allow(clippy::type_complexity)]
pub(crate) fn death_cries(
    dying: Query<'_, '_, (&UnitShow, &UnitBody, &GlobalTransform), Changed<UnitShow>>,
    voices: Option<Res<'_, CreatureVoices>>,
    kits: Option<ResMut<'_, SoundKits>>,
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    listener: Res<'_, AudioListener>,
) {
    let (Some(voices), Some(mut kits)) = (voices, kits) else {
        return;
    };
    let mut sound = Sounding {
        kits: &mut kits,
        out: &mut out,
        config: &config,
        listener: listener.pos,
    };
    for (show, body, at) in &dying {
        if show.play == Some(DEATH) {
            let kit = voices.voice(body.display).map_or(0, |v| v.death);
            sound.play(kit, at.translation(), Bus::DEFAULT, "death cry");
        }
    }
}

#[cfg(test)]
mod tests;
