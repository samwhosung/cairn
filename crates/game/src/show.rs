/// An animation of the install's `AnimationData.dbc`, by its id: what a game has a body show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Anim(pub u16);

/// The animations the games cairn carries show, by their `AnimationData.dbc` names.
pub mod anim {
    use super::Anim;

    pub const DEATH: Anim = Anim(1);
    /// The pose Death ends in: a character's model plays Death and holds its last frame.
    pub const DEAD: Anim = Anim(6);
    pub const COMBAT_WOUND: Anim = Anim(9);
    pub const ATTACK_UNARMED: Anim = Anim(16);
}

/// What a tick had the players' bodies show, by body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shows {
    /// Each animation played once, by body and then in the order the rules played them.
    pub played: Vec<(u32, Anim)>,
    /// Each pose that changed, by body: the one now held, or `None` once let go.
    pub held: Vec<(u32, Option<Anim>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Show {
    Play(Anim),
    Hold(Option<Anim>),
}

/// The poses the players' bodies hold, by body, and what this tick had them show.
#[derive(Default)]
pub(crate) struct Poses {
    held: Vec<Option<Anim>>,
    pub tick: Shows,
}

impl Poses {
    pub fn join(&mut self, n: u32) {
        let n = n as usize;
        if self.held.len() <= n {
            self.held.resize(n + 1, None);
        }
    }

    pub fn held(&self, n: u32) -> Option<Anim> {
        self.held.get(n as usize).copied().flatten()
    }

    pub fn all(&self) -> &[Option<Anim>] {
        &self.held
    }

    /// Takes a tick's shows, each `(body, phase, show)` in the order its rule made them.
    pub fn settle(&mut self, mut shows: Vec<(u32, u8, Show)>) {
        self.tick.played.clear();
        self.tick.held.clear();
        shows.sort_by_key(|&(n, phase, _)| (n, phase));
        let mut i = 0;
        while i < shows.len() {
            let n = shows[i].0;
            let was = self.held(n);
            let mut now = was;
            while let Some(&(m, _, show)) = shows.get(i)
                && m == n
            {
                match show {
                    Show::Play(anim) => self.tick.played.push((n, anim)),
                    Show::Hold(pose) => now = pose,
                }
                i += 1;
            }
            if now != was {
                self.join(n);
                self.held[n as usize] = now;
                self.tick.held.push((n, now));
            }
        }
    }
}
