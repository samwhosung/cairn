use std::path::PathBuf;
use std::sync::Arc;

use world::unit::CharacterDress;

use super::super::*;
use crate::log::PlayLog;
use crate::mixer::{Mixer, MixerSettings};
use crate::output::{OFFLINE_SAMPLE_RATE, Output};
use crate::tables::KitCatalog;

const STEP_SECS: f64 = 1.0 / 60.0;
/// The frames from the telling on which the human's swing crosses its `$CSS` and its `$CAH`, as
/// the world fires them for its first variation.
const WHOOSH_FRAME: usize = 14;
const BLOW_FRAME: usize = 18;
const HUMAN_MALE: u32 = 49;
const ORC_MALE: u32 = 51;
const ATTACKER_AT: Vec3 = Vec3::new(0.0, 0.0, 0.0);
const VICTIM_AT: Vec3 = Vec3::new(1.5, 0.0, 0.0);
const KEY_AT: Vec3 = Vec3::new(0.8, 1.3, 0.0);

struct Fight {
    app: App,
    attacker: Entity,
    victim: Entity,
    log: PathBuf,
    frame: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Heard {
    frame: usize,
    kit: u32,
    at: Vec3,
}

fn body(display: u32, at: Vec3) -> (UnitBody, UnitShow, Transform, GlobalTransform) {
    let unit = UnitBody {
        display,
        model: String::new(),
        skins: [None, None, None],
        character: Some(CharacterDress::default()),
    };
    let placed = Transform::from_translation(at);
    (
        unit,
        UnitShow::default(),
        placed,
        GlobalTransform::from(placed),
    )
}

impl Fight {
    fn new(name: &str) -> Option<Self> {
        let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
            eprintln!("skipped: WOW_DATA is not set");
            return None;
        };
        let chain = Arc::new(mpq::Chain::open(data).expect("the install"));
        let log = std::env::temp_dir().join(format!(
            "cairn-sound-fight-{name}-{}.jsonl",
            std::process::id()
        ));
        let offline = MixerSettings {
            output: Output::Offline {
                sample_rate: OFFLINE_SAMPLE_RATE,
            },
            mix_tap: None,
        };
        let out = SoundOutput {
            mixer: Some(Mixer::new(&offline).expect("an offline mixer")),
            channels: Vec::new(),
            zone_streams: 0,
            voices_stolen: 0,
            voices_denied: 0,
            copies_dropped: 0,
            log: PlayLog::create(&log),
            play_time: 0.0,
            offline: None,
        };
        let mut app = App::new();
        app.add_message::<UnitAttack>()
            .add_message::<AnimEvent>()
            .insert_resource(SoundKits::new(
                KitCatalog::load(&chain).expect("the kits"),
                chain.clone(),
            ))
            .insert_resource(CreatureVoices::load(&chain).expect("the voices"))
            .insert_resource(WeaponSounds::load(&chain).expect("the weapons"))
            .init_resource::<SoundConfig>()
            .insert_resource(AudioListener {
                pos: (ATTACKER_AT + VICTIM_AT) / 2.0,
                rot: Quat::IDENTITY,
            })
            .insert_non_send_resource(out)
            .add_systems(Update, (death_cries, combat_sounds).chain());
        let attacker = app.world_mut().spawn(body(HUMAN_MALE, ATTACKER_AT)).id();
        let victim = app.world_mut().spawn(body(ORC_MALE, VICTIM_AT)).id();
        Some(Self {
            app,
            attacker,
            victim,
            log,
            frame: 0,
        })
    }

    fn update(&mut self) {
        let t = self.frame as f64 * STEP_SECS;
        self.app
            .world_mut()
            .non_send_resource_mut::<SoundOutput>()
            .play_time = t;
        self.app.update();
        self.frame += 1;
    }

    fn key(&mut self, ident: [u8; 4]) {
        self.app.world_mut().write_message(AnimEvent {
            entity: self.attacker,
            ident,
            data: 0,
            anim_id: 16,
            pos: KEY_AT,
        });
    }

