/// An animation of the install's `AnimationData.dbc`, by its id: what a game has a body show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Anim(pub u16);

/// Animations by their `AnimationData.dbc` names.
pub mod anim {
    use super::Anim;

    pub const DEATH: Anim = Anim(1);
    /// The pose Death ends in: a character's model stands at Death's last frame.
    pub const DEAD: Anim = Anim(6);
    pub const COMBAT_WOUND: Anim = Anim(9);
    pub const ATTACK_UNARMED: Anim = Anim(16);
    pub const READY_UNARMED: Anim = Anim(25);
}

/// How an attack came out, in the terms the client shows and sounds one by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Outcome {
    Hit,
    Crit,
    /// A crushing blow: the one struck always gives its crushing cry, if its body has one.
    Crushing,
    Miss,
    Dodge,
    Parry,
    Block,
    /// A hit that landed and took nothing.
    Absorb,
    Immune,
}

/// What a tick had the players' bodies show, by body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shows {
    /// Each animation played once, by body and then in the order the rules played them.
    pub played: Vec<(u32, Anim)>,
    /// Each attack, by attacker and then in the order the rules made them: the player's body it
    /// attacked, if any, and how it came out.
    pub attacked: Vec<(u32, Option<u32>, Outcome)>,
    /// Each pose that changed, by body: the one now held, or `None` once let go.
    pub held: Vec<(u32, Option<Anim>)>,
    /// Each idle that changed, by body: the one it now idles in, or `None` once it stops.
    pub idled: Vec<(u32, Option<Anim>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Show {
    Play(Anim),
    Attack(Option<u32>, Outcome),
    Hold(Option<Anim>),
    Idle(Option<Anim>),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BodyShow {
    pub body: u32,
    pub phase: u8,
    pub show: Show,
}

#[derive(Default)]
pub(crate) struct Poses {
    held: Vec<Option<Anim>>,
    idling: Vec<Option<Anim>>,
    pub tick: Shows,
}

impl Poses {
    pub fn join(&mut self, n: u32) {
        let n = n as usize;
        if self.held.len() <= n {
            self.held.resize(n + 1, None);
            self.idling.resize(n + 1, None);
        }
    }

    pub fn held(&self, n: u32) -> Option<Anim> {
        self.held.get(n as usize).copied().flatten()
    }

    pub fn idling(&self, n: u32) -> Option<Anim> {
        self.idling.get(n as usize).copied().flatten()
    }

    pub fn all(&self) -> (&[Option<Anim>], &[Option<Anim>]) {
        (&self.held, &self.idling)
    }

    pub fn settle(&mut self, mut shows: Vec<BodyShow>) {
        self.tick.played.clear();
        self.tick.attacked.clear();
        self.tick.held.clear();
        self.tick.idled.clear();
        shows.sort_by_key(|s| (s.body, s.phase));
        let mut i = 0;
        while i < shows.len() {
            let n = shows[i].body;
            let (was_held, was_idling) = (self.held(n), self.idling(n));
            let (mut held, mut idling) = (was_held, was_idling);
            while let Some(&BodyShow { body, show, .. }) = shows.get(i)
                && body == n
            {
                match show {
                    Show::Play(anim) => self.tick.played.push((n, anim)),
                    Show::Attack(target, outcome) => self.tick.attacked.push((n, target, outcome)),
                    Show::Hold(pose) => held = pose,
                    Show::Idle(anim) => idling = anim,
                }
                i += 1;
            }
            if (held, idling) != (was_held, was_idling) {
                self.join(n);
            }
            if held != was_held {
                self.held[n as usize] = held;
                self.tick.held.push((n, held));
            }
            if idling != was_idling {
                self.idling[n as usize] = idling;
                self.tick.idled.push((n, idling));
            }
        }
    }
}
