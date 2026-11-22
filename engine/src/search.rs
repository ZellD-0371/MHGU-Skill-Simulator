//! Armor combination search.
//!
//! Structure
//! 1. Project matching armor/charms onto "target skill points + slots + special trees" and group identical ones.
//! 2. (Optional) remove groups beaten in every respect by another group.
//! 3. Depth-first search torso -> other parts -> charm, cutting branches that cannot reach the targets
//!    even with the remaining parts and decorations. The search uses an explicit stack, so `run` can be called in pieces.
//! 4. When all six slots are filled, solve the decoration placement exactly for the solution using the fewest slots.
use crate::charm::KindView;
use crate::types::*;
use alloc::vec::Vec;

pub const MAXT: usize = 8;
/// Fixed-point scale of the slot-equivalent bound (LCM of 1..10: keeps decoration size/point ratios integral)
const VAL_SCALE: u32 = 2520;

/// Resistance and defense in the slot-equivalent bounds (cond_value): value of one missing resistance point, x VAL_SCALE
/// per slot. The bound holds for any weight; each is tried and the tightest is used (measured best range, 2026-11-22).
const RES_LAMS: [i64; 7] = [VAL_SCALE as i64 / 4, VAL_SCALE as i64 / 2, VAL_SCALE as i64, 2 * VAL_SCALE as i64, 4 * VAL_SCALE as i64, 8 * VAL_SCALE as i64, 16 * VAL_SCALE as i64];
/// A defense point's weight = the resistance weight / one of these (each paired with each resistance weight)
const DEF_DIVS: [i64; 3] = [4, 16, 64];
/// Dimensions of that bound: the five elements and defense (bit RES)
const COND_DIMS: usize = RES + 1;
const NEG_VAL: i64 = i64::MIN / 4;
pub const MAXJ: usize = 24;
pub const LEVELS: usize = 6;
/// Internal priority of the needed-charm mode: easiest charm first.
const PRIORITY_CHARM: u8 = 3;
const CHARM_LEVEL: usize = 5;
const MAXD: usize = 128;
const INF: u8 = 255;
const NONE: i16 = -1000;
const NOCOST: u32 = 1 << 20;
/// One slot of a virtual charm: (target index, points)
type VSide = Option<(usize, u8)>;
/// Virtual charm candidate: (chance x 1e15, slots, first slot, second slot)
type VCand = (u64, u8, VSide, VSide);

#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub tree: u16,
    pub need: i16,
}

#[derive(Clone, Copy)]
pub struct Query<'a> {
    pub targets: &'a [Target],
    /// 1 blademaster, 2 gunner
    pub kind: u8,
    /// 1 male, 2 female
    pub sex: u8,
    /// bit r = rarity r allowed
    pub rare_mask: u16,
    pub weapon_slots: u8,
    /// Armor with these flags (FLAG_ARENA etc.) is not used. Fixed armor is exempt.
    pub exclude_flags: u8,
    /// Only decorations craftable at or below this rank (1 low, 2 high, 3 G).
    pub jewel_rank: u8,
    /// If given, only these decorations (ids, sorted) on top of jewel_rank: the ones the smithy lists right now.
    pub jewel_ids: Option<&'a [u16]>,
    /// Result priority: 0 free slots, 1 defense, 2 lowest rarity (sets whose highest-rarity part to craft is lowest first,
    /// ties by free slots -> parts to craft -> defense).
    /// Anything but 0 cannot use free-slot pruning and is slower.
    pub priority: u8,
    pub forbid_negative: bool,
    /// Trees with negative skills (used for forbidding negative skills). Empty means every tree counts as negative at -10.
    /// Trees without negative skills activate nothing even at -10 or below, so they are not blocked.
    pub negative_trees: &'a [u16],
    /// Dominated group removal. Keeps the max free slots and whether a solution exists,
    /// but tied sets that differ only in defense or extra skills may be dropped.
    pub dominance: bool,
    /// Also compare defense when removing dominated groups (keep groups with higher defense).
    pub keep_defense: bool,
    /// With forbid_negative, non-target trees compare only negative points (groups shrink a lot).
    /// May miss rare combinations where another part's negative is offset by a non-target positive.
    pub relaxed: bool,
    /// Fixed armor id per part. 0 means not fixed.
    pub fixed: [u16; PARTS],
    pub excluded: &'a [u16],
    pub charms: &'a [Charm],
    pub max_results: usize,
    /// If not 0, only sets with total defense at or above this value.
    pub min_def: u16,
    /// Lower bound of the resistance total per element (fire, water, thunder, ice, dragon). None means no condition.
    pub min_res: [Option<i8>; RES],
    /// A set differing from an earlier result in only one part (same charm) is grouped into that result's variants instead of its own row.
    /// Grouped sets do not count toward the result limit (max_results), so more distinct sets fit.
    pub group_similar: bool,
    /// If given, look for the "needed charm" instead of the owned charms. Results ordered by easiest charm.
    pub charm_kinds: Option<&'a [CharmKind]>,
    /// Armor ids in the equipment box. Owned armor is not dropped for being beaten by unowned armor,
    /// and with equal free slots, sets with fewer parts to craft come first.
    pub owned: &'a [u16],
    /// Collect "non-dominated sets" separately (sets not beaten in both free slots and parts to craft; on a tie, higher defense).
    /// At most one per parts-to-craft count, so 6 at most.
    /// Sets pushed out of the result limit stay here. Groups with higher defense are not dropped either, so it is slower.
    /// Owned-charm mode only (ignored in needed-charm mode).
    pub pareto: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    BadTargets,
    /// Decorations raising special trees (torso up / secret / talisman boost) are not supported.
    UnsupportedJewel,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Status {
    Paused,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    /// Is the armor owned (different ownership means a different group)
    own: bool,
    /// Rarity priority only: rarity of unowned armor (0 otherwise)
    rare: u8,
    tup: bool,
    proj: [i16; MAXT],
    slots: u8,
    sec: i16,
    tal: i16,
    sig: [Pt; 4],
    /// Resistance of elements with a condition (0 without one). Armor in the same group has the same value.
    res: [i8; RES],
}

#[derive(Clone)]
pub struct Group {
    key: Key,
    first: u32,
    len: u32,
    /// Defense of the representative (the member with the highest defense)
    def: u16,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Placed {
    /// Index into `Searcher::jewel`
    pub jewel: u16,
    pub torso: bool,
    /// In the charm slots while talisman boost is active (points x2). Without talisman boost charm slots are ordinary slots (false).
    pub charm: bool,
}

#[derive(Clone, Debug)]
pub struct SetResult {
    /// Group index per part (`Searcher::armor_members`)
    pub groups: [u16; PARTS],
    /// Charm group index (`Searcher::charm_members`; empty means no charm)
    pub charm: u16,
    pub jewels: [Placed; MAXJ],
    pub nj: u8,
    pub free: u8,
    pub bonus: u8,
    pub def: u16,
    /// Number of parts to craft (unowned armor)
    pub craft: u8,
    /// Rarity priority only: highest rarity among armor to craft (0 otherwise)
    pub rare: u8,
    /// Charm required in needed-charm mode (skills[0] first slot, skills[1] second slot, tree 0 = anything).
    pub need: Charm,
    /// Estimated chance x 1e15 of getting that charm or better in one appraisal. 0 outside needed-charm mode.
    pub chance: u64,
    /// With group_similar: sets differing from this one in one part (best first, at most MAX_VARIANTS)
    pub variants: Vec<SetResult>,
}

/// Limit of similar sets grouped into one result
pub const MAX_VARIANTS: usize = 20;

/// Same charm (owned charm group or needed charm) and armor differing in exactly one part?
fn similar(a: &SetResult, b: &SetResult) -> bool {
    a.charm == b.charm && a.need == b.need && (0..PARTS).filter(|&p| a.groups[p] != b.groups[p]).count() == 1
}

impl SetResult {
    /// sub: user priority to follow on equal chance in needed-charm mode
    fn rank(&self, priority: u8, sub: u8) -> (u64, u16, u16, u16) {
        // Fewer parts to craft is better, so compare reversed.
        let (free, easy) = (self.free as u16, (PARTS as u16).saturating_sub(self.craft as u16));
        match priority {
            1 => (0, self.def, free, easy),
            // Lower rarity is better. Parts to craft and defense after free slots share one field (defense < 4096).
            2 => (0, 16u16.saturating_sub(self.rare as u16), free, easy << 12 | self.def.min(4095)),
            PRIORITY_CHARM if sub == 1 => (self.chance, self.def, free, easy),
            PRIORITY_CHARM => (self.chance, free, easy, self.def),
            _ => (0, free, easy, self.def),
        }
    }

    /// Can a set with the given values not become non-dominated because of this set?
    /// Compare by free slots and parts to craft (wins if not worse in both and better in one);
    /// if both are equal, higher or equal defense wins. Using defense as a full criterion
    /// would add a slightly-higher-defense set per free slot count and grow the choices to dozens.
    fn covers(&self, free: u8, craft: u8, def: u16) -> bool {
        self.free >= free && self.craft <= craft && (self.free > free || self.craft < craft || self.def >= def)
    }
}

#[derive(Clone, Copy, Default)]
struct State {
    cur: [i16; MAXT],
    /// Number of non-torso slot containers (size 1..=3)
    n: [u8; 4],
    ts: u8,
    /// Slots of the chosen charm (also counted in n). With talisman boost they hold decorations at x2.
    cs: u8,
    k: u8,
    sec: i16,
    tal: i16,
    def: u16,
    res: [i16; RES],
    craft: u8,
    rare: u8,
}

#[derive(Clone, Copy, Default)]
struct Level {
    idx: usize,
    state: State,
}

struct Deco {
    totals: [i16; NT],
    last: [u8; NT],
    watch: [u16; 64],
    nw: usize,
    stack: [Placed; MAXJ],
    ns: usize,
    c: [u8; 4],
    tc: u8,
    n: [u8; 4],
    ts: u8,
    m: i16,
    /// Charm container at x2 (talisman boost): capacity and used. 0 when the charm slots are ordinary slots (in n).
    cs: u8,
    cc: u8,
    req: [i16; MAXT],
    neg_thr: i16,
    other_total: u8,
    /// Only slot usage below this value is accepted.
    best: u8,
    best_stack: [Placed; MAXJ],
    best_n: usize,
    found: bool,
    nodes: u64,
}

#[derive(Clone)]
pub struct Searcher<'a> {
    armor: &'a [Armor],
    charms: &'a [Charm],
    tt: [u16; MAXT],
    need: [i16; MAXT],
    nt: usize,
    t2i: [u8; NT],
    forbid: bool,
    /// Trees with negative skills
    neg: [bool; NT],
    priority: u8,
    user_priority: u8,
    group_similar: bool,
    max_results: usize,
    min_def: u16,
    views: Vec<KindView>,
    vmax1: [u8; MAXT],
    vmax2: [u8; MAXT],
    tal_possible: bool,
    ccache: Vec<(u32, f64)>,
    /// Size of ccache as a power of two (13 = 8192 entries; smaller for searches that stop at the first set)
    ccache_bits: u32,
    jewels: Vec<Jewel>,
    jstart: [u16; NT + 1],
    mincost: [[u8; MAXD]; MAXT],
    groups: [Vec<Group>; LEVELS],
    members: [Vec<u16>; LEVELS],
    level_part: [usize; PARTS],
    part_level: [usize; PARTS],
    lvl_max: [[i16; MAXT]; LEVELS],
    lvl_sec: [i16; LEVELS],
    lvl_tal: [i16; LEVELS],
    lvl_slots: [u8; LEVELS],
    lvl_tup: [bool; LEVELS],
    suf_m: [[i16; MAXT]; LEVELS + 1],
    suf_sec: [i16; LEVELS + 1],
    suf_tal: [i16; LEVELS + 1],
    suf_slots: [u8; LEVELS + 1],
    suf_tup: [u8; LEVELS + 1],
    torso_proj: [i16; MAXT],
    torso_sec: i16,
    torso_tal: i16,
    levels: [Level; LEVELS],
    chosen: [u16; LEVELS],
    depth: usize,
    done: bool,
    results: Vec<SetResult>,
    worst: usize,
    pareto: bool,
    front: Vec<SetResult>,
    /// Sum of the max defense of the parts from level d on
    suf_def: [u16; LEVELS + 1],
    pub nodes: u64,
    pub leaves: u64,
    pub deco_nodes: u64,
    /// Per level (groups, groups after dominance removal). Level 0 is torso, 5 is charm.
    pub group_counts: [(u16, u16); LEVELS],
    min_res: [Option<i8>; RES],
    suf_res: [[i16; RES]; LEVELS + 1],
    /// A resistance or defense condition is set (quick_short in the search loop)
    quick: bool,
    /// Resistance conditions: armor groups split by resistance (the key holds it), so with conditions on many elements a
    /// level had ~70 groups instead of ~11 and the skill bounds repeated the same work for every resistance variant
    /// (five elements: 14-30M nodes, 2026-11-22). The search then walks coarse groups (same key without resistance; the
    /// key's resistance is the per-element max, the defense the max) and, once a whole set is chosen, expands each part
    /// into its resistance groups (expand). Bounds on a coarse group are at least as loose as on any of its groups, so no
    /// set is lost. Empty when there is no resistance condition (the search walks `groups`).
    coarse: [Vec<Group>; PARTS],
    /// Resistance groups (indices into `groups[l]`) of each coarse group: coarse[l][c].first / len index this list
    coarse_fine: [Vec<u16>; PARTS],
    /// Resistance / defense conditions in the slot-equivalent bounds (cond_value). The weight pairs (resistance, defense),
    /// and per subset of the conditioned dimensions (bit e = element e, bit RES = defense) its index among them (255 = not
    /// a subset). Index of the tables = weight pair * cond_n + subset index. Per level the max of 'value + weights x the
    /// subset's resistance and defense' over non-torso-up groups, and the max of 'weights x ...' over torso-up groups
    /// (their value depends on the torso: rebuild_suffix). Empty without conditions.
    cond_w: Vec<(i64, i64)>,
    cond_idx: [u8; 1 << COND_DIMS],
    cond_n: usize,
    lvl_valr: Vec<[i64; LEVELS]>,
    lvl_tupr: Vec<[i64; LEVELS]>,
    suf_valr: Vec<[i64; LEVELS + 1]>,
    /// Split work (parallel search): searchers with the same conditions take torso candidates one at a time from this ticket counter.
    /// Without it, one searcher goes through all of them in order.
    root: Option<&'a core::sync::atomic::AtomicU32>,
    /// Sets added in advance by seed (not added again when the search finds the same set)
    seeded: alloc::collections::BTreeSet<SetKey>,
    /// Can talisman boost doubling never hurt? (decided by the owned charms; part of the charm signature)
    tal_mono: bool,
    /// Slot-equivalent bound: min slot cost of 1 target point (xVAL_SCALE), max per level, sum from level d on (depends on torso)
    rate: [u32; MAXT],
    lvl_val: [u32; LEVELS],
    suf_val: [u32; LEVELS + 1],
    /// Slots of the chosen torso (for valuing torso-up parts)
    torso_slots: u8,
    /// Needed-charm mode: (charm value, max chance x1e15 among charms with at least that value), value ascending. [no talisman boost, boost]
    ctables: [Vec<(u32, u64)>; 2],
    ctables_built: bool,
    /// Value of the charm slot [no talisman boost, boost] (lvl_val[CHARM_LEVEL] uses 'boost' when possible)
    charm_val: [u32; 2],
    /// Max points of one part counted in the value (target + sum of possible negatives)
    gcap: [i32; MAXT],
}