    /// One swing, from the telling through its blow, and what it sounded, by frame from the
    /// telling. `None` swings with no attack told.
    fn swing(&mut self, told: Option<Outcome>) -> Vec<Heard> {
        let (start, before) = (self.frame, self.heard().len());
        if let Some(outcome) = told {
            let attack = UnitAttack {
                attacker: self.attacker,
                target: Some(self.victim),
                outcome,
            };
            self.app.world_mut().write_message(attack);
        }
        for f in 0..=BLOW_FRAME + 2 {
            match f {
                WHOOSH_FRAME => self.key(*b"$CSS"),
                BLOW_FRAME => {
                    self.key(*b"$HIT");
                    self.key(*b"$CAH");
                }
                _ => {}
            }
            self.update();
        }
        self.app
            .world_mut()
            .non_send_resource_mut::<SoundOutput>()
            .channels
            .clear();
        let mut heard = self.heard().split_off(before);
        for h in &mut heard {
            h.frame -= start;
        }
        heard
    }

    fn heard(&self) -> Vec<Heard> {
        let text = std::fs::read_to_string(&self.log).expect("the play log");
        text.lines()
            .map(|line| {
                let field = |name: &str| {
                    let at = line.find(&format!("\"{name}\":")).expect("a field") + name.len() + 3;
                    let rest = &line[at..];
                    rest[..rest.find([',', '}']).expect("its end")].to_owned()
                };
                let pos = &line[line.find("\"pos\":[").expect("a position") + 7..];
                let pos: Vec<f32> = pos[..pos.find(']').expect("its end")]
                    .split(',')
                    .map(|v| v.parse().expect("a coordinate"))
                    .collect();
                let t: f64 = field("t").parse().expect("a time");
                Heard {
                    frame: (t / STEP_SECS).round() as usize,
                    kit: field("kit").parse().expect("a kit"),
                    at: Vec3::new(pos[0], pos[1], pos[2]),
                }
            })
            .collect()
    }
}

impl Drop for Fight {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.log);
    }
}

/// A kit the swing sounds for certain, or only when the client's roll passes.
#[derive(Clone, Copy, Debug)]
enum Kit {
    Sure(u32),
    Rolled(u32),
}

/// What benilla's order has the human's swing at the orc sound for `outcome`: at the telling,
/// at `$CSS`, at `$CAH`, and where.
fn benillas_order(outcome: Outcome) -> Vec<(usize, Kit, Vec3)> {
    use Outcome::{Absorb, Block, Crit, Crushing, Dodge, Hit, Immune, Miss, Parry};
    let (human, orc) = ((2941, 186), (1320, 1321));
    let mut order = Vec::new();
    match outcome {
        Miss => {}
        Crit => order.push((0, Kit::Sure(human.1), ATTACKER_AT)),
        _ => order.push((0, Kit::Rolled(human.0), ATTACKER_AT)),
    }
    order.push(match outcome {
        Miss | Dodge => (WHOOSH_FRAME, Kit::Sure(7080), ATTACKER_AT),
        Crit => (WHOOSH_FRAME, Kit::Sure(234), KEY_AT),
        _ => (WHOOSH_FRAME, Kit::Sure(233), KEY_AT),
    });
    if matches!(outcome, Hit | Crit | Crushing | Absorb) {
        order.push((BLOW_FRAME, Kit::Sure(1014), KEY_AT));
    }
    if matches!(outcome, Absorb | Immune) {
        order.push((BLOW_FRAME, Kit::Sure(3334), VICTIM_AT + Vec3::Y * 2.0));
    }
    match outcome {
        Hit => order.push((BLOW_FRAME, Kit::Rolled(orc.0), VICTIM_AT)),
        Crit => order.push((BLOW_FRAME, Kit::Sure(orc.1), VICTIM_AT)),
        Crushing | Miss | Dodge | Parry | Block | Absorb | Immune => {}
    }
    order
}

