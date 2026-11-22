//! One result or saved set. Same computations as the preview's model.Result (decoration placement, tree totals, total defense and resistances).
use crate::db::Db;
use alloc::{collections::BTreeMap, string::String, vec::Vec};
use core::fmt::Write;
use skillsim_engine::types::*;

#[derive(Clone, Default)]
pub struct Set {
    pub free: u8,
    pub bonus: u8,
    pub def: u16,
    /// Owned charm index, -1 if none
    pub charm: i32,
    /// Armor ids per part. The first is the one shown (the rest are the same group).
    pub parts: [Vec<u16>; PARTS],
    /// (decoration id, location): LOC_ANY, LOC_TORSO (torso-up multiplied slot) or LOC_CHARM (charm slot doubled by talisman boost)
    pub jewels: Vec<(u16, u8)>,
    /// Charm of a saved set or unowned-charm result (instead of an owned charm index)
    pub saved_charm: Option<Charm>,
    pub weapon: u8,
    /// Target skill ids at search time
    pub targets: Vec<u16>,
    /// Set found with unowned charms
    pub needed: bool,
    /// Estimated chance of getting that charm or better in one appraisal
    pub chance: f64,
    /// Rarity of that charm (icon color)
    pub charm_rare: u8,
    /// Recommendation tags: 1 ready to wear, 2 most slots, 4 highest defense (not saved)
    pub tags: u8,
}

pub const CHARM: usize = 6;
/// Decoration locations (Set::jewels; saved as the number after ':')
pub const LOC_ANY: u8 = 0;
pub const LOC_TORSO: u8 = 1;
/// In the charm's slots: x2 while talisman boost is active (the game doubles charm decorations too, negatives included)
pub const LOC_CHARM: u8 = 2;
pub const WEAPON: usize = 5;

impl Set {
    pub fn head(&self, p: usize) -> u16 {
        self.parts[p].first().copied().unwrap_or(0)
    }

    pub fn charm_data(&self, db: &Db) -> Option<Charm> {
        if self.saved_charm.is_some() {
            return self.saved_charm;
        }
        usize::try_from(self.charm).ok().and_then(|i| db.inv().charms.get(i).copied())
    }

    /// Parts to make at the smithy: not owned, and not reward armor (that one is obtained another way)
    pub fn craft(&self, db: &Db) -> usize {
        (0..PARTS).filter(|&p| !db.inv().owned.contains(&self.head(p)) && !db.reward_armor.contains(&self.head(p))).count()
    }

    /// Highest rarity among armor to craft
    pub fn craft_rare(&self, db: &Db) -> u8 {
        (0..PARTS)
            .filter(|&p| !db.inv().owned.contains(&self.head(p)))
            .filter_map(|p| db.armor(self.head(p)).map(|a| a.rare))
            .max()
            .unwrap_or(0)
    }

    /// Are armor, charm, decorations and weapon slots all equal (duplicate check when saving; target skills are not compared)
    pub fn same_gear(&self, other: &Set, db: &Db) -> bool {
        let mut a = self.jewels.clone();
        let mut b = other.jewels.clone();
        a.sort();
        b.sort();
        (0..PARTS).all(|p| self.head(p) == other.head(p))
            && self.charm_data(db) == other.charm_data(db)
            && a == b
            && self.weapon == other.weapon
    }

    fn copies(armor: &[Option<&Armor>; PARTS]) -> i16 {
        (0..PARTS).filter(|&p| p != TORSO && armor[p].is_some_and(Db::is_torso_up)).count() as i16
    }