/// Charm signature: among the owned charms under this condition, the charm groups not beaten by another charm (target points, slots, etc.)
/// plus the talisman-boost flag. If two charm lists have the same signature, the owned-charm results for that condition are the same
/// (the signature does not change when every newly obtained charm is worse than or equal to existing ones).
#[derive(Clone, PartialEq)]
pub struct CharmSignature {
    keys: Vec<Key>,
    tal_mono: bool,
}

/// Key for identifying the same set: group per part, charm group, needed charm, free slots
type SetKey = ([u16; PARTS], u16, [Pt; 2], u8, u8);
fn set_key(x: &SetResult) -> SetKey {
    (x.groups, x.charm, x.need.skills, x.need.slots, x.free)
}

fn sig_get(s: &[Pt; 4], tree: u16) -> i8 {
    s.iter().find(|x| x.tree == tree).map_or(0, |x| x.pt)
}

fn sig_ge(a: &[Pt; 4], b: &[Pt; 4]) -> bool {
    b.iter().all(|x| x.tree == 0 || sig_get(a, x.tree) >= x.pt)
        && a.iter().all(|x| x.tree == 0 || x.pt >= sig_get(b, x.tree))
}

/// `tal_mono`: can talisman boost (charm points x2) never hurt? If an owned charm has negative points
/// in a target tree or a forbidden tree, doubling can hurt, so compare only when talisman boost and secret points are equal.
fn dominates(a: &Key, b: &Key, tal_mono: bool) -> bool {
    !a.tup
        && !b.tup
        && a.slots >= b.slots
        && if tal_mono { a.sec >= b.sec && a.tal >= b.tal } else { a.sec == b.sec && a.tal == b.tal }
        && a.proj.iter().zip(&b.proj).all(|(x, y)| x >= y)
        && sig_ge(&a.sig, &b.sig)
        && a.res.iter().zip(&b.res).all(|(x, y)| x >= y)
        && (a.own || !b.own)
        && a.rare <= b.rare
}

/// Do the decoration counts per size c fit into the containers n?
fn fits(c: &[u8; 4], n: &[u8; 4]) -> bool {
    c[3] <= n[3]
        && c[2] <= n[2] + n[3] - c[3]
        && c[1] + 2 * c[2] + 3 * c[3] <= n[1] + 2 * n[2] + 3 * n[3]
}