/// `Ok` with how many rolled kits played, or what was heard apart from the order.
fn heard_as(outcome: Outcome, heard: &[Heard]) -> Result<usize, String> {
    let order = benillas_order(outcome);
    let mut rolled = 0;
    for h in heard {
        let wanted = order.iter().find(|&&(frame, kit, at)| {
            let (Kit::Sure(k) | Kit::Rolled(k)) = kit;
            frame == h.frame && k == h.kit && at.distance(h.at) < 1e-3
        });
        match wanted {
            Some((_, Kit::Rolled(_), _)) => rolled += 1,
            Some(_) => {}
            None => return Err(format!("{outcome:?}: {h:?} is not in the order {order:?}")),
        }
    }
    for &(frame, kit, _) in &order {
        if let Kit::Sure(k) = kit
            && !heard.iter().any(|h| h.frame == frame && h.kit == k)
        {
            return Err(format!(
                "{outcome:?}: kit {k} never played at frame {frame}"
            ));
        }
    }
    Ok(rolled)
}

#[test]
fn every_outcome_sounds_in_the_clients_order_at_the_telling_the_whoosh_and_the_blow() {
    let Some(mut fight) = Fight::new("outcomes") else {
        return;
    };
    let every = [
        Outcome::Hit,
        Outcome::Crit,
        Outcome::Crushing,
        Outcome::Miss,
        Outcome::Dodge,
        Outcome::Parry,
        Outcome::Block,
        Outcome::Absorb,
        Outcome::Immune,
    ];
    let mut rolled = [0usize; 9];
    let swings = 20;
    for _ in 0..swings {
        for (i, outcome) in every.into_iter().enumerate() {
            let heard = fight.swing(Some(outcome));
            rolled[i] += heard_as(outcome, &heard).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    let hit = fight.swing(Some(Outcome::Hit));
    let dropped = fight.swing(None);
    let told_wrong = fight.swing(Some(Outcome::Miss));
    assert!(heard_as(Outcome::Hit, &hit).is_ok());
    assert_eq!(
        dropped,
        [],
        "no attack told, the swing's keys sound nothing"
    );
    let wrong = heard_as(Outcome::Hit, &told_wrong).expect_err("a miss told for a hit");
    assert!(wrong.contains("7080"), "{wrong}");
    let (hit_rolls, parry_rolls) = (rolled[0], rolled[5]);
    assert!(
        (1..2 * swings).contains(&hit_rolls) && (1..swings).contains(&parry_rolls),
        "the rolled grunts and cries sound some swings, not all: {rolled:?}"
    );
    eprintln!(
        "{swings} swings of each outcome held benilla's order; rolled grunts and cries played \
         {rolled:?} times; the swing with no attack told sounded {dropped:?}; a miss told for a \
         hit: {wrong}"
    );
}

#[test]
fn a_body_told_to_die_cries_its_death_and_one_told_a_wound_does_not() {
    let Some(mut fight) = Fight::new("death") else {
        return;
    };
    let tell = |fight: &mut Fight, play| {
        let mut e = fight.app.world_mut().entity_mut(fight.victim);
        e.get_mut::<UnitShow>().expect("a show").play = Some(play);
    };
    tell(&mut fight, 9);
    fight.update();
    tell(&mut fight, DEATH);
    fight.update();
    fight.update();
    let heard = fight.heard();
    assert_eq!(
        heard,
        [Heard {
            frame: 1,
            kit: 1322,
            at: VICTIM_AT
        }],
        "the orc's cry, where it dies, once"
    );
}

#[test]
fn a_whoosh_needs_an_attack_that_no_blow_has_taken() {
    let Some(mut fight) = Fight::new("taken") else {
        return;
    };
    fight.swing(Some(Outcome::Hit));
    let before = fight.heard().len();
    fight.key(*b"$CSS");
    fight.update();
    fight.key(*b"$CAH");
    fight.update();
    assert_eq!(
        fight.heard().len(),
        before,
        "a ready stance's keys, after the blow, sound nothing"
    );
}