    fn armor<'a>(&self, db: &'a Db) -> [Option<&'static Armor>; PARTS] {
        core::array::from_fn(|p| db.armor(self.head(p)))
    }

    /// Number of torso-up parts
    pub fn torso_copies(&self, db: &Db) -> i16 {
        Self::copies(&self.armor(db))
    }

    /// Place decorations into real slots: containers 0-4 armor, 5 weapon, 6 charm. Torso-up targets go into the torso,
    /// the rest fill the smallest remaining fitting slot first, leaving the large slots.
    pub fn placement(&self, db: &Db) -> ([Vec<u16>; CONTAINERS], [u8; CONTAINERS]) {
        let armor = self.armor(db);
        let charm = self.charm_data(db);
        let mut caps = [0u8; CONTAINERS];
        for p in 0..PARTS {
            caps[p] = armor[p].map_or(0, |a| if Db::is_torso_up(a) { 0 } else { a.slots });
        }
        caps[WEAPON] = self.weapon;
        caps[CHARM] = charm.map_or(0, |c| c.slots);
        let copies = Self::copies(&armor);
        // Under talisman boost the charm's slots double their decorations: only the decorations meant for them go there
        let boost = self.talisman(db);
        let mut placed: [Vec<u16>; CONTAINERS] = Default::default();
        let mut left = caps;
        let mut jewels = self.jewels.clone();
        jewels.sort_by_key(|(id, _)| core::cmp::Reverse(db.jewel(*id).map_or(0, |j| j.size)));
        for (id, loc) in jewels {
            let size = db.jewel(id).map_or(0, |j| j.size);
            let choice = if loc == LOC_TORSO && copies > 0 {
                TORSO
            } else if loc == LOC_CHARM && left[CHARM] >= size {
                CHARM
            } else {
                match (0..CONTAINERS)
                    .filter(|&c| (c != TORSO || copies == 0) && !(boost && c == CHARM) && left[c] >= size)
                    .min_by_key(|&c| left[c])
                {
                    Some(c) => c,
                    None => continue,
                }
            };
            left[choice] = left[choice].saturating_sub(size);
            placed[choice].push(id);
        }
        (placed, caps)
    }

    /// Free slot containers left after placing the decorations: sizes 1-3, largest first (e.g. [3] or [1, 1, 1]).
    pub fn free_slots(&self, db: &Db) -> Vec<u8> {
        let (placed, caps) = self.placement(db);
        let mut out: Vec<u8> = (0..CONTAINERS)
            .map(|c| caps[c].saturating_sub(placed[c].iter().map(|&j| db.jewel(j).map_or(0, |j| j.size)).sum()))
            .filter(|&s| s > 0)
            .collect();
        out.sort_unstable_by(|a, b| b.cmp(a));
        out
    }

    /// Could the free slots activate one more skill level (a new skill or the next level of one)?
    /// Optimistic: decoration side effects are ignored, so 'false' means the free slots are certainly of no use.
    /// use_: the decorations the search could use (rank, smithy list).
    pub fn free_slots_useful(&self, db: &Db, use_: &crate::db::JewelUse) -> bool {
        // (capacity, multiplier): decorations in the torso count once more per torso-up part
        let (placed, caps) = self.placement(db);
        let copies = self.torso_copies(db);
        let boost = self.talisman(db);
        let mult = |c: usize| if c == TORSO { 1 + copies } else if c == CHARM && boost { 2 } else { 1 };
        let free: Vec<(u8, i16)> = (0..CONTAINERS)
            .map(|c| (caps[c].saturating_sub(placed[c].iter().map(|&j| db.jewel(j).map_or(0, |j| j.size)).sum()), mult(c)))
            .filter(|x| x.0 > 0)
            .collect();
        if free.is_empty() {
            return false;
        }
        let totals = self.totals(db);
        // Per tree: the most points that fit in a container of capacity 1, 2, 3 (several decorations allowed)
        let mut best: BTreeMap<u16, [i16; 4]> = BTreeMap::new();
        for j in db.gd.jewels.iter().filter(|j| use_.allows(j) && j.pos.pt > 0) {
            let e = best.entry(j.pos.tree).or_insert([0; 4]);
            for cap in 1..=3usize {
                let n = cap / j.size.max(1) as usize;
                e[cap] = e[cap].max(j.pos.pt as i16 * n as i16);
            }
        }
        for row in best.values_mut() {
            // mix sizes: 3 = 2 + 1, 2 = 1 + 1
            row[2] = row[2].max(row[1] * 2);
            row[3] = row[3].max(row[2] + row[1]).max(row[1] * 3);
        }
        for (&tree, row) in &best {
            let add: i16 = free.iter().map(|&(c, m)| row[c.min(3) as usize] * m).sum();
            let cur = totals.get(&tree).copied().unwrap_or(0);
            if db.tree_skills(tree).any(|s| s.threshold > 1 && cur < s.threshold as i16 && cur + add >= s.threshold as i16) {
                return true;
            }
        }
        false
    }

    /// Set totals: (defense, 5 resistances)
    pub fn sums(&self, db: &Db) -> (u16, [i16; RES]) {
        let mut def = 0;
        let mut res = [0i16; RES];
        for a in self.armor(db).into_iter().flatten() {
            def += a.def;
            for e in 0..RES {
                res[e] += a.res[e] as i16;
            }
        }
        (def, res)
    }

    /// Is talisman boost active (talisman points, with the secret skill's +2, reach 10). Decorations never raise these trees.
    pub fn talisman(&self, db: &Db) -> bool {
        self.base_totals(db).1
    }

    /// Points before talisman doubling and skill points +2, and whether talisman boost is active
    fn base_totals(&self, db: &Db) -> (BTreeMap<u16, i16>, bool) {
        let armor = self.armor(db);
        let copies = Self::copies(&armor);
        let mut totals: BTreeMap<u16, i16> = BTreeMap::new();
        let mut add = |tree: u16, pt: i16| {
            if tree != 0 {
                *totals.entry(tree).or_default() += pt;
            }
        };
        for (p, a) in armor.iter().enumerate() {
            let Some(a) = a else { continue };
            if Db::is_torso_up(a) {
                continue;
            }
            for s in a.skills {
                add(s.tree, s.pt as i16 * if p == TORSO { 1 + copies } else { 1 });
            }
        }
        for &(id, loc) in &self.jewels {
            if let Some(j) = db.jewel(id) {
                let m = if loc == LOC_TORSO { 1 + copies } else { 1 };
                add(j.pos.tree, j.pos.pt as i16 * m);
                add(j.neg.tree, j.neg.pt as i16 * m);
            }
        }
        let charm = self.charm_data(db);
        for s in charm.map(|c| c.skills).into_iter().flatten() {
            add(s.tree, s.pt as i16);
        }
        let secret = totals.get(&TREE_SECRET).copied().unwrap_or(0) >= 10;
        let tal = totals.get(&TREE_TALISMAN).copied().unwrap_or(0);
        let boost = tal + if secret && tal != 0 { 2 } else { 0 } >= 10;
        (totals, boost)
    }

    /// Compute tree totals in the same order as the rule engine (rules.rs).
    pub fn totals(&self, db: &Db) -> BTreeMap<u16, i16> {
        let (mut totals, boost) = self.base_totals(db);
        let mut add = |tree: u16, pt: i16| {
            if tree != 0 {
                *totals.entry(tree).or_default() += pt;
            }
        };
        // Talisman boost: the charm's points and the decorations in its slots once more
        if boost {
            for s in self.charm_data(db).map(|c| c.skills).into_iter().flatten() {
                add(s.tree, s.pt as i16);
            }
            for &(id, loc) in &self.jewels {
                if let Some(j) = db.jewel(id).filter(|_| loc == LOC_CHARM) {
                    add(j.pos.tree, j.pos.pt as i16);
                    add(j.neg.tree, j.neg.pt as i16);
                }
            }
        }
        let secret = totals.get(&TREE_SECRET).copied().unwrap_or(0) >= 10;
        if secret {
            for v in totals.values_mut() {
                if *v != 0 {
                    *v += 2;
                }
            }
        }
        totals
    }

    // ---- Save format: one line ----
    // free,bonus,def,weapon,needed,charm_rare|part0 ID;ID..|..|part4|decoration ID:t;..|charm t1:p1:t2:p2:slots or -|target IDs,..
    pub fn encode(&self, db: &Db) -> String {
        let mut s = String::new();
        let _ = write!(s, "{},{},{},{},{},{}", self.free, self.bonus, self.def, self.weapon, self.needed as u8, self.charm_rare);
        for p in 0..PARTS {
            s.push('|');
            for (i, id) in self.parts[p].iter().enumerate() {
                let _ = write!(s, "{}{}", if i > 0 { ";" } else { "" }, id);
            }
        }
        s.push('|');
        for (i, (id, t)) in self.jewels.iter().enumerate() {
            let _ = write!(s, "{}{}:{}", if i > 0 { ";" } else { "" }, id, t);
        }
        s.push('|');
        match self.charm_data(db) {
            Some(c) => {
                let _ = write!(s, "{}:{}:{}:{}:{}", c.skills[0].tree, c.skills[0].pt, c.skills[1].tree, c.skills[1].pt, c.slots);
            }
            None => s.push('-'),
        }
        s.push('|');
        for (i, id) in self.targets.iter().enumerate() {
            let _ = write!(s, "{}{}", if i > 0 { "," } else { "" }, id);
        }
        s
    }

    pub fn decode(line: &str) -> Option<Set> {
        let f: Vec<&str> = line.split('|').collect();
        if f.len() != 9 {
            return None;
        }
        let head: Vec<u32> = f[0].split(',').filter_map(|v| v.parse().ok()).collect();
        if head.len() != 6 {
            return None;
        }
        let parts: [Vec<u16>; PARTS] = core::array::from_fn(|p| f[1 + p].split(';').filter_map(|v| v.parse().ok()).collect());
        if parts.iter().any(|v| v.is_empty()) {
            return None;
        }
        let jewels = f[6]
            .split(';')
            .filter_map(|j| j.split_once(':'))
            .filter_map(|(a, b)| Some((a.parse().ok()?, b.parse::<u8>().ok().filter(|&l| l <= LOC_CHARM).unwrap_or(LOC_ANY))))
            .collect();
        let saved_charm = if f[7] == "-" {
            None
        } else {
            let v: Vec<i32> = f[7].split(':').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 5).then(|| Charm {
                skills: [Pt { tree: v[0] as u16, pt: v[1] as i8 }, Pt { tree: v[2] as u16, pt: v[3] as i8 }],
                slots: v[4] as u8,
            })
        };
        Some(Set {
            free: head[0] as u8,
            bonus: head[1] as u8,
            def: head[2] as u16,
            weapon: head[3] as u8,
            needed: head[4] == 1,
            charm_rare: head[5] as u8,
            charm: -1,
            parts,
            jewels,
            saved_charm,
            targets: f[8].split(',').filter_map(|v| v.parse().ok()).collect(),
            chance: 0.,
            tags: 0,
        })
    }
}