impl<'a> Searcher<'a> {
    pub fn new(armor: &'a [Armor], jewels: &[Jewel], q: &Query<'a>) -> Result<Self, Error> {
        crate::trace::mark(1);
        let nt = q.targets.len();
        if nt == 0 || nt > MAXT {
            return Err(Error::BadTargets);
        }
        let mut t2i = [255u8; NT];
        let mut tt = [0u16; MAXT];
        let mut need = [0i16; MAXT];
        for (i, t) in q.targets.iter().enumerate() {
            let ok = t.tree as usize > TREE_TORSO_UP as usize && (t.tree as usize) < NT && t.need >= 3;
            if !ok || t2i[t.tree as usize] != 255 {
                return Err(Error::BadTargets);
            }
            t2i[t.tree as usize] = i as u8;
            tt[i] = t.tree;
            need[i] = t.need;
        }

        crate::trace::mark(2);
        let mut js: Vec<Jewel> =
            jewels.iter().filter(|j| j.rank <= q.jewel_rank && q.jewel_ids.is_none_or(|ids| ids.binary_search(&j.id).is_ok())).copied().collect();
        js.sort_by(|a, b| (a.pos.tree, b.size, a).cmp(&(b.pos.tree, a.size, b)));
        js.dedup_by(|a, b| (a.size, a.pos, a.neg) == (b.size, b.pos, b.neg));
        if js.iter().any(|j| {
            j.pos.pt <= 0 || j.neg.pt > 0 || [TREE_TORSO_UP, TREE_SECRET, TREE_TALISMAN].contains(&j.pos.tree)
        }) {
            return Err(Error::UnsupportedJewel);
        }
        let mut jstart = [0u16; NT + 1];
        for j in &js {
            jstart[j.pos.tree as usize + 1] += 1;
        }
        for t in 0..NT {
            jstart[t + 1] += jstart[t];
        }

        crate::trace::mark(3);
        let mut mincost = [[INF; MAXD]; MAXT];
        for i in 0..nt {
            let list = &js[jstart[tt[i] as usize] as usize..jstart[tt[i] as usize + 1] as usize];
            mincost[i][0] = 0;
            for d in 1..MAXD {
                for j in list {
                    let prev = mincost[i][d.saturating_sub(j.pos.pt as usize)];
                    if prev != INF {
                        mincost[i][d] = mincost[i][d].min(prev + j.size);
                    }
                }
            }
        }

        crate::trace::mark(4);
        let mut neg = [q.negative_trees.is_empty(); NT];
        for &t in q.negative_trees {
            if (t as usize) < NT {
                neg[t as usize] = true;
            }
        }
        let key_of = |skills: &[Pt], slots: u8, tup: bool, res: &[i8; RES], own: bool, rare: u8| -> Key {
            // Rarity splits groups only where it ranks results: the owned-charm search with rarity priority. The unowned-charm
            // search ranks by chance, then free slots (rarity unused), and splitting there only made it bigger: with a minimum
            // defense, 44.6M nodes instead of 71k for the same 30 sets (2026-11-22, over 2 minutes on Recomp).
            let rare = if q.priority == 2 && q.charm_kinds.is_none() && !own { rare } else { 0 };
            let mut k =
                Key { own, rare, tup, proj: [0; MAXT], slots, sec: 0, tal: 0, sig: [Pt::default(); 4], res: [0; RES] };
            for e in 0..RES {
                if q.min_res[e].is_some() {
                    k.res[e] = res[e];
                }
            }
            if tup {
                k.slots = 0;
                return k;
            }
            let mut n = 0;
            for s in skills.iter().filter(|s| s.tree != 0 && s.pt != 0) {
                let i = t2i[s.tree as usize];
                if i != 255 {
                    k.proj[i as usize] += s.pt as i16;
                }
                if s.tree == TREE_SECRET {
                    k.sec += s.pt as i16;
                } else if s.tree == TREE_TALISMAN {
                    k.tal += s.pt as i16;
                } else if i == 255 && q.forbid_negative && neg[s.tree as usize] && (!q.relaxed || s.pt < 0) {
                    match k.sig[..n].iter_mut().find(|x| x.tree == s.tree) {
                        Some(x) => x.pt += s.pt,
                        None => {
                            k.sig[n] = *s;
                            n += 1;
                        }
                    }
                }
            }
            k.sig[..n].sort();
            k
        };

        crate::trace::mark(5);
        let mut groups: [Vec<Group>; LEVELS] = Default::default();
        let mut members: [Vec<u16>; LEVELS] = Default::default();
        let mut group_counts = [(0u16, 0u16); LEVELS];
        // Virtual charms only have positive points in target trees, so doubling always helps.
        let tal_mono = q.charm_kinds.is_some()
            || q.charms.iter().flat_map(|c| c.skills.iter()).all(|s| {
                s.tree == 0
                    || s.pt >= 0
                    || !(t2i[s.tree as usize] != 255
                        || s.tree == TREE_SECRET
                        || s.tree == TREE_TALISMAN
                        || (q.forbid_negative && neg[s.tree as usize]))
            });
        let pareto = q.pareto && q.charm_kinds.is_none();
        // For non-dominated sets (pareto) to be exact down to defense, also turn on keep_defense ('Thorough').
        // pareto used to always turn it on, which made searches with owned armor 10-35x slower (measured 2026-11-18).
        // Without it, free slots and parts to craft are exact; only the highest-defense set may be missed.
        let keep_defense = q.keep_defense;
        let mut build = |level: usize, mut items: Vec<(Key, u16, u16)>| {
            // (key, defense, original index): within the same key, highest defense first
            crate::trace::mark(100 + level as u32);
            items.sort_by(|a, b| (a.0, b.1, a.2).cmp(&(b.0, a.1, b.2)));
            crate::trace::mark(110 + level as u32);
            let mut gs: Vec<Group> = Vec::new();
            let mut ms: Vec<u16> = Vec::new();
            for (key, def, idx) in items {
                match gs.last_mut() {
                    Some(g) if g.key == key => g.len += 1,
                    _ => gs.push(Group { key, first: ms.len() as u32, len: 1, def }),
                }
                ms.push(idx);
            }
            group_counts[level].0 = gs.len() as u16;
            if q.dominance {
                let keys: Vec<(Key, u16)> = gs.iter().map(|g| (g.key, g.def)).collect();
                // Two groups that beat each other (effectively equal) must not both be dropped: only the one earlier in key order wins.
                gs.retain(|g| {
                    !keys.iter().any(|(k, def)| {
                        *k != g.key
                            && dominates(k, &g.key, tal_mono)
                            && (!keep_defense || *def >= g.def)
                            && (!dominates(&g.key, k, tal_mono) || (keep_defense && *def > g.def) || *k < g.key)
                    })
                });
            }
            crate::trace::mark(120 + level as u32);
            group_counts[level].1 = gs.len() as u16;
            let score = |g: &Group| -> i32 {
                g.key.proj.iter().map(|&p| p as i32 * 4).sum::<i32>()
                    + g.key.slots as i32 * 5
                    + g.key.sec as i32
                    + g.key.tal as i32
            };
            // Rarity priority: try low-rarity groups first so the results fill with low rarity early and the
            // rarity bound starts cutting sooner (key.rare is 0 for owned armor and outside rarity priority).
            gs.sort_by(|a, b| a.key.rare.cmp(&b.key.rare).then(score(b).cmp(&score(a))));
            groups[level] = gs;
            members[level] = ms;
        };

        // Torso first, then the parts with the fewest candidates.
        crate::trace::mark(6);
        let mut per_part: [Vec<(Key, u16, u16)>; PARTS] = Default::default();
        for (idx, a) in armor.iter().enumerate() {
            let p = a.part as usize;
            // A fixed part only needs the id check (the other checks are skipped: many small searches fix every part)
            let pass = if q.fixed[p] != 0 {
                q.fixed[p] == a.id
            } else {
                (a.kind == 0 || a.kind == q.kind)
                    && (a.sex == 0 || a.sex == q.sex)
                    && q.rare_mask >> a.rare & 1 == 1
                    && a.flags & q.exclude_flags == 0
                    && !q.excluded.contains(&a.id)
            };
            if pass && !(p == TORSO && a.torso_up()) {
                let own = q.owned.contains(&a.id);
                per_part[p].push((key_of(&a.skills, a.slots, a.torso_up(), &a.res, own, a.rare), a.def, idx as u16));
            }
        }
        crate::trace::mark(7);
        let mut others: Vec<usize> = (0..PARTS).filter(|&p| p != TORSO).collect();
        others.sort_by_key(|&p| per_part[p].len());
        let mut level_part = [TORSO; PARTS];
        level_part[1..].copy_from_slice(&others);
        let mut part_level = [0usize; PARTS];
        for (l, &p) in level_part.iter().enumerate() {
            part_level[p] = l;
            build(l, core::mem::take(&mut per_part[p]));
        }
        crate::trace::mark(8);
        let owned: &[Charm] = if q.charm_kinds.is_some() { &[] } else { q.charms };
        let charm_items: Vec<(Key, u16, u16)> = owned
            .iter()
            .enumerate()
            .map(|(i, c)| (key_of(&c.skills, c.slots, false, &[0; RES], false, 0), 0, i as u16))
            .collect();
        build(CHARM_LEVEL, charm_items);
        if groups[CHARM_LEVEL].is_empty() {
            // No charm
            let key = Key {
                own: false,
                rare: 0,
                tup: false,
                proj: [0; MAXT],
                slots: 0,
                sec: 0,
                tal: 0,
                sig: [Pt::default(); 4],
                res: [0; RES],
            };
            groups[CHARM_LEVEL].push(Group { key, first: 0, len: 0, def: 0 });
        }

        crate::trace::mark(9);
        let tal_possible = groups.iter().flatten().any(|g| g.key.tal > 0);
        let mut lvl_max = [[NONE; MAXT]; LEVELS];
        let mut lvl_sec = [NONE; LEVELS];
        let mut lvl_tal = [NONE; LEVELS];
        let mut lvl_slots = [0u8; LEVELS];
        let mut lvl_tup = [false; LEVELS];
        let mut suf_res = [[0i16; RES]; LEVELS + 1];
        for l in (0..LEVELS).rev() {
            for e in 0..RES {
                let best = groups[l].iter().map(|g| g.key.res[e] as i16).max().unwrap_or(NONE);
                suf_res[l][e] = suf_res[l + 1][e] + best;
            }
        }
        let mut coarse: [Vec<Group>; PARTS] = Default::default();
        let mut coarse_fine: [Vec<u16>; PARTS] = Default::default();
        if q.min_res.iter().any(|m| m.is_some()) {
            for l in 0..PARTS {
                // Coarse groups in the order of their first resistance group (the search order of the level)
                let mut lists: Vec<(Key, u16, [i8; RES], Vec<u16>)> = Vec::new();
                for (gi, g) in groups[l].iter().enumerate() {
                    let k = Key { res: [0; RES], ..g.key };
                    match lists.iter_mut().find(|x| x.0 == k) {
                        Some(x) => {
                            x.1 = x.1.max(g.def);
                            for e in 0..RES {
                                x.2[e] = x.2[e].max(g.key.res[e]);
                            }
                            x.3.push(gi as u16);
                        }
                        None => lists.push((k, g.def, g.key.res, alloc::vec![gi as u16])),
                    }
                }
                for (k, def, res, fine) in lists {
                    coarse[l].push(Group { key: Key { res, ..k }, first: coarse_fine[l].len() as u32, len: fine.len() as u32, def });
                    coarse_fine[l].extend(fine);
                }
            }
        }
        let mut suf_def = [0u16; LEVELS + 1];
        for l in (0..LEVELS).rev() {
            suf_def[l] = suf_def[l + 1] + groups[l].iter().map(|g| g.def).max().unwrap_or(0);
        }
        for l in 0..LEVELS {
            let double = l == CHARM_LEVEL && tal_possible;
            for g in &groups[l] {
                if g.key.tup {
                    lvl_tup[l] = true;
                    continue;
                }
                for i in 0..nt {
                    let p = g.key.proj[i];
                    lvl_max[l][i] = lvl_max[l][i].max(if double { p.max(2 * p) } else { p });
                }
                lvl_sec[l] = lvl_sec[l].max(g.key.sec);
                lvl_tal[l] = lvl_tal[l].max(g.key.tal);
                lvl_slots[l] = lvl_slots[l].max(g.key.slots);
            }
        }

        // Slot-equivalent bound (prune, prune_virtual): the cheapest slot cost to fill 1 target point with decorations (xVAL_SCALE, rounded down) and,
        // per part (level), the max of 'slots + target points converted'. One armor cannot give every target and slot at its max at once,
        // so this is much tighter than suf_m/suf_slots, which add the max of each target separately. It ignores decoration size fitting and side effects (loose),
        // so it never drops a possible set. Targets without decorations get cost 0 and are left to the per-target check.
        let mut rate = [0u32; MAXT];
        for i in 0..nt {
            let list = &js[jstart[tt[i] as usize] as usize..jstart[tt[i] as usize + 1] as usize];
            rate[i] = list.iter().filter(|j| j.pos.pt > 0).map(|j| VAL_SCALE * j.size as u32 / j.pos.pt as u32).min().unwrap_or(0);
        }
        // When computing the value, a part's points count only up to 'the most that can be missing'. Negative points on other parts (e.g. torso -3)
        // can make the shortfall exceed the target, so count up to target + the sum of possible negatives (cutting at the target dropped valid sets:
        // 2026-11-19 stress case 1000303).
        let mut gcap = [0i32; MAXT];
        for i in 0..nt {
            let mut neg = 0i32;
            let torso_min = groups[0].iter().map(|g| g.key.proj[i] as i32).min().unwrap_or(0).min(0);
            for l in 0..LEVELS {
                let mult = if l == CHARM_LEVEL && tal_possible { 2 } else { 1 };
                let mut m = groups[l].iter().filter(|g| !g.key.tup).map(|g| g.key.proj[i] as i32 * mult).min().unwrap_or(0).min(0);
                if l > 0 && groups[l].iter().any(|g| g.key.tup) {
                    m = m.min(torso_min);
                }
                neg -= m;
            }
            gcap[i] = (need[i] as i32).max(0) + neg;
        }
        // With talisman boost (mult 2) the charm's slots hold decorations at x2, so they count double too
        let value = |k: &Key, mult: i32| -> u32 {
            let mut v = VAL_SCALE * k.slots as u32 * mult as u32;
            for i in 0..nt {
                let p = (k.proj[i] as i32).max(0) * mult;
                v += rate[i] * p.min(gcap[i]) as u32;
            }
            v
        };
        let mut lvl_val = [0u32; LEVELS];
        for l in 0..LEVELS {
            let mult = if l == CHARM_LEVEL && tal_possible { 2 } else { 1 };
            lvl_val[l] = groups[l].iter().filter(|g| !g.key.tup).map(|g| value(&g.key, mult)).max().unwrap_or(0);
        }
        // Value of the charm slot: [no talisman boost, boost]. If a branch cannot reach talisman boost (10), the smaller one is used (charm_adj).
        let mut charm_val = [0u32; 2];
        for (fi, mult) in [(0usize, 1i32), (1, 2)] {
            charm_val[fi] = groups[CHARM_LEVEL].iter().filter(|g| !g.key.tup).map(|g| value(&g.key, mult)).max().unwrap_or(0);
        }

        crate::trace::mark(10);
        let views: Vec<KindView> =
            q.charm_kinds.unwrap_or(&[]).iter().map(|k| KindView::new(k, &tt[..nt])).collect();
        let mut vmax1 = [0u8; MAXT];
        let mut vmax2 = [0u8; MAXT];
        for i in 0..nt {
            vmax1[i] = views.iter().map(|v| v.max_first(i)).max().unwrap_or(0);
            vmax2[i] = views.iter().map(|v| v.max_second(i)).max().unwrap_or(0);
        }
        if q.charm_kinds.is_some() {
            // Virtual charm: its points are computed separately by prune_virtual; slots count as at most 3.
            lvl_max[CHARM_LEVEL] = [0; MAXT];
            lvl_slots[CHARM_LEVEL] = 3;
            // Value of the virtual charm in the bound: 3 slots + the largest value two slots (first/second, different targets) can give
            for (fi, f) in [(0usize, 1i32), (1, 2)] {
                let pv = |i: usize, p: u8| rate[i] * ((p as i32) * f).min(gcap[i]) as u32;
                let mut best = 0u32;
                for i in 0..nt {
                    best = best.max(pv(i, vmax1[i])).max(pv(i, vmax2[i]));
                    for j in 0..nt {
                        if i != j {
                            best = best.max(pv(i, vmax1[i]) + pv(j, vmax2[j]));
                        }
                    }
                }
                charm_val[fi] = VAL_SCALE * 3 * f as u32 + best;
            }
            lvl_val[CHARM_LEVEL] = charm_val[tal_possible as usize];
        }
        // Resistance and defense in the slot-equivalent bounds: a set must cover the missing target value with slots and
        // part values and also reach each resistance and the defense; adding 'weight x missing amount' to both sides keeps
        // it a valid bound, and a part gives its value, resistance and defense together. Groups beaten in skills but kept
        // for their resistance or defense then no longer count with the best of everything at once (2026-11-22: resistance
        // grid 580M -> 226M nodes, minimum defense grid 384M -> 180M).
        let mut cond_mask = 0usize;
        for e in 0..RES {
            if q.min_res[e].is_some() {
                cond_mask |= 1 << e;
            }
        }
        if q.min_def > 0 {
            cond_mask |= 1 << RES;
        }
        let (mut cond_w, mut cond_idx, mut cond_n) = (Vec::new(), [255u8; 1 << COND_DIMS], 0usize);
        let (mut lvl_valr, mut lvl_tupr) = (Vec::new(), Vec::new());
        if cond_mask != 0 {
            let subsets: Vec<usize> = (0..1usize << COND_DIMS).filter(|&x| x & !cond_mask == 0).collect();
            for (k, &x) in subsets.iter().enumerate() {
                cond_idx[x] = k as u8;
            }
            cond_n = subsets.len();
            for &lam in &RES_LAMS {
                if cond_mask >> RES & 1 == 1 {
                    cond_w.extend(DEF_DIVS.iter().map(|&dv| (lam, lam / dv)));
                } else {
                    cond_w.push((lam, 0));
                }
            }
            // Per group: value (None for torso-up), resistance, defense
            // (an item no better than another in value, every resistance and defense is never a max: left out)
            let items: [Vec<(Option<i64>, [i8; RES], i64)>; PARTS] = core::array::from_fn(|l| {
                let all: Vec<(Option<i64>, [i8; RES], i64)> =
                    groups[l].iter().map(|g| ((!g.key.tup).then(|| value(&g.key, 1) as i64), g.key.res, g.def as i64)).collect();
                let le = |a: &(Option<i64>, [i8; RES], i64), b: &(Option<i64>, [i8; RES], i64)| {
                    a.0.is_some() == b.0.is_some() && a.0 <= b.0 && a.2 <= b.2 && (0..RES).all(|e| a.1[e] <= b.1[e])
                };
                (0..all.len())
                    .filter(|&i| !(0..all.len()).any(|j| j != i && le(&all[i], &all[j]) && (!le(&all[j], &all[i]) || j < i)))
                    .map(|i| all[i])
                    .collect()
            });
            let nw = cond_w.len();
            lvl_valr = alloc::vec![[NEG_VAL; LEVELS]; nw * cond_n];
            lvl_tupr = alloc::vec![[NEG_VAL; LEVELS]; nw * cond_n];
            for (k, &x) in subsets.iter().enumerate() {
                let with_def = x >> RES & 1 == 1;
                for l in 0..PARTS {
                    for (val, res, def) in &items[l] {
                        // The subset's resistance once per group, then each weight pair
                        let r: i64 = (0..RES).filter(|&e| x >> e & 1 == 1).map(|e| res[e] as i64).sum();
                        let dd = if with_def { *def } else { 0 };
                        for (w, &(wr, wd)) in cond_w.iter().enumerate() {
                            let c = wr * r + wd * dd;
                            let at = w * cond_n + k;
                            match val {
                                Some(val) => lvl_valr[at][l] = lvl_valr[at][l].max(val + c),
                                None => lvl_tupr[at][l] = lvl_tupr[at][l].max(c),
                            }
                        }
                    }
                }
            }
            // Charms have no resistance or defense
            for v in lvl_valr.iter_mut() {
                v[CHARM_LEVEL] = lvl_val[CHARM_LEVEL] as i64;
            }
        }
        let suf_valr = alloc::vec![[0i64; LEVELS + 1]; lvl_valr.len()];
        // Chance bound table (needed-charm mode): for every possible charm, its 'slots + target points' value and the chance to get it or better.
        // Kept in value order with the max chance accumulated from the end -> 'chance of the easiest charm with value V or more'.
        // After the results fill up, a branch is cut when its needed charm value is so high that no charm beats the worst result's chance (chance_short).
        crate::trace::mark(11);
        let mut s = Searcher {
            armor,
            charms: owned,
            tt,
            need,
            nt,
            t2i,
            forbid: q.forbid_negative,
            neg,
            priority: if q.charm_kinds.is_some() { PRIORITY_CHARM } else { q.priority },
            user_priority: q.priority,
            group_similar: q.group_similar,
            max_results: q.max_results.max(1),
            min_def: q.min_def,
            // Allocated on first use (vchance): searches that stop at the first set often never need it
            ccache: Vec::new(),
            ccache_bits: 13,
            views,
            vmax1,
            vmax2,
            tal_possible,
            jewels: js,
            jstart,
            mincost,
            groups,
            members,
            level_part,
            part_level,
            lvl_max,
            lvl_sec,
            lvl_tal,
            lvl_slots,
            lvl_tup,
            suf_m: [[0; MAXT]; LEVELS + 1],
            suf_sec: [0; LEVELS + 1],
            suf_tal: [0; LEVELS + 1],
            suf_slots: [0; LEVELS + 1],
            suf_tup: [0; LEVELS + 1],
            torso_proj: [0; MAXT],
            torso_sec: 0,
            torso_tal: 0,
            levels: [Level::default(); LEVELS],
            chosen: [0; LEVELS],
            depth: 0,
            done: false,
            results: Vec::new(),
            worst: 0,
            pareto,
            front: Vec::new(),
            suf_def,
            nodes: 0,
            leaves: 0,
            deco_nodes: 0,
            group_counts,
            min_res: q.min_res,
            suf_res,
            quick: q.min_def > 0 || q.min_res.iter().any(|m| m.is_some()),
            coarse,
            coarse_fine,
            cond_w,
            cond_idx,
            cond_n,
            lvl_valr,
            lvl_tupr,
            suf_valr,
            root: None,
            seeded: Default::default(),
            tal_mono,
            rate,
            lvl_val,
            suf_val: [0; LEVELS + 1],
            torso_slots: 0,
            ctables: Default::default(),
            ctables_built: false,
            charm_val,
            gcap,
        };
        if q.weapon_slots > 0 {
            s.levels[0].state.n[q.weapon_slots.min(3) as usize] += 1;
        }
        crate::trace::mark(12);
        Ok(s)
    }

    pub fn results(&self) -> &[SetResult] {
        &self.results
    }

    /// Results sorted best first.
    pub fn sorted_results(&self) -> Vec<SetResult> {
        let mut v = self.results.clone();
        v.sort_by(|a, b| b.rank(self.priority, self.user_priority).cmp(&a.rank(self.priority, self.user_priority)));
        v
    }

    /// Non-dominated sets (Query::pareto). Best first.
    pub fn front(&self) -> Vec<SetResult> {
        let mut v = self.front.clone();
        v.sort_by(|a, b| b.rank(self.priority, self.user_priority).cmp(&a.rank(self.priority, self.user_priority)));
        v
    }

    /// With these values (free slots upper bound, parts to craft lower bound, defense upper bound), is there no room to be non-dominated?
    fn front_hopeless(&self, free: u8, craft: u8, def: u16) -> bool {
        !self.pareto || self.front.iter().any(|f| f.covers(free, craft, def))
    }

    /// Armor in the same group (original array indices). The first is the representative.
    pub fn armor_members(&self, part: usize, group: u16) -> &[u16] {
        let l = self.part_level[part];
        let g = &self.groups[l][group as usize];
        &self.members[l][g.first as usize..(g.first + g.len) as usize]
    }

    pub fn charm_members(&self, group: u16) -> &[u16] {
        let g = &self.groups[CHARM_LEVEL][group as usize];
        &self.members[CHARM_LEVEL][g.first as usize..(g.first + g.len) as usize]
    }

    pub fn jewel(&self, idx: u16) -> &Jewel {
        &self.jewels[idx as usize]
    }

    /// Estimated searched fraction (0.0-1.0), from the position in the top three levels (torso and the next two parts).
    /// Pruning makes the remaining amount unknowable, so the speed is uneven, but it never goes down and is exactly 1.0 when done.
    pub fn progress(&self) -> f32 {
        if self.done {
            return 1.0;
        }
        let (mut p, mut w) = (0.0f64, 1.0f64);
        for d in 0..=self.depth.min(2) {
            let len = self.walk(d).len();
            if len == 0 {
                break;
            }
            let idx = self.levels[d].idx;
            if d < self.depth {
                // At this level the idx-1th branch is being explored deeper
                p += w * idx.saturating_sub(1) as f64 / len as f64;
                w /= len as f64;
            } else {
                p += w * idx.min(len) as f64 / len as f64;
                break;
            }
        }
        p.clamp(0.0, 0.999) as f32
    }

    /// Split work: searchers built with the same conditions share this ticket counter (starting at 0); each takes the next torso
    /// candidate and searches everything below it. When all are done, `merge` collects the results into one.
    /// Call only before the first `run`.
    pub fn share_root(&mut self, counter: &'a core::sync::atomic::AtomicU32) {
        self.root = Some(counter);
    }

    /// Collect the results and non-dominated sets of another split searcher into this one (same conditions required,
    /// since group indices must match). Node counts are added too.
    pub fn merge(&mut self, other: Searcher) {
        self.nodes += other.nodes;
        self.leaves += other.leaves;
        self.deco_nodes += other.deco_nodes;
        self.absorb(&other);
    }

    /// Add the sets another split searcher has found so far to this one (existing sets are skipped).
    /// Exchanging often while running restores the pruning of 'branches that cannot beat sets already found'
    /// to single-searcher strength (split searchers each knew only half and nodes doubled: 2026-11-18 Recomp).
    pub fn absorb(&mut self, other: &Searcher) {
        for f in &other.front {
            if self.pareto && !self.front.iter().any(|g| g.covers(f.free, f.craft, f.def)) {
                self.front.retain(|g| !f.covers(g.free, g.craft, g.def));
                self.front.push(f.clone());
            }
        }
        // Add the representative and grouped sets separately (offer rebuilds the groups). front was collected above, so turn it off briefly.
        // Existing sets are checked with a sorted set: scanning the list per set is hundreds x hundreds and on Recomp
        // took over 100 ms, making the draw skip frames (window flicker, 2026-11-18).
        let key = set_key;
        let mut known: alloc::collections::BTreeSet<_> =
            self.results.iter().flat_map(|h| core::iter::once(h).chain(h.variants.iter())).map(key).collect();
        let pareto = core::mem::replace(&mut self.pareto, false);
        for r in &other.results {
            for x in core::iter::once(r).chain(r.variants.iter()) {
                if known.insert(key(x)) {
                    let mut c = x.clone();
                    c.variants = Vec::new();
                    self.offer(c);
                }
            }
        }
        self.pareto = pareto;
    }

    /// Re-check a set from an earlier search (original armor index per part, original charm index) under this search's conditions and,
    /// if valid, add it to the results in advance. Only before searching (owned-charm search only). Results filled from the start let 'branches that cannot
    /// do better' be cut early. Not added if that armor was removed by this condition (excluded, fixed, rank) or dropped as dominated.
    /// The check is the same computation the search uses to find sets (leaf), so no wrong set gets in. Returns true if added.
    pub fn seed(&mut self, armor: [u16; PARTS], charm: Option<u16>) -> bool {
        if !self.views.is_empty() || self.nodes > 0 || self.done {
            return false;
        }
        let find = |s: &Self, l: usize, item: u16| -> Option<usize> {
            s.groups[l].iter().position(|g| s.members[l][g.first as usize..(g.first + g.len) as usize].contains(&item))
        };
        let mut gis = [0usize; LEVELS];
        for d in 0..PARTS {
            let Some(gi) = find(self, d, armor[self.level_part[d]]) else { return false };
            gis[d] = gi;
        }
        gis[CHARM_LEVEL] = match charm {
            Some(c) => match find(self, CHARM_LEVEL, c) {
                Some(gi) => gi,
                None => return false,
            },
            // 'No charm' is only the empty group used when there are no charms at all
            None if self.groups[CHARM_LEVEL].len() == 1 && self.groups[CHARM_LEVEL][0].len == 0 => 0,
            None => return false,
        };
        let saved = (self.levels, self.chosen, self.depth);
        let before = self.found_count();
        for d in 0..LEVELS {
            self.chosen[d] = gis[d] as u16;
            let (k, def) = (self.groups[d][gis[d]].key, self.groups[d][gis[d]].def);
            let ns = self.apply_key(d, k, def);
            if d == 0 {
                self.rebuild_suffix();
            }
            if d == CHARM_LEVEL {
                self.leaf(&ns);
            } else {
                self.levels[d + 1] = Level { idx: 0, state: ns };
            }
        }
        (self.levels, self.chosen, self.depth) = saved;
        self.nodes = 0;
        self.leaves = 0;
        let added = self.found_count() != before;
        if added {
            // Remember the key of the set just added (so the search does not add it again when it finds the same set)
            for r in &self.results {
                for x in core::iter::once(r).chain(r.variants.iter()) {
                    self.seeded.insert(set_key(x));
                }
            }
        }
        added
    }

    /// Charm signature as text (for saving). Numbers joined with ',' and groups separated by '/'.
    pub fn charm_signature_text(sig: &CharmSignature) -> alloc::string::String {
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        let _ = write!(s, "{}", sig.tal_mono as u8);
        for k in &sig.keys {
            s.push('/');
            let _ = write!(s, "{},{},{},{},{},{}", k.own as u8, k.rare, k.tup as u8, k.slots, k.sec, k.tal);
            for v in k.proj {
                let _ = write!(s, ",{}", v);
            }
            for p in k.sig {
                let _ = write!(s, ",{},{}", p.tree, p.pt);
            }
            for r in k.res {
                let _ = write!(s, ",{}", r);
            }
        }
        s
    }

    /// Text written by charm_signature_text back to a signature. None if the format differs.
    pub fn charm_signature_parse(text: &str) -> Option<CharmSignature> {
        let mut parts = text.split('/');
        let tal_mono = parts.next()? == "1";
        let mut keys = Vec::new();
        for part in parts {
            let v: Vec<i32> = part.split(',').map(|x| x.parse().ok()).collect::<Option<Vec<i32>>>()?;
            if v.len() != 6 + MAXT + 8 + RES {
                return None;
            }
            let mut i = 6;
            let proj = core::array::from_fn(|j| v[i + j] as i16);
            i += MAXT;
            let sig = core::array::from_fn(|j| Pt { tree: v[i + 2 * j] as u16, pt: v[i + 2 * j + 1] as i8 });
            i += 8;
            let res = core::array::from_fn(|j| v[i + j] as i8);
            keys.push(Key { own: v[0] == 1, rare: v[1] as u8, tup: v[2] == 1, slots: v[3] as u8, sec: v[4] as i16, tal: v[5] as i16, proj, sig, res });
        }
        Some(CharmSignature { keys, tal_mono })
    }

    /// Charm signature (CharmSignature) of this search
    pub fn charm_signature(&self) -> CharmSignature {
        let mut keys: Vec<Key> = self.groups[CHARM_LEVEL].iter().map(|g| g.key).collect();
        keys.sort();
        CharmSignature { keys, tal_mono: self.tal_mono }
    }

    /// Number of sets found (including grouped) + non-dominated sets. Used to see whether anything changed (skip exchanges in split work)
    /// A new searcher with the same conditions (for split-work helpers): copies the prepared tables instead of running Searcher::new again.
    /// Found sets, node counts, seeds and the ticket counter are cleared. Call on a searcher that has not run yet.
    /// For callers that stop at the first set (only 'is it possible'): never build the charm chance tables. They only
    /// prune against the worst kept result, which such a search never uses, and building them took most of the time of
    /// the extra-skill checks (one table build per skill, 2026-11-19).
    pub fn skip_chance_tables(&mut self) {
        self.ctables_built = true;
        // Few chance lookups before the first set: a small cache is cheaper to set up (it was 128 KB per search)
        if self.ccache.is_empty() {
            self.ccache_bits = 9;
        }
    }

    pub fn fork(&self) -> Self {
        let mut s = Searcher { results: Vec::new(), front: Vec::new(), seeded: Default::default(), ..self.clone() };
        s.worst = 0;
        s.nodes = 0;
        s.leaves = 0;
        s.deco_nodes = 0;
        s.done = false;
        s.depth = 0;
        s.levels[0].idx = 0;
        s.root = None;
        s
    }

    pub fn found_count(&self) -> usize {
        self.results.iter().map(|r| 1 + r.variants.len()).sum::<usize>() + self.front.len()
    }

    /// Progress (0.0-1.0) inside the torso candidate being searched. None while choosing the torso.
    /// Overall progress in split work = (finished torsos + each searcher's progress inside its torso) / torso count.
    pub fn branch_progress(&self) -> Option<f32> {
        if self.done || self.depth == 0 {
            return None;
        }
        let (mut p, mut w) = (0.0f64, 1.0f64);
        for d in 1..=self.depth.min(3) {
            let len = self.walk(d).len();
            if len == 0 {
                break;
            }
            let idx = self.levels[d].idx;
            if d < self.depth {
                p += w * idx.saturating_sub(1) as f64 / len as f64;
                w /= len as f64;
            } else {
                p += w * idx.min(len) as f64 / len as f64;
                break;
            }
        }
        Some(p.clamp(0.0, 1.0) as f32)
    }

    /// Number of torso candidates (denominator of split-work progress)
    pub fn root_len(&self) -> usize {
        self.walk(0).len()
    }

    /// Search nodes used so far.
    pub fn nodes_used(&self) -> u64 {
        self.nodes
    }

    /// Process about `budget` search nodes and return. On `Paused`, call again to continue.
    pub fn run(&mut self, budget: u64) -> Status {
        if self.done {
            return Status::Done;
        }
        let stop = self.nodes.saturating_add(budget);
        loop {
            let d = self.depth;
            if d == 0 {
                if let Some(c) = self.root {
                    // Split work: take the next torso candidate from the ticket counter
                    self.levels[0].idx = c.fetch_add(1, core::sync::atomic::Ordering::Relaxed) as usize;
                }
            }
            let gi = self.levels[d].idx;
            if gi >= self.walk(d).len() {
                if d == 0 {
                    self.done = true;
                    return Status::Done;
                }
                self.depth -= 1;
                continue;
            }
            self.levels[d].idx += 1;
            self.nodes += 1;
            // Resistance / defense conditions: most armor-level children fail them right after being applied (the last
            // part: 9.7M of 11.7M nodes with five elements and a minimum defense, 2026-11-22). Check them first, cheaply.
            if self.quick && d < PARTS && self.quick_short(d, gi) {
                if self.nodes >= stop {
                    return Status::Paused;
                }
                continue;
            }
            self.chosen[d] = gi as u16;
            let ns = self.apply(d, gi);
            if d == 0 {
                self.rebuild_suffix();
            }
            if d == CHARM_LEVEL && !self.coarse[0].is_empty() {
                self.expand(&ns);
            } else if d == CHARM_LEVEL {
                if self.views.is_empty() {
                    self.leaf(&ns);
                } else {
                    self.leaf_virtual(&ns);
                }
            } else if !self.prune(&ns, d + 1) {
                self.levels[d + 1] = Level { idx: 0, state: ns };
                self.depth = d + 1;
            }
            if self.nodes >= stop {
                return Status::Paused;
            }
        }
    }

    /// Choose walked group gi at level d (a coarse group with resistance conditions)
    fn apply(&mut self, d: usize, gi: usize) -> State {
        let g = &self.walk(d)[gi];
        let (k, def) = (g.key, g.def);
        self.apply_key(d, k, def)
    }

    fn apply_key(&mut self, d: usize, k: Key, def: u16) -> State {
        let mut s = self.levels[d].state;
        s.def += def;
        if d < PARTS && !k.own {
            s.craft += 1;
            s.rare = s.rare.max(k.rare);
        }
        for e in 0..RES {
            s.res[e] += k.res[e] as i16;
        }
        if k.tup {
            for i in 0..self.nt {
                s.cur[i] += self.torso_proj[i];
            }
            s.sec += self.torso_sec;
            s.tal += self.torso_tal;
            s.k += 1;
            return s;
        }
        for i in 0..self.nt {
            s.cur[i] += k.proj[i];
        }
        s.sec += k.sec;
        s.tal += k.tal;
        if d == 0 {
            s.ts = k.slots;
            self.torso_slots = k.slots;
            self.torso_proj = k.proj;
            self.torso_sec = k.sec;
            self.torso_tal = k.tal;
        } else if k.slots > 0 {
            s.n[k.slots as usize] += 1;
            if d == CHARM_LEVEL {
                s.cs = k.slots;
            }
        }
        s
    }

    /// Groups the search walks at level d: coarse groups with resistance conditions (armor levels), else `groups`
    fn walk(&self, d: usize) -> &[Group] {
        if d < PARTS && !self.coarse[d].is_empty() { &self.coarse[d] } else { &self.groups[d] }
    }

    /// Resistance conditions: the whole set is chosen in coarse groups (s from them, its resistance and defense the
    /// maxima); try every combination of their resistance groups and hand each to the leaf. Parts are chosen in level
    /// order, cut by the most resistance and defense the remaining parts can still add.
    fn expand(&mut self, s: &State) {
        if s.def < self.min_def || self.res_short(s, LEVELS) {
            return;
        }
        let picked: [u16; PARTS] = core::array::from_fn(|l| self.chosen[l]);
        // Remaining maxima from part l on (the coarse groups' values) and the state without the armor parts' share
        let mut rem_res = [[0i16; RES]; PARTS + 1];
        let mut rem_def = [0u16; PARTS + 1];
        let mut base = *s;
        for l in (0..PARTS).rev() {
            let c = &self.coarse[l][picked[l] as usize];
            for e in 0..RES {
                rem_res[l][e] = rem_res[l + 1][e] + c.key.res[e] as i16;
            }
            rem_def[l] = rem_def[l + 1] + c.def;
        }
        for e in 0..RES {
            base.res[e] -= rem_res[0][e];
        }
        base.def -= rem_def[0];
        self.expand_part(0, &base, &picked, &rem_res, &rem_def);
        for l in 0..PARTS {
            self.chosen[l] = picked[l];
        }
    }

    fn expand_part(&mut self, l: usize, s: &State, picked: &[u16; PARTS], rem_res: &[[i16; RES]; PARTS + 1], rem_def: &[u16; PARTS + 1]) {
        if l == PARTS {
            if self.views.is_empty() {
                self.leaf(s);
            } else {
                self.leaf_virtual(s);
            }
            return;
        }
        let c = &self.coarse[l][picked[l] as usize];
        let (first, len) = (c.first as usize, c.len as usize);
        for k in first..first + len {
            let fi = self.coarse_fine[l][k];
            let g = &self.groups[l][fi as usize];
            let mut ns = *s;
            ns.def += g.def;
            for e in 0..RES {
                ns.res[e] += g.key.res[e] as i16;
            }
            if ns.def + rem_def[l + 1] < self.min_def
                || (0..RES).any(|e| self.min_res[e].is_some_and(|m| ns.res[e] + rem_res[l + 1][e] < m as i16))
            {
                continue;
            }
            self.nodes += 1;
            self.chosen[l] = fi;
            self.expand_part(l + 1, &ns, picked, rem_res, rem_def);
        }
    }

    fn rebuild_suffix(&mut self) {
        for d in (1..LEVELS).rev() {
            let tup = self.lvl_tup[d];
            for i in 0..self.nt {
                let m = if tup { self.lvl_max[d][i].max(self.torso_proj[i]) } else { self.lvl_max[d][i] };
                self.suf_m[d][i] = self.suf_m[d + 1][i] + m;
            }
            let sec = if tup { self.lvl_sec[d].max(self.torso_sec) } else { self.lvl_sec[d] };
            let tal = if tup { self.lvl_tal[d].max(self.torso_tal) } else { self.lvl_tal[d] };
            self.suf_sec[d] = self.suf_sec[d + 1] + sec;
            self.suf_tal[d] = self.suf_tal[d + 1] + tal;
            self.suf_slots[d] = self.suf_slots[d + 1] + self.lvl_slots[d];
            self.suf_tup[d] = self.suf_tup[d + 1] + tup as u8;
            // Torso-up part: torso points once more + one more multiplier for decorations in torso slots (as many as torso slots)
            let mut v = self.lvl_val[d];
            if tup {
                let mut t = VAL_SCALE * self.torso_slots as u32;
                for i in 0..self.nt {
                    t += self.rate[i] * (self.torso_proj[i] as i32).max(0).min(self.gcap[i]) as u32;
                }
                v = v.max(t);
            }
            self.suf_val[d] = self.suf_val[d + 1] + v;
        }
        if !self.lvl_valr.is_empty() {
            let mut t = VAL_SCALE as i64 * self.torso_slots as i64;
            for i in 0..self.nt {
                t += self.rate[i] as i64 * (self.torso_proj[i] as i32).max(0).min(self.gcap[i]) as i64;
            }
            for x in 0..self.lvl_valr.len() {
                for d in (1..LEVELS).rev() {
                    let tup = if self.lvl_tupr[x][d] > NEG_VAL { t + self.lvl_tupr[x][d] } else { NEG_VAL };
                    self.suf_valr[x][d] = self.suf_valr[x][d + 1] + self.lvl_valr[x][d].max(tup);
                }
            }
        }
    }

    /// Resistance / defense conditions in the slot-equivalent bounds (lvl_valr): the value the remaining parts (from level
    /// d, the charm level included) can still give to the targets while also giving the missing resistance and defense.
    /// A set needs both from the same parts, so sum(value) <= sum(value + w x amount) - w x missing for every weight w;
    /// the smallest over the weights. None when nothing is missing (then suf_val[d] is the bound).
    fn cond_value(&self, s: &State, d: usize) -> Option<i64> {
        if self.lvl_valr.is_empty() {
            return None;
        }
        let (mut set, mut gap) = (0usize, 0i64);
        for e in 0..RES {
            if let Some(m) = self.min_res[e] {
                let g = m as i64 - s.res[e] as i64;
                if g > 0 {
                    set |= 1 << e;
                    gap += g;
                }
            }
        }
        let def_gap = (self.min_def as i64 - s.def as i64).max(0);
        if def_gap > 0 {
            set |= 1 << RES;
        }
        if set == 0 {
            return None;
        }
        // With missing defense also the subset without it (the weight pairs give defense no zero weight)
        let x = self.cond_idx[set] as usize;
        let full = self.cond_w.iter().enumerate().map(|(w, &(wr, wd))| self.suf_valr[w * self.cond_n + x][d] - wr * gap - wd * def_gap);
        let res_only = set & !(1 << RES);
        let y = self.cond_idx[res_only] as usize;
        let part = (def_gap > 0 && res_only != 0)
            .then(|| self.cond_w.iter().enumerate().map(move |(w, &(wr, _))| self.suf_valr[w * self.cond_n + y][d] - wr * gap))
            .into_iter()
            .flatten();
        full.chain(part).min()
    }

    /// The remaining parts' value for the targets: suf_val[d], or less with resistance / defense conditions (cond_value)
    fn remaining_value(&self, s: &State, d: usize) -> i64 {
        let v = self.suf_val[d] as i64;
        self.cond_value(s, d).map_or(v, |r| r.min(v))
    }

    /// True if the targets cannot be reached even when levels from d on are filled most favorably.
    /// True if the resistance condition cannot be met even when the remaining parts give the most resistance. d = next level to choose.
    /// Group gi at armor level d cannot meet the resistance or defense conditions even with the most the levels after it
    /// can add (the same test res_short / prune make after applying it)
    fn quick_short(&self, d: usize, gi: usize) -> bool {
        let g = &self.walk(d)[gi];
        let s = &self.levels[d].state;
        if s.def + g.def + self.suf_def[d + 1] < self.min_def {
            return true;
        }
        (0..RES).any(|e| self.min_res[e].is_some_and(|m| s.res[e] + g.key.res[e] as i16 + self.suf_res[d + 1][e] < m as i16))
    }

    fn res_short(&self, s: &State, d: usize) -> bool {
        (0..RES).any(|e| self.min_res[e].is_some_and(|m| s.res[e] + self.suf_res[d][e] < m as i16))
    }

    fn prune(&self, s: &State, d: usize) -> bool {
        if self.res_short(s, d) {
            return true;
        }
        if !self.views.is_empty() {
            return self.prune_virtual(s, d);
        }
        let bonus = if s.sec + self.suf_sec[d] >= 10 { 2 } else { 0 };
        let mut need = 0u32;
        for i in 0..self.nt {
            let gap = self.need[i] - bonus - s.cur[i] - self.suf_m[d][i];
            if gap > 0 {
                if gap as usize >= MAXD {
                    return true;
                }
                let c = self.mincost[i][gap as usize];
                if c == INF {
                    return true;
                }
                need += c as u32;
            }
        }
        let other = (s.n[1] + 2 * s.n[2] + 3 * s.n[3] + self.suf_slots[d]) as u32;
        let mmax = 1 + s.k as u32 + self.suf_tup[d] as u32;
        let cx = self.charm_extra(s, d);
        if need > other + s.ts as u32 * mmax + cx {
            return true;
        }
        // The remaining parts' value, once per node (with conditions it takes a few dozen lookups)
        let rv = self.remaining_value(s, d);
        if self.joint_short(s, d, bonus, rv) {
            return true;
        }
        // Free-slot upper bound: the smaller of the per-target estimate and the slot-equivalent bound
        let joint_free = self.joint_free(s, d, bonus, rv);
        // Defense bound: the best remaining parts still cannot reach the minimum defense, or (defense priority, full results)
        // cannot beat the worst result's defense. Defense ranks first in that priority, so such a branch cannot enter the top
        // results; it is kept only if it can still be a non-dominated set. Without this, defense priority searched
        // over 100M nodes for two skills (2026-11-19).
        let max_def = s.def + self.suf_def[d];
        if max_def < self.min_def {
            return true;
        }
        if self.priority == 1 && self.results.len() >= self.max_results && max_def < self.results[self.worst].def {
            let free = (other + s.ts as u32).min(joint_free).min(255) as u8;
            return self.front_hopeless(free, s.craft, max_def);
        }
        // Free-slot priority, or rarity priority on a branch whose rarity is already at or above the worst: cut by free slots.
        // Rarity only goes up as parts are chosen, so the current value is a lower bound.
        let worst_rare = self.results.get(self.worst).map_or(0, |w| w.rare);
        let slot_bound = self.priority == 0 || (self.priority == 2 && s.rare >= worst_rare);
        if slot_bound && self.results.len() >= self.max_results {
            let used = need.div_ceil((if s.ts > 0 { mmax } else { 1 }).max(if cx > 0 { 2 } else { 1 }));
            let rare_lost = self.priority == 2 && s.rare > worst_rare;
            let est = (other + s.ts as u32).saturating_sub(used).min(joint_free);
            if rare_lost || est < self.results[self.worst].free as u32 {
                // Keep going if it can still be non-dominated even though it cannot enter the top results.
                let free = est.min(255) as u8;
                return self.front_hopeless(free, s.craft, s.def + self.suf_def[d]);
            }
        }
        false
    }

    fn leaf(&mut self, s: &State) {
        self.leaves += 1;
        if s.def < self.min_def || self.res_short(s, LEVELS) {
            return;
        }
        let secret = s.sec >= 10;
        let double = s.tal + if secret && s.tal != 0 { 2 } else { 0 } >= 10;
        let cg = &self.groups[CHARM_LEVEL][self.chosen[CHARM_LEVEL] as usize];
        let m = 1 + s.k as i16;
        let mut req = [0i16; MAXT];
        let mut need = 0u32;
        for i in 0..self.nt {
            req[i] = self.need[i] - if secret { 2 } else { 0 };
            let cur = s.cur[i] + if double { cg.key.proj[i] } else { 0 };
            let gap = req[i] - cur;
            if gap > 0 {
                if gap as usize >= MAXD || self.mincost[i][gap as usize] == INF {
                    return;
                }
                need += self.mincost[i][gap as usize] as u32;
            }
        }
        let other_total = s.n[1] + 2 * s.n[2] + 3 * s.n[3];
        // Talisman boost: the charm's slots become a separate container at x2
        let cs = if double { s.cs } else { 0 };
        if need > other_total as u32 + s.ts as u32 * m as u32 + cs as u32 {
            return;
        }
        let total = other_total + s.ts;
        // A leaf that cannot enter the top results still gets its decorations solved if it can be non-dominated.
        // Rarity priority: if this leaf's rarity is below the worst, no bound; if equal, by free slots; if higher, it cannot enter the top.
        let worst_rare = self.results.get(self.worst).map_or(0, |w| w.rare);
        let full = (self.priority == 0 || (self.priority == 2 && s.rare >= worst_rare))
            && self.results.len() >= self.max_results
            && self.front_hopeless(total, s.craft, s.def);
        if full && (total < self.results[self.worst].free || (self.priority == 2 && s.rare > worst_rare)) {
            return;
        }
        let best = if full { total - self.results[self.worst].free + 1 } else { total + 1 };

        let mut d = Deco {
            totals: [0; NT],
            last: [0; NT],
            watch: [0; 64],
            nw: 0,
            stack: [Placed::default(); MAXJ],
            ns: 0,
            c: [0; 4],
            tc: 0,
            n: s.n,
            ts: s.ts,
            m,
            cs: 0,
            cc: 0,
            req,
            neg_thr: -10 - if secret { 2 } else { 0 },
            other_total,
            best,
            best_stack: [Placed::default(); MAXJ],
            best_n: 0,
            found: false,
            nodes: 0,
        };
        for l in 0..PARTS {
            let g = &self.groups[l][self.chosen[l] as usize];
            if g.key.tup {
                continue;
            }
            let a = &self.armor[self.members[l][g.first as usize] as usize];
            let mult = if l == 0 { m } else { 1 };
            for sk in a.skills.iter().filter(|x| x.tree != 0) {
                d.totals[sk.tree as usize] += sk.pt as i16 * mult;
            }
        }
        if cs > 0 {
            d.n[cs as usize] -= 1;
            d.other_total -= cs;
            d.cs = cs;
        }
        if cg.len > 0 {
            let c = &self.charms[self.members[CHARM_LEVEL][cg.first as usize] as usize];
            for sk in c.skills.iter().filter(|x| x.tree != 0) {
                d.totals[sk.tree as usize] += sk.pt as i16 * if double { 2 } else { 1 };
            }
        }
        if self.forbid {
            for t in 1..NT {
                if self.neg[t] && d.totals[t] <= d.neg_thr && d.nw < 32 {
                    d.watch[d.nw] = t as u16;
                    d.nw += 1;
                }
            }
        }

        self.solve(&mut d, 0);
        self.deco_nodes += d.nodes;
        self.nodes += d.nodes;
        if !d.found {
            return;
        }
        for p in &d.best_stack[..d.best_n] {
            let j = &self.jewels[p.jewel as usize];
            let mult = if p.torso { m } else if p.charm { 2 } else { 1 };
            d.totals[j.pos.tree as usize] += j.pos.pt as i16 * mult;
            if j.neg.tree != 0 {
                d.totals[j.neg.tree as usize] += j.neg.pt as i16 * mult;
            }
        }
        let bonus = (TREE_TORSO_UP as usize + 1..NT)
            .filter(|&t| self.t2i[t] == 255 && d.totals[t] != 0 && d.totals[t] + if secret { 2 } else { 0 } >= 10)
            .count() as u8;
        let mut groups = [0u16; PARTS];
        for l in 0..PARTS {
            groups[self.level_part[l]] = self.chosen[l];
        }
        let res = SetResult {
            groups,
            charm: self.chosen[CHARM_LEVEL],
            jewels: d.best_stack,
            nj: d.best_n as u8,
            free: total - d.best,
            bonus,
            def: s.def,
            craft: s.craft,
            rare: s.rare,
            need: Charm::default(),
            chance: 0,
            variants: Vec::new(),
        };
        self.offer(res);
    }

    /// Build the chance bound tables (chance_short): once, when the results first fill up (they are not used before).
    /// So small searches that stop at the first set (cause analysis) never build them (building was heavy on Recomp).
    fn build_ctables(&mut self) {
        self.ctables_built = true;
        for (fi, f) in [(0usize, 1i32), (1, 2)] {
            if self.views.is_empty() || (fi == 1 && !self.tal_possible) {
                continue;
            }
            let mut ctable: Vec<(u32, u64)> = Vec::new();
            let (rate, gcap, vmax1, vmax2, nt) = (self.rate, self.gcap, self.vmax1, self.vmax2, self.nt);
            let pv = |i: usize, p: u8| rate[i] * ((p as i32) * f).min(gcap[i]) as u32;
            // Charms with more points than the target have the same value and are only harder to get: look only up to the target
            let pcap = |i: usize, m: u8| m.min((gcap[i] as u32).div_ceil(f as u32).min(255) as u8);
            let mut s1: Vec<Option<(usize, u8)>> = alloc::vec![None];
            let mut s2: Vec<Option<(usize, u8)>> = alloc::vec![None];
            for i in 0..nt {
                for p in 1..=pcap(i, vmax1[i]) {
                    s1.push(Some((i, p)));
                }
                for p in 1..=pcap(i, vmax2[i]) {
                    s2.push(Some((i, p)));
                }
            }
            for sl in 0..=3u8 {
                for &a in &s1 {
                    for &b in &s2 {
                        if matches!((a, b), (Some((i, _)), Some((j, _))) if i == j) {
                            continue;
                        }
                        let c = self.views.iter().map(|k| k.chance(a, b, sl)).fold(0.0, f64::max);
                        if c <= 0.0 {
                            continue;
                        }
                        let v = VAL_SCALE * sl as u32 * f as u32 + a.map_or(0, |(i, p)| pv(i, p)) + b.map_or(0, |(j, p)| pv(j, p));
                        ctable.push((v, (c * 1e15) as u64));
                    }
                }
            }
            ctable.sort_unstable();
            let mut best = 0u64;
            for e in ctable.iter_mut().rev() {
                best = best.max(e.1);
                e.1 = best;
            }
            self.ctables[fi] = ctable;
        }
    }

    fn offer(&mut self, res: SetResult) {
        if !self.seeded.is_empty() && self.nodes > 0 && self.seeded.contains(&set_key(&res)) {
            return; // Set already added by seed
        }
        let (p, s) = (self.priority, self.user_priority);
        if self.pareto && !self.front.iter().any(|f| f.covers(res.free, res.craft, res.def)) {
            self.front.retain(|f| !res.covers(f.free, f.craft, f.def));
            let mut copy = res.clone();
            copy.variants = Vec::new();
            self.front.push(copy);
        }
        if self.group_similar {
            if let Some(i) = self.results.iter().position(|r| similar(r, &res)) {
                // When the representative changes, keep only grouped sets differing from the new one in one part, and re-offer the rest.
                let mut again = Vec::new();
                if res.rank(p, s) > self.results[i].rank(p, s) {
                    // If the new set is better it becomes the representative and the old one moves into its group.
                    let mut old = core::mem::replace(&mut self.results[i], res);
                    let rest = core::mem::take(&mut old.variants);
                    self.results[i].variants.push(old);
                    for v in rest {
                        if similar(&self.results[i], &v) {
                            self.results[i].variants.push(v);
                        } else {
                            again.push(v);
                        }
                    }
                } else {
                    self.results[i].variants.push(res);
                }
                let head = &mut self.results[i];
                head.variants.sort_by(|a, b| b.rank(p, s).cmp(&a.rank(p, s)));
                head.variants.truncate(MAX_VARIANTS);
                if self.results.len() >= self.max_results {
                    self.worst = (0..self.results.len()).min_by_key(|&i| self.results[i].rank(p, s)).unwrap_or(0);
                }
                for v in again {
                    self.offer(v);
                }
                return;
            }
        }
        if self.results.len() < self.max_results {
            self.results.push(res);
        } else if res.rank(self.priority, self.user_priority) > self.results[self.worst].rank(self.priority, self.user_priority) {
            self.results[self.worst] = res;
        } else {
            return;
        }
        if self.results.len() >= self.max_results {
            self.worst = (0..self.results.len()).min_by_key(|&i| self.results[i].rank(self.priority, self.user_priority)).unwrap_or(0);
            if !self.ctables_built && !self.views.is_empty() {
                self.build_ctables();
            }
        }
    }

    /// Decoration placement. Pick one violated condition and branch only on decorations raising that tree.
    /// Fixing a violated condition requires a decoration of that tree, so no solution is missed.
    /// Within one tree the choice index never decreases, removing duplicates that differ only in order.
    fn solve(&self, d: &mut Deco, used: u8) {
        d.nodes += 1;
        if used >= d.best {
            return;
        }
        let mut need = 0u32;
        let mut viol = 0u16;
        for i in 0..self.nt {
            let gap = d.req[i] - d.totals[self.tt[i] as usize];
            if gap > 0 {
                if viol == 0 {
                    viol = self.tt[i];
                }
                if gap as usize >= MAXD || self.mincost[i][gap as usize] == INF {
                    return;
                }
                need += self.mincost[i][gap as usize] as u32;
            }
        }
        if viol == 0 && self.forbid {
            viol = d.watch[..d.nw].iter().copied().find(|&w| d.totals[w as usize] <= d.neg_thr).unwrap_or(0);
        }
        if viol == 0 {
            d.best = used;
            d.best_stack = d.stack;
            d.best_n = d.ns;
            d.found = true;
            return;
        }
        let other_used = d.c[1] + 2 * d.c[2] + 3 * d.c[3];
        let torso_left = (d.ts - d.tc) as u32;
        let charm_left = (d.cs - d.cc) as u32;
        if need > (d.other_total - other_used) as u32 + torso_left * d.m as u32 + charm_left * 2 {
            return;
        }
        let mmax = (if torso_left > 0 { d.m as u32 } else { 1 }).max(if charm_left > 0 { 2 } else { 1 });
        if used as u32 + need.div_ceil(mmax) >= d.best as u32 {
            return;
        }
        let (a, b) = (self.jstart[viol as usize] as usize, self.jstart[viol as usize + 1] as usize);
        let saved = d.last[viol as usize];
        for ji in a..b {
            let j = self.jewels[ji];
            // Locations: 0 torso (x m), 1 charm under talisman boost (x2), 2 any other slot
            for loc in 0..3u8 {
                let e = ((ji - a) * 3) as u8 + loc;
                if e < saved || d.ns >= MAXJ {
                    continue;
                }
                let torso = loc == 0;
                let charm = loc == 1;
                let mult = if torso {
                    if d.tc + j.size > d.ts {
                        continue;
                    }
                    d.tc += j.size;
                    d.m
                } else if charm {
                    if d.cc + j.size > d.cs {
                        continue;
                    }
                    d.cc += j.size;
                    2
                } else {
                    d.c[j.size as usize] += 1;
                    if !fits(&d.c, &d.n) {
                        d.c[j.size as usize] -= 1;
                        continue;
                    }
                    1
                };
                d.totals[j.pos.tree as usize] += j.pos.pt as i16 * mult;
                let watched = self.forbid && j.neg.tree != 0 && self.neg[j.neg.tree as usize] && d.nw < d.watch.len();
                if j.neg.tree != 0 {
                    d.totals[j.neg.tree as usize] += j.neg.pt as i16 * mult;
                }
                if watched {
                    d.watch[d.nw] = j.neg.tree;
                    d.nw += 1;
                }
                d.last[viol as usize] = e;
                d.stack[d.ns] = Placed { jewel: ji as u16, torso, charm };
                d.ns += 1;

                self.solve(d, used + j.size);

                d.ns -= 1;
                if watched {
                    d.nw -= 1;
                }
                d.totals[j.pos.tree as usize] -= j.pos.pt as i16 * mult;
                if j.neg.tree != 0 {
                    d.totals[j.neg.tree as usize] -= j.neg.pt as i16 * mult;
                }
                if torso {
                    d.tc -= j.size;
                } else if charm {
                    d.cc -= j.size;
                } else {
                    d.c[j.size as usize] -= 1;
                }
            }
        }
        d.last[viol as usize] = saved;
    }

    // ---- Needed-charm search ----

    /// Min slots to cover tree i's shortfall gap with decorations. NOCOST if impossible.
    fn cost(&self, i: usize, gap: i16) -> u32 {
        if gap <= 0 {
            0
        } else if gap as usize >= MAXD || self.mincost[i][gap as usize] == INF {
            NOCOST
        } else {
            self.mincost[i][gap as usize] as u32
        }
    }

    /// Chance to get the required charm or better (max over charm kinds). The same lookups repeat, so they are cached in a small table.
    fn vchance(&mut self, i: Option<(usize, u8)>, j: Option<(usize, u8)>, s: u8) -> f64 {
        let pack = |x: Option<(usize, u8)>| x.map_or(0x1ff, |(t, p)| (t as u32) << 5 | (p as u32 & 31));
        let key = pack(i) << 11 | pack(j) << 2 | s as u32;
        let slot = (key.wrapping_mul(2654435761) >> (32 - self.ccache_bits)) as usize;
        if self.ccache.is_empty() {
            self.ccache = alloc::vec![(u32::MAX, 0.0); 1 << self.ccache_bits];
        }
        if self.ccache[slot].0 == key {
            return self.ccache[slot].1;
        }
        let v = self.views.iter().map(|k| k.chance(i, j, s)).fold(0.0, f64::max);
        self.ccache[slot] = (key, v);
        v
    }

    /// Pruning for needed-charm mode. Uses the fact that a charm can help at most two trees.
    fn prune_virtual(&self, s: &State, d: usize) -> bool {
        let bonus = if s.sec + self.suf_sec[d] >= 10 { 2 } else { 0 };
        let f: i16 = 1 + self.charm_double(s, d) as i16;
        let mut need = 0u32;
        let mut saving = [0u32; 2];
        let mut forced = 0;
        for i in 0..self.nt {
            let gap = self.need[i] - bonus - s.cur[i] - self.suf_m[d][i];
            if gap <= 0 {
                continue;
            }
            let full = self.cost(i, gap);
            let helped = self.cost(i, gap - f * self.vmax1[i].max(self.vmax2[i]) as i16);
            if full == NOCOST {
                // A tree decorations alone cannot cover: one charm slot must be used.
                forced += 1;
                if helped == NOCOST || forced > 2 {
                    return true;
                }
                need += helped;
            } else {
                need += full;
                let gain = full - helped;
                if gain > saving[0] {
                    saving = [gain, saving[0]];
                } else if gain > saving[1] {
                    saving[1] = gain;
                }
            }
        }
        need -= saving[..2 - forced].iter().sum::<u32>();
        let other = (s.n[1] + 2 * s.n[2] + 3 * s.n[3] + self.suf_slots[d]) as u32;
        let mmax = 1 + s.k as u32 + self.suf_tup[d] as u32;
        if need > other + s.ts as u32 * mmax + self.charm_extra(s, d) {
            return true;
        }
        let rv = self.remaining_value(s, d);
        self.joint_short(s, d, bonus, rv) || self.chance_short(s, d, bonus, self.charm_double(s, d), rv)
    }

    /// Charm already chosen (in s.n) and talisman boost possible: its slots count once more (x2). Before the charm level it is in suf_val.
    fn chosen_charm_extra(&self, s: &State, d: usize) -> u32 {
        if d > CHARM_LEVEL { self.charm_extra(s, d) } else { 0 }
    }

    /// Can talisman boost (charm points x2) activate on this branch: current talisman points + max of remaining parts + secret bonus 2.
    /// If it cannot, the charm slot is valued at x1 (1: can activate).
    fn charm_double(&self, s: &State, d: usize) -> usize {
        (self.tal_possible && s.tal + self.suf_tal[d] + 2 >= 10) as usize
    }

    /// Extra decoration capacity (slot units) of the charm slots under talisman boost: they count once more.
    /// Charm not chosen yet (d <= CHARM_LEVEL): the most slots a charm can have at this level.
    fn charm_extra(&self, s: &State, d: usize) -> u32 {
        if self.charm_double(s, d) == 0 {
            0
        } else if d > CHARM_LEVEL {
            s.cs as u32
        } else {
            self.lvl_slots[CHARM_LEVEL] as u32
        }
    }

    /// Amount to subtract from suf_val[d]: on branches where talisman boost cannot activate, the charm slot at x1 value (0 if d is past the charm level)
    fn charm_adj(&self, s: &State, d: usize) -> u32 {
        if d > CHARM_LEVEL {
            return 0;
        }
        self.lvl_val[CHARM_LEVEL] - self.charm_val[self.charm_double(s, d)].min(self.lvl_val[CHARM_LEVEL])
    }

    /// Needed-charm mode with full results: true if, even with the best remaining parts, the needed charm value (in slots) is so high
    /// that the easiest such charm cannot beat the worst result's chance. d = next level to choose.
    /// The leaf (leaf_virtual) also only returns charms more likely than the worst, so cutting here gives the same results.
    /// rv = remaining_value(s, d)
    fn chance_short(&self, s: &State, d: usize, bonus: i16, fi: usize, rv: i64) -> bool {
        let ctable = &self.ctables[fi];
        if self.results.len() < self.max_results || ctable.is_empty() {
            return false;
        }
        let floor = self.results[self.worst].chance;
        let mut deficit = 0i64;
        for i in 0..self.nt {
            let gap = self.need[i] - bonus - s.cur[i];
            if gap > 0 {
                deficit += self.rate[i] as i64 * gap as i64;
            }
        }
        let slots = (s.n[1] + 2 * s.n[2] + 3 * s.n[3]) as i64 + s.ts as i64 * (1 + s.k as i64) + self.chosen_charm_extra(s, d) as i64;
        // Value of the remaining armor parts (excluding the charm slot; charms give no resistance)
        let armor = rv - self.lvl_val[CHARM_LEVEL] as i64;
        let want = deficit - VAL_SCALE as i64 * slots - armor;
        let at = if want <= 0 { 0 } else { ctable.partition_point(|e| (e.0 as i64) < want) };
        ctable.get(at).map_or(0, |e| e.1) <= floor
    }

    /// Upper bound on free slots with the best remaining parts (slot-equivalent): (current slots + max of remaining parts - missing points
    /// converted) / scale. Torso slots count times the multiplier (decoration points are multiplied), so this is >= the real free slots (an upper bound).
    /// rv = remaining_value(s, d)
    fn joint_free(&self, s: &State, d: usize, bonus: i16, rv: i64) -> u32 {
        let mut deficit = 0u64;
        for i in 0..self.nt {
            let gap = self.need[i] - bonus - s.cur[i];
            if gap > 0 {
                deficit += self.rate[i] as u64 * gap as u64;
            }
        }
        let slots = (s.n[1] + 2 * s.n[2] + 3 * s.n[3]) as i64 + s.ts as i64 * (1 + s.k as i64) + self.chosen_charm_extra(s, d) as i64;
        let cap = VAL_SCALE as i64 * slots + rv - self.charm_adj(s, d) as i64;
        ((cap - deficit as i64).max(0) / VAL_SCALE as i64).min(u32::MAX as i64) as u32
    }

    /// Slot-equivalent bound: if the missing target points, converted to slot cost at the cheapest decoration ratio, exceed the current slots
    /// plus the sum of the max 'slots + target points' the remaining parts can give, no choice of the remaining parts works.
    /// d = next level to choose (torso is already chosen). The charm level is included in the remaining parts (max virtual charm when unowned).
    /// rv = remaining_value(s, d)
    fn joint_short(&self, s: &State, d: usize, bonus: i16, rv: i64) -> bool {
        let mut deficit = 0u64;
        for i in 0..self.nt {
            let gap = self.need[i] - bonus - s.cur[i];
            if gap > 0 {
                deficit += self.rate[i] as u64 * gap as u64;
            }
        }
        let slots = (s.n[1] + 2 * s.n[2] + 3 * s.n[3]) as i64 + s.ts as i64 * (1 + s.k as i64) + self.chosen_charm_extra(s, d) as i64;
        deficit as i64 > VAL_SCALE as i64 * slots + rv - self.charm_adj(s, d) as i64
    }

    /// Decoration solver state with only the armor applied.
    fn deco_armor(&self, s: &State, m: i16, req: [i16; MAXT], secret: bool) -> Deco {
        let other_total = s.n[1] + 2 * s.n[2] + 3 * s.n[3];
        let mut d = Deco {
            totals: [0; NT],
            last: [0; NT],
            watch: [0; 64],
            nw: 0,
            stack: [Placed::default(); MAXJ],
            ns: 0,
            c: [0; 4],
            tc: 0,
            n: s.n,
            ts: s.ts,
            m,
            cs: 0,
            cc: 0,
            req,
            neg_thr: -10 - if secret { 2 } else { 0 },
            other_total,
            best: other_total + s.ts + 1,
            best_stack: [Placed::default(); MAXJ],
            best_n: 0,
            found: false,
            nodes: 0,
        };
        for l in 0..PARTS {
            let g = &self.groups[l][self.chosen[l] as usize];
            if g.key.tup {
                continue;
            }
            let a = &self.armor[self.members[l][g.first as usize] as usize];
            let mult = if l == 0 { m } else { 1 };
            for sk in a.skills.iter().filter(|x| x.tree != 0) {
                d.totals[sk.tree as usize] += sk.pt as i16 * mult;
            }
        }
        d
    }

    /// With all five armor parts fixed, find the easiest charm that completes the targets.
    /// Candidates are chosen quickly by slot arithmetic; only those verified by a real decoration placement become results.
    fn leaf_virtual(&mut self, s: &State) {
        type Cand = VCand;
        fn keep(list: &mut [Cand; 6], c: Cand) {
            if let Some(at) = (0..6).find(|&k| c.0 > list[k].0) {
                list.copy_within(at..5, at + 1);
                list[at] = c;
            }
        }

        self.leaves += 1;
        if s.def < self.min_def || self.res_short(s, LEVELS) {
            return;
        }
        let secret = s.sec >= 10;
        let double = s.tal + if secret && s.tal != 0 { 2 } else { 0 } >= 10;
        let rv = self.remaining_value(s, CHARM_LEVEL);
        if self.chance_short(s, CHARM_LEVEL, if secret { 2 } else { 0 }, (double && self.tal_possible) as usize, rv) {
            return;
        }
        let f: i16 = if double { 2 } else { 1 };
        let m = 1 + s.k as i16;
        let nt = self.nt;
        let mut req = [0i16; MAXT];
        let mut gap = [0i16; MAXT];
        let mut full = [0u32; MAXT];
        let mut all = 0u32;
        for i in 0..nt {
            req[i] = self.need[i] - if secret { 2 } else { 0 };
            gap[i] = (req[i] - s.cur[i]).max(0);
            full[i] = self.cost(i, gap[i]);
            all += full[i];
        }
        let cap0 = (s.n[1] + 2 * s.n[2] + 3 * s.n[3]) as u32 + s.ts as u32 * m as u32;
        let floor = if self.results.len() >= self.max_results { self.results[self.worst].chance } else { 0 };
        let scale = |p: f64| (p * 1e15) as u64;

        let mut cand: [Cand; 6] = [(0, 0, None, None); 6];
        for sl in 0..=3u8 {
            let cap = cap0 + sl as u32 * f as u32;
            if all <= cap {
                let c = scale(self.vchance(None, None, sl));
                if c > floor {
                    keep(&mut cand, (c, sl, None, None));
                }
            }
            for i in 0..nt {
                if gap[i] == 0 {
                    continue;
                }
                let base = all - full[i];
                if base <= cap {
                    // Charm using one slot only: the first or the second
                    for first in [true, false] {
                        let maxp = if first { self.vmax1[i] } else { self.vmax2[i] };
                        if let Some(p) = (1..=maxp).find(|&p| self.cost(i, gap[i] - f * p as i16) <= cap - base) {
                            let (a, b) = if first { (Some((i, p)), None) } else { (None, Some((i, p))) };
                            let c = scale(self.vchance(a, b, sl));
                            if c > floor {
                                keep(&mut cand, (c, sl, a, b));
                            }
                        }
                    }
                }
                for j in 0..nt {
                    if j == i || gap[j] == 0 || all - full[i] - full[j] > cap {
                        continue;
                    }
                    let room = cap - (all - full[i] - full[j]);
                    for pi in 1..=self.vmax1[i] {
                        let used = self.cost(i, gap[i] - f * pi as i16);
                        if used > room {
                            continue;
                        }
                        if let Some(pj) = (1..=self.vmax2[j]).find(|&p| self.cost(j, gap[j] - f * p as i16) <= room - used) {
                            let (a, b) = (Some((i, pi)), Some((j, pj)));
                            let c = scale(self.vchance(a, b, sl));
                            if c > floor {
                                keep(&mut cand, (c, sl, a, b));
                            }
                        }
                        if used == 0 {
                            break;
                        }
                    }
                }
            }
        }

        if cand[0].0 == 0 {
            return;
        }
        let contrib = |a: VSide, b: VSide| {
            let mut v = [0i16; MAXT];
            for (t, p) in [a, b].into_iter().flatten() {
                v[t] += f * p as i16;
            }
            v
        };
        let mut failed: Vec<(u8, [i16; MAXT])> = Vec::new();
        for c in cand {
            if c.0 == 0 {
                continue;
            }
            if self.verify_virtual(s, m, req, secret, f, c) {
                return;
            }
            failed.push((c.1, contrib(c.2, c.3)));
        }
        // Every candidate picked by the quick estimate failed verification (decoration side effects or slot sizes).
        // Collect every possible charm and verify by highest chance first. A charm whose points and slots are all equal to or lower
        // than a failed charm cannot work and is skipped. If any charm works it is always found.
        let mut sides: Vec<VSide> = alloc::vec![None];
        let mut sides2: Vec<VSide> = alloc::vec![None];
        for i in 0..nt {
            if gap[i] == 0 {
                continue;
            }
            // Also try values above the shortfall: sometimes the charm must cover negative decoration points (e.g. a decoration for another target giving -1 here)
            // (confirmed with needed_charm seed 0x1234567890abcdef+22 on 2026-11-18).
            for p in 1..=self.vmax1[i] {
                sides.push(Some((i, p)));
            }
            for p in 1..=self.vmax2[i] {
                sides2.push(Some((i, p)));
            }
        }
        let mut pool: Vec<VCand> = Vec::new();
        for sl in 0..=3u8 {
            let cap = cap0 + sl as u32 * f as u32;
            for &a in &sides {
                for &b in &sides2 {
                    if matches!((a, b), (Some((i, _)), Some((j, _))) if i == j) {
                        continue;
                    }
                    let v = contrib(a, b);
                    // Necessary condition by decoration size sum only
                    let cost: u32 = (0..nt).map(|t| self.cost(t, gap[t] - v[t])).sum();
                    if cost > cap {
                        continue;
                    }
                    let c = scale(self.vchance(a, b, sl));
                    if c > floor {
                        pool.push((c, sl, a, b));
                    }
                }
            }
        }
        pool.sort_by(|x, y| y.0.cmp(&x.0));
        for c in pool {
            let v = contrib(c.2, c.3);
            if failed.iter().any(|(fs, fv)| *fs >= c.1 && (0..nt).all(|t| fv[t] >= v[t])) {
                continue;
            }
            if self.verify_virtual(s, m, req, secret, f, c) {
                return;
            }
            failed.push((c.1, v));
        }
    }

    /// Verify one charm candidate with a real decoration placement and add it as a result if it works.
    fn verify_virtual(&mut self, s: &State, m: i16, req: [i16; MAXT], secret: bool, f: i16, c: VCand) -> bool {
        let (chance, sl, a, b) = c;
        let mut d = self.deco_armor(s, m, req, secret);
        for (t, p) in [a, b].into_iter().flatten() {
            d.totals[self.tt[t] as usize] += p as i16 * f;
        }
        let mut total = d.other_total + d.ts;
        if sl > 0 {
            total += sl;
            d.best += sl;
            if f == 2 {
                // Talisman boost: the charm's slots hold decorations at x2
                d.cs = sl;
            } else {
                d.n[sl as usize] += 1;
                d.other_total += sl;
            }
        }
        if self.forbid {
            for t in 1..NT {
                if self.neg[t] && d.totals[t] <= d.neg_thr && d.nw < 32 {
                    d.watch[d.nw] = t as u16;
                    d.nw += 1;
                }
            }
        }
        self.solve(&mut d, 0);
        self.deco_nodes += d.nodes;
        self.nodes += d.nodes;
        if !d.found {
            return false;
        }
        for p in &d.best_stack[..d.best_n] {
            let j = &self.jewels[p.jewel as usize];
            let mult = if p.torso { m } else if p.charm { 2 } else { 1 };
            d.totals[j.pos.tree as usize] += j.pos.pt as i16 * mult;
            if j.neg.tree != 0 {
                d.totals[j.neg.tree as usize] += j.neg.pt as i16 * mult;
            }
        }
        let bonus = (TREE_TORSO_UP as usize + 1..NT)
            .filter(|&t| self.t2i[t] == 255 && d.totals[t] != 0 && d.totals[t] + if secret { 2 } else { 0 } >= 10)
            .count() as u8;
        let mut groups = [0u16; PARTS];
        for l in 0..PARTS {
            groups[self.level_part[l]] = self.chosen[l];
        }
        let side = |x: VSide| x.map_or(Pt::default(), |(t, p)| Pt { tree: self.tt[t], pt: p as i8 });
        let res = SetResult {
            groups,
            charm: self.chosen[CHARM_LEVEL],
            jewels: d.best_stack,
            nj: d.best_n as u8,
            free: total - d.best,
            bonus,
            def: s.def,
            craft: s.craft,
            rare: s.rare,
            need: Charm { skills: [side(a), side(b)], slots: sl },
            chance,
            variants: Vec::new(),
        };
        self.offer(res);
        true
    }
}

// ---- Rarity priority in stages (owned charms) ----
// A single pass with rarity priority had no free-slot bound for branches below the worst result's rarity: with a minimum
// defense up to 300x more nodes than free-slot priority for the same sets (2026-11-22). The callers (plugin job.rs, the
// query example) run it as stages instead: stage N is a free-slot search where unowned armor above rarity N is excluded,
// and only its sets of rarity exactly N are kept (lower ones came from earlier stages). Stages stop once max results are
// kept. Within one rarity, free-slot priority ranks like rarity priority (free slots, parts to craft, defense).

/// Highest armor rarity (the last stage)
pub fn stage_max_rarity(armor: &[Armor]) -> u8 {
    armor.iter().map(|a| a.rare).max().unwrap_or(0)
}

/// Armor ids a stage leaves out: unowned armor above the rarity cap (`owned` sorted)
pub fn stage_excluded(armor: &[Armor], owned: &[u16], cap: u8) -> Vec<u16> {
    armor.iter().filter(|a| a.rare > cap && owned.binary_search(&a.id).is_err()).map(|a| a.id).collect()
}

/// Rarity of a set for rarity priority: per part the lowest-rarity member of its group (owned = 0), the highest of those
pub fn set_rarity(s: &Searcher<'_>, armor: &[Armor], owned: &[u16], r: &SetResult) -> u8 {
    (0..PARTS)
        .map(|p| {
            s.armor_members(p, r.groups[p])
                .iter()
                .map(|&i| {
                    let a = &armor[i as usize];
                    if owned.binary_search(&a.id).is_ok() { 0 } else { a.rare }
                })
                .min()
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}
