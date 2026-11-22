//! Search engine verification.
//!
//! 1. Compare with exhaustive search on small random data (`matches_brute_force`, at scale `stress_brute_force`).
//!    The exhaustive search evaluates every armor/charm combination and every decoration placement with the rule engine.
//! 2. On the real game data, compare the results of the settings used by the preview/plugin (dominance, relaxed, grouping, non-dominated sets)
//!    with the exact settings (dominance and relaxed off), and verify every result with the rule engine (`stress_real_data`).
//!
//! Large runs (slow):
//!   STRESS_N=20000 cargo test --release --test brute -- --ignored --nocapture
//!   REAL_N=2000 cargo test --release --test brute stress_real_data -- --ignored --nocapture
use skillsim_engine::rules::{evaluate, Build};
use skillsim_engine::search::{Query, Searcher, SetResult, Status, Target};
use skillsim_engine::types::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        // Mix so it never becomes 0
        Rng(seed.wrapping_mul(0x9e3779b97f4a7c15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[derive(Clone)]
struct Case {
    armor: Vec<Armor>,
    jewels: Vec<Jewel>,
    charms: Vec<Charm>,
    targets: Vec<Target>,
    weapon_slots: u8,
    /// Armor treated as being in the equipment box
    owned: Vec<u16>,
    /// Trees with negative skills (empty means all trees)
    neg_trees: Vec<u16>,
}

/// Size and character of the random data
#[derive(Clone, Copy)]
struct Params {
    /// Armor count per part
    k: usize,
    /// Armor skill tree range 2..=trees_hi
    trees_hi: i64,
    /// Max number of targets
    max_targets: i64,
    /// Data with many slots (like real high/G rank)
    rich: bool,
}

const SMALL: Params = Params { k: 3, trees_hi: 8, max_targets: 3, rich: false };

fn slots(r: &mut Rng, rich: bool) -> u8 {
    match (r.below(20), rich) {
        (0..=10, false) | (0..=5, true) => 0,
        (11..=16, false) | (6..=11, true) => 1,
        (17..=18, false) | (12..=16, true) => 2,
        _ => 3,
    }
}

fn make_case(r: &mut Rng, p: Params) -> Case {
    let mut armor = Vec::new();
    let mut id = 1;
    for part in 0..PARTS as u8 {
        for n in 0..p.k {
            let mut skills = [Pt::default(); 4];
            let mut sl = slots(r, p.rich);
            if part as usize != TORSO && n == 0 && r.below(3) == 0 {
                skills[0] = Pt { tree: TREE_TORSO_UP, pt: 1 };
                sl = 0;
            } else {
                for s in skills.iter_mut().take(r.range(1, 3) as usize) {
                    *s = Pt { tree: r.range(2, p.trees_hi) as u16, pt: r.range(-4, 7) as i8 };
                }
                if r.below(3) == 0 {
                    skills[3] = Pt {
                        tree: if r.below(2) == 0 { TREE_SECRET } else { TREE_TALISMAN },
                        pt: r.range(1, 5) as i8,
                    };
                }
            }
            // Keep the same tree from appearing twice on one armor.
            for i in 0..4 {
                for j in 0..i {
                    if skills[i].tree == skills[j].tree {
                        skills[i] = Pt::default();
                    }
                }
            }
            let res = [0; RES].map(|_: i8| r.range(-3, 4) as i8);
            let kind = if r.below(5) == 0 { r.range(1, 2) as u8 } else { 0 };
            armor.push(Armor { id, part, kind, sex: 0, rare: 1, slots: sl, flags: 0, def: r.range(10, 99) as u16, skills, res });
            id += 1;
        }
    }
    let mut jewels = Vec::new();
    for tree in 2..=(p.trees_hi - 1).min(9) as u16 {
        for (size, pt) in [(1u8, 1i8), (2, 3), (3, 5)] {
            if r.below(3) != 0 {
                let mut neg = Pt { tree: r.range(2, p.trees_hi) as u16, pt: -r.range(1, 2) as i8 };
                if neg.tree == tree || r.below(4) == 0 {
                    neg = Pt::default();
                }
                jewels.push(Jewel { id: 100 + jewels.len() as u16, size, rank: 1, pos: Pt { tree, pt }, neg });
            }
        }
    }
    let mut charms = Vec::new();
    for _ in 0..r.below(4) {
        let a = Pt { tree: r.range(2, p.trees_hi) as u16, pt: r.range(1, 6) as i8 };
        let mut b = Pt { tree: r.range(2, p.trees_hi) as u16, pt: r.range(-4, 6) as i8 };
        if b.tree == a.tree || b.pt == 0 {
            b = Pt::default();
        }
        charms.push(Charm { skills: [a, b], slots: slots(r, p.rich) });
    }
    let mut targets: Vec<Target> = Vec::new();
    for _ in 0..r.range(1, p.max_targets) {
        let tree = r.range(2, 5 + (p.max_targets - 3).max(0)) as u16;
        if targets.iter().all(|t| t.tree != tree) {
            targets.push(Target { tree, need: r.range(5, 13 + 2 * (p.rich as i64)) as i16 });
        }
    }
    let owned = armor.iter().filter(|a| (a.def + a.id) % 3 == 0).map(|a| a.id).collect();
    for a in armor.iter_mut() {
        a.rare = (a.def % 4 + 1) as u8;
    }
    // Half have "every tree has negative skills", half only some trees
    let neg_trees = if r.below(2) == 0 {
        Vec::new()
    } else {
        let mut v: Vec<u16> = (2..=p.trees_hi as u16).filter(|_| r.below(2) == 0).collect();
        if v.is_empty() {
            v.push(2);
        }
        v
    };
    Case { armor, jewels, charms, targets, weapon_slots: r.below(4) as u8, owned, neg_trees }
}

/// Put decorations into the real containers (an implementation independent of the engine's count formula).
fn pack(items: &[usize], jewels: &[Jewel], caps: &mut [u8], place: &mut Vec<usize>) -> bool {
    let Some((&first, rest)) = items.split_first() else { return true };
    let size = jewels[first].size;
    for c in 0..caps.len() {
        if caps[c] >= size {
            caps[c] -= size;
            place.push(c);
            if pack(rest, jewels, caps, place) {
                return true;
            }
            place.pop();
            caps[c] += size;
        }
    }
    false
}

/// Every decoration multiset with total size at most cap.
fn multisets(jewels: &[Jewel], cap: u8, from: usize, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    out.push(cur.clone());
    for j in from..jewels.len() {
        if jewels[j].size <= cap {
            cur.push(j);
            multisets(jewels, cap - jewels[j].size, j, cur, out);
            cur.pop();
        }
    }
}

type Found = BTreeMap<([u16; PARTS], usize), u8>;

fn valid(case: &Case, forbid: bool, b: &Build) -> bool {
    let Some(e) = evaluate(b) else { return false };
    case.targets.iter().all(|t| e.totals[t.tree as usize] >= t.need) && !(forbid && e.has_negative_skill_in(&case.neg_trees))
}

/// Armor usable with weapon kind 1 (blademaster)
fn usable(a: &Armor) -> bool {
    a.kind == 0 || a.kind == 1
}

/// Point totals of decorations per tree, only for the trees valid() looks at (test jewels use trees below 16 and never
/// the special trees). valid() reads a tree's total only for targets and, with forbid, the negative-skill trees, and
/// each tree's skill depends on that tree's total alone, so decoration lists with equal totals here judge the same.
type Pts = [i16; 16];
fn jewel_pts(items: &[usize], jewels: &[Jewel], relevant: &[bool; 16]) -> Pts {
    let mut v = [0i16; 16];
    for &j in items {
        for s in [jewels[j].pos, jewels[j].neg] {
            if s.tree != 0 && relevant[s.tree as usize] {
                v[s.tree as usize] += s.pt as i16;
            }
        }
    }
    v
}

/// Decoration lists for a group of containers whose slots all count the same (torso x copies, charm x2 under talisman
/// boost, the others x1): one list per container, one entry per distinct relevant total, the smallest total size kept.
/// Built container by container, so every entry really fits (no packing needed). The final judge is still evaluate()
/// on a real placement. Sorted by size, smallest first.
fn group_sets(jewels: &[Jewel], caps: &[u8], relevant: &[bool; 16]) -> Vec<(Vec<Vec<usize>>, u8)> {
    let mut cur: BTreeMap<Pts, (Vec<Vec<usize>>, u8)> = BTreeMap::new();
    cur.insert([0; 16], (Vec::new(), 0));
    for &cap in caps {
        // Decorations of one container (cap at most 3: few), one per distinct total
        let mut all = Vec::new();
        multisets(jewels, cap, 0, &mut Vec::new(), &mut all);
        let mut one: BTreeMap<Pts, (Vec<usize>, u8)> = BTreeMap::new();
        for ms in all {
            let used: u8 = ms.iter().map(|&j| jewels[j].size).sum();
            let key = jewel_pts(&ms, jewels, relevant);
            if one.get(&key).is_none_or(|b| used < b.1) {
                one.insert(key, (ms, used));
            }
        }
        let mut next: BTreeMap<Pts, (Vec<Vec<usize>>, u8)> = BTreeMap::new();
        for (pv, (lists, used)) in &cur {
            for (qv, (ms, u)) in &one {
                let key: Pts = core::array::from_fn(|t| pv[t] + qv[t]);
                let size = used + u;
                if next.get(&key).is_none_or(|b| size < b.1) {
                    let mut l = lists.clone();
                    l.push(ms.clone());
                    next.insert(key, (l, size));
                }
            }
        }
        cur = next;
    }
    let mut out: Vec<(Vec<Vec<usize>>, u8)> = cur.into_values().collect();
    out.sort_by_key(|x| x.1);
    out
}

fn brute(case: &Case, forbid: bool) -> Found {
    // Forbid only adds a condition: a combination with no solution without it has none with it, and the smallest size
    // with forbid is at least the smallest without. So the forbid run only visits the combinations found without it.
    // (Not cached across calls: callers change a case in place between calls.)
    let without = forbid.then(|| brute_inner(case, false, None));
    brute_inner(case, forbid, without.as_ref())
}

fn brute_inner(case: &Case, forbid: bool, without: Option<&Found>) -> Found {
    use std::rc::Rc;
    let by_part: Vec<Vec<&Armor>> =
        (0..PARTS).map(|p| case.armor.iter().filter(|a| a.part as usize == p && usable(a)).collect()).collect();
    if by_part.iter().any(|v| v.is_empty()) {
        return Found::new();
    }
    let charm_opts: Vec<Option<usize>> =
        if case.charms.is_empty() { vec![None] } else { (0..case.charms.len()).map(Some).collect() };
    // Trees valid() reads: the targets, and with forbid the negative-skill trees (empty list = every tree)
    let mut relevant = [false; 16];
    for t in &case.targets {
        relevant[t.tree as usize] = true;
    }
    if forbid {
        for t in 0..16 {
            if case.neg_trees.is_empty() || case.neg_trees.contains(&(t as u16)) {
                relevant[t] = true;
            }
        }
    }
    // Group lists depend only on the capacities: reused across armor combinations
    let mut lists: BTreeMap<Vec<u8>, Rc<Vec<(Vec<Vec<usize>>, u8)>>> = BTreeMap::new();
    let mut found = Found::new();
    let mut pick = [0usize; PARTS];
    loop {
        let armor: [&Armor; PARTS] = core::array::from_fn(|p| by_part[p][pick[p]]);
        for &co in &charm_opts {
            let charm = co.map(|i| &case.charms[i]);
            // Smallest size possible (from the run without forbid); no solution there = none here
            let lower = match without {
                Some(w) => match w.get(&(armor.map(|a| a.id), co.unwrap_or(usize::MAX))) {
                    Some(&u) => u,
                    None => continue,
                },
                None => 0,
            };
            // Torso and charm are groups of their own (torso x copies, charm x2 under talisman boost)
            let torso_cap = armor[TORSO].usable_slots();
            let charm_cap = charm.map_or(0, |x| x.slots);
            // Containers other than torso and charm: (container index, capacity)
            let others: Vec<(usize, u8)> = (0..CONTAINERS)
                .filter(|&c| c != TORSO && c != CHARM)
                .map(|c| (c, if c == WEAPON { case.weapon_slots } else { armor[c].usable_slots() }))
                .filter(|x| x.1 > 0)
                .collect();
            let mut get = |caps: Vec<u8>| lists.entry(caps.clone()).or_insert_with(|| Rc::new(group_sets(&case.jewels, &caps, &relevant))).clone();
            let torso_sets = get(vec![torso_cap]);
            let charm_sets = get(vec![charm_cap]);
            let other_sets = get(others.iter().map(|x| x.1).collect());
            let mut best: Option<u8> = None;
            for (os, ou) in other_sets.iter() {
                if best.is_some_and(|b| *ou >= b) {
                    break;
                }
                for (ts, tu) in torso_sets.iter() {
                    if best.is_some_and(|b| ou + tu >= b) {
                        break;
                    }
                    for (cs, cu) in charm_sets.iter() {
                        let used = ou + tu + cu;
                        if best.is_some_and(|b| used >= b) {
                            break;
                        }
                        if used < lower {
                            continue;
                        }
                        let mut b = Build {
                            armor: armor.map(Some),
                            charm,
                            weapon_slots: case.weapon_slots,
                            jewels: [[None; 3]; CONTAINERS],
                        };
                        for (k, list) in os.iter().enumerate() {
                            for (i, &j) in list.iter().enumerate() {
                                b.jewels[others[k].0][i] = Some(&case.jewels[j]);
                            }
                        }
                        for (i, &j) in ts[0].iter().enumerate() {
                            b.jewels[TORSO][i] = Some(&case.jewels[j]);
                        }
                        for (i, &j) in cs[0].iter().enumerate() {
                            b.jewels[CHARM][i] = Some(&case.jewels[j]);
                        }
                        if valid(case, forbid, &b) {
                            best = Some(used);
                            break;
                        }
                    }
                }
            }
            if let Some(used) = best {
                found.insert((armor.map(|a| a.id), co.unwrap_or(usize::MAX)), used);
            }
        }
        let mut p = 0;
        loop {
            pick[p] += 1;
            if pick[p] < by_part[p].len() {
                break;
            }
            pick[p] = 0;
            p += 1;
            if p == PARTS {
                return found;
            }
        }
    }
}

fn armor_by_id<'c>(case: &'c Case, id: u16) -> &'c Armor {
    case.armor.iter().find(|a| a.id == id).unwrap()
}

fn total_slots(case: &Case, armor: &[&Armor; PARTS], charm: Option<&Charm>) -> u8 {
    armor.iter().map(|a| a.usable_slots()).sum::<u8>() + case.weapon_slots + charm.map_or(0, |c| c.slots)
}

/// Build one engine result down to a real decoration placement and verify it with the rule engine. Returns the reason on failure.
fn verify(case: &Case, forbid: bool, s: &Searcher, r: &SetResult, armor: &[&Armor; PARTS], charm: Option<&Charm>) -> Result<(), String> {
    let mut b = Build { armor: armor.map(Some), charm, weapon_slots: case.weapon_slots, jewels: [[None; 3]; CONTAINERS] };
    let placed = &r.jewels[..r.nj as usize];
    let jl: Vec<Jewel> = placed.iter().map(|p| *s.jewel(p.jewel)).collect();
    let mut other_idx: Vec<usize> = (0..jl.len()).filter(|&i| !placed[i].torso && !placed[i].charm).collect();
    other_idx.sort_by_key(|&i| std::cmp::Reverse(jl[i].size));
    // Under talisman boost the charm's slots double their decorations: only decorations the engine put there may use them
    let double = evaluate(&b).is_some_and(|e| e.talisman);
    let others: Vec<usize> = (0..CONTAINERS).filter(|&c| c != TORSO && !(double && c == CHARM)).collect();
    let mut caps: Vec<u8> = others
        .iter()
        .map(|&c| match c {
            WEAPON => case.weapon_slots,
            CHARM => charm.map_or(0, |x| x.slots),
            _ => armor[c].usable_slots(),
        })
        .collect();
    let mut place = Vec::new();
    if !pack(&other_idx, &jl, &mut caps, &mut place) {
        return Err("들어가지 않는 장식품 배치".into());
    }
    let mut fill = [0usize; CONTAINERS];
    for (i, &ji) in other_idx.iter().enumerate() {
        let c = others[place[i]];
        b.jewels[c][fill[c]] = Some(&jl[ji]);
        fill[c] += 1;
    }
    for (i, p) in placed.iter().enumerate() {
        if p.charm {
            if !double {
                return Err("호석 계통 배가 없이 호석 배가 칸에 장식품".into());
            }
            if fill[CHARM] >= 3 || jl[i].size + b.jewels[CHARM].iter().flatten().map(|j| j.size).sum::<u8>() > charm.map_or(0, |c| c.slots) {
                return Err("호석 칸에 들어가지 않는 장식품".into());
            }
            b.jewels[CHARM][fill[CHARM]] = Some(&jl[i]);
            fill[CHARM] += 1;
        }
        if p.torso {
            if fill[TORSO] >= 3 {
                return Err("몸통 장식품이 3개를 넘는다".into());
            }
            b.jewels[TORSO][fill[TORSO]] = Some(&jl[i]);
            fill[TORSO] += 1;
        }
    }
    if !valid(case, forbid, &b) {
        let e = evaluate(&b);
        return Err(format!(
            "규칙 엔진 검증 실패 (목표 합계 {:?})",
            case.targets.iter().map(|t| (t.tree, e.as_ref().map(|e| e.totals[t.tree as usize]))).collect::<Vec<_>>()
        ));
    }
    let used: u8 = jl.iter().map(|j| j.size).sum();
    if r.free != total_slots(case, armor, charm) - used {
        return Err(format!("남는 슬롯 {} != {}", r.free, total_slots(case, armor, charm) - used));
    }
    let def: u16 = armor.iter().map(|a| a.def).sum();
    if r.def != def {
        return Err(format!("방어력 {} != {}", r.def, def));
    }
    Ok(())
}

fn check_result(case: &Case, forbid: bool, s: &Searcher, r: &SetResult, armor: &[&Armor; PARTS], charm: Option<&Charm>) {
    if let Err(e) = verify(case, forbid, s, r, armor, charm) {
        panic!("엔진 결과가 틀렸다: {e}");
    }
}

#[derive(Clone, Copy)]
struct Mode {
    forbid: bool,
    dominance: bool,
    relaxed: bool,
    max_results: usize,
    slice: u64,
    min_res: [Option<i8>; RES],
    group: bool,
    pareto: bool,
    priority: u8,
    fixed: [u16; PARTS],
    excluded: [u16; 2],
    min_def: u16,
    keep_defense: bool,
    /// Split work (parallel search emulation): two searchers share the ticket counter, run in turns, and are merged at the end
    split: bool,
    /// Start from the earlier sets stored in SEEDS (Searcher::seed)
    seed: bool,
}

thread_local! {
    /// (original armor index per part, original charm index) to add as seeds
    static SEEDS: core::cell::RefCell<Vec<([u16; PARTS], Option<u16>)>> = const { core::cell::RefCell::new(Vec::new()) };
}

const EXACT: Mode = Mode {
    forbid: false,
    dominance: false,
    relaxed: false,
    max_results: 1 << 20,
    slice: 37,
    min_res: [None; RES],
    group: false,
    pareto: false,
    priority: 0,
    fixed: [0; PARTS],
    excluded: [0; 2],
    min_def: 0,
    keep_defense: false,
    split: false,
    seed: false,
};

/// Record of the last engine() call
#[derive(Default, Clone)]
struct Last {
    /// (free slots, parts to craft, defense) of the non-dominated sets
    front: BTreeSet<(u8, u8, u16)>,
    /// Top set: (highest rarity among parts to craft, free slots, parts to craft)
    top: Option<(u8, u8, u8)>,
    /// Defense of the top set
    top_def: Option<u16>,
    /// Worst free slots among the results
    worst_free: Option<u8>,
}

/// Run the engine and return (results expanded to members, result count, record).
fn engine(case: &Case, m: &Mode) -> (Found, usize, Last) {
    let q = Query {
        targets: &case.targets,
        kind: 1,
        sex: 1,
        rare_mask: 0xffff,
        weapon_slots: case.weapon_slots,
        exclude_flags: 0,
        jewel_rank: 3,
        jewel_ids: None,
        priority: m.priority,
        forbid_negative: m.forbid,
        negative_trees: &case.neg_trees,
        dominance: m.dominance,
        keep_defense: m.keep_defense,
        relaxed: m.relaxed,
        fixed: m.fixed,
        excluded: &m.excluded,
        charms: &case.charms,
        max_results: m.max_results,
        min_def: m.min_def,
        min_res: m.min_res,
        group_similar: m.group,
        charm_kinds: None,
        owned: &case.owned,
        pareto: m.pareto,
    };
    let mut last = Last::default();
    let mut s = Searcher::new(&case.armor, &case.jewels, &q).unwrap();
    if m.seed {
        SEEDS.with(|v| {
            for &(a, c) in v.borrow().iter() {
                s.seed(a, c);
            }
        });
    }
    let counter = core::sync::atomic::AtomicU32::new(0);
    if m.split {
        let mut s2 = Searcher::new(&case.armor, &case.jewels, &q).unwrap();
        s.share_root(&counter);
        s2.share_root(&counter);
        // Alternate in small steps (really two threads run at the same time)
        loop {
            let a = s.run(m.slice);
            let b = s2.run(m.slice + 7);
            if a == Status::Done && b == Status::Done {
                break;
            }
        }
        s.merge(s2);
    } else {
        while s.run(m.slice) == Status::Paused {}
    }
    let mut found = Found::new();
    // Sets in a group must differ from the representative in exactly one part (also after the representative changes).
    for r in s.results() {
        for v in &r.variants {
            assert_eq!((0..PARTS).filter(|&p| r.groups[p] != v.groups[p]).count(), 1, "대표와 두 부위 이상 다른 세트가 묶였다");
        }
    }
    let all: Vec<&SetResult> = s.results().iter().flat_map(|r| core::iter::once(r).chain(r.variants.iter())).collect();
    for r in all {
        let reps: [&Armor; PARTS] = core::array::from_fn(|p| &case.armor[s.armor_members(p, r.groups[p])[0] as usize]);
        let cm = s.charm_members(r.charm);
        check_result(case, m.forbid, &s, r, &reps, cm.first().map(|&i| &case.charms[i as usize]));
        if m.relaxed {
            // In relaxed mode group members may differ in non-target positive points, so only the representative is guaranteed.
            let charm = cm.first().map(|&i| &case.charms[i as usize]);
            let key = (reps.map(|a| a.id), cm.first().map_or(usize::MAX, |&i| i as usize));
            found.insert(key, total_slots(case, &reps, charm) - r.free);
            continue;
        }
        let mut idx = [0usize; PARTS];
        loop {
            let armor: [&Armor; PARTS] =
                core::array::from_fn(|p| &case.armor[s.armor_members(p, r.groups[p])[idx[p]] as usize]);
            let ids = armor.map(|a| a.id);
            // Only the group representative (highest defense) is guaranteed to meet the min defense. The preview filters short members when showing them.
            let below = armor.iter().map(|a| a.def).sum::<u16>() < m.min_def;
            if below {
            } else if cm.is_empty() {
                found.insert((ids, usize::MAX), total_slots(case, &armor, None) - r.free);
            }
            for &c in cm.iter().filter(|_| !below) {
                let charm = &case.charms[c as usize];
                found.insert((ids, c as usize), total_slots(case, &armor, Some(charm)) - r.free);
            }
            let mut p = 0;
            loop {
                idx[p] += 1;
                if idx[p] < s.armor_members(p, r.groups[p]).len() {
                    break;
                }
                idx[p] = 0;
                p += 1;
                if p == PARTS {
                    break;
                }
            }
            if p == PARTS {
                break;
            }
        }
    }
    let sorted = s.sorted_results();
    if let Some(top) = sorted.first() {
        let reps: [&Armor; PARTS] = core::array::from_fn(|p| &case.armor[s.armor_members(p, top.groups[p])[0] as usize]);
        last.top = Some(rank_of(case, &reps, top.free));
        last.top_def = Some(top.def);
    }
    last.worst_free = s.results().iter().map(|r| r.free).min();
    if m.pareto {
        for f in s.front() {
            let reps: [&Armor; PARTS] = core::array::from_fn(|p| &case.armor[s.armor_members(p, f.groups[p])[0] as usize]);
            let cm = s.charm_members(f.charm);
            check_result(case, m.forbid, &s, &f, &reps, cm.first().map(|&i| &case.charms[i as usize]));
            let craft = reps.iter().filter(|a| !case.owned.contains(&a.id)).count() as u8;
            let def: u16 = reps.iter().map(|a| a.def).sum();
            assert_eq!((f.craft, f.def), (craft, def), "밀리지 않는 세트의 만들 부위/방어력이 틀렸다");
            last.front.insert((f.free, f.craft, f.def));
        }
    }
    (found, s.results().len(), last)
}

fn rank_of(case: &Case, armor: &[&Armor; PARTS], free: u8) -> (u8, u8, u8) {
    let new: Vec<&&Armor> = armor.iter().filter(|a| !case.owned.contains(&a.id)).collect();
    (new.iter().map(|a| a.rare).max().unwrap_or(0), free, new.len() as u8)
}

fn armor_of(case: &Case, ids: &[u16; PARTS]) -> [Armor; PARTS] {
    core::array::from_fn(|p| *armor_by_id(case, ids[p]))
}

fn free_of(case: &Case, key: &([u16; PARTS], usize), used: u8) -> u8 {
    let a = armor_of(case, &key.0);
    let refs: [&Armor; PARTS] = core::array::from_fn(|p| &a[p]);
    total_slots(case, &refs, case.charms.get(key.1)) - used
}

fn def_of(case: &Case, ids: &[u16; PARTS]) -> u16 {
    ids.iter().map(|&i| armor_by_id(case, i).def).sum()
}

/// Over all correct answers, the values not beaten in both (free slots, parts to craft) (on a tie, the highest defense).
fn true_front(case: &Case, found: &Found) -> BTreeSet<(u8, u8, u16)> {
    let all: BTreeSet<(u8, u8, u16)> = found
        .iter()
        .map(|(k, used)| {
            let craft = k.0.iter().filter(|id| !case.owned.contains(id)).count() as u8;
            (free_of(case, k, *used), craft, def_of(case, &k.0))
        })
        .collect();
    all.iter()
        .filter(|&&(f, c, d)| {
            !all.iter().any(|&(f2, c2, d2)| (f2, c2, d2) != (f, c, d) && f2 >= f && c2 <= c && (f2 > f || c2 < c || d2 >= d))
        })
        .copied()
        .collect()
}

fn max_free(case: &Case, found: &Found) -> Option<u8> {
    found.iter().map(|(k, used)| free_of(case, k, *used)).max()
}

#[derive(Default)]
struct Stats {
    cases: u32,
    feasible: u32,
    relaxed_runs: u32,
    relaxed_miss: u32,
    res_feasible: u32,
    failed: Vec<u64>,
}

/// Every comparison for one data set. Panics on a mismatch.
fn check_case(case: &Case, n: u64, st: &mut Stats) {
    st.cases += 1;
    let mut rng = Rng::new(n ^ 0x5151);
    for forbid in [false, true] {
        let truth = brute(case, forbid);
        st.feasible += !truth.is_empty() as u32;
        let exact = Mode { forbid, ..EXACT };

        // 1. Without dominance: the set of valid combinations and the min slot usage must match exactly.
        assert_eq!(engine(case, &exact).0, truth, "case {n} forbid {forbid}: 전체 결과 불일치");

        // 2. Dominance: whether a solution exists and the max free slots must match.
        let (got, _, _) = engine(case, &Mode { dominance: true, ..exact });
        assert!(got.iter().all(|(k, v)| truth.get(k) == Some(v)), "case {n}: 열등 제거 결과가 정답에 없다");
        assert_eq!(max_free(case, &got), max_free(case, &truth), "case {n} forbid {forbid}: 최선 불일치");

        // 3. Even with a small result limit the best must remain.
        let (got, count, _) = engine(case, &Mode { max_results: 2, slice: u64::MAX, ..exact });
        assert!(count <= 2);
        assert_eq!(max_free(case, &got), max_free(case, &truth), "case {n} forbid {forbid}: 상위 결과 불일치");

        // 3-0. Top K: a combination pushed out of the results cannot be better than the worst free slots in the results.
        let (got, count, last) = engine(case, &Mode { max_results: 4, ..exact });
        if count < 4 {
            assert_eq!(got, truth, "case {n} forbid {forbid}: 상한보다 적게 냈는데 빠진 조합이 있다");
        } else {
            let worst = last.worst_free.unwrap();
            for (k, used) in &truth {
                let f = free_of(case, k, *used);
                assert!(f <= worst || got.get(k) == Some(used), "case {n} forbid {forbid}: 남는 슬롯 {f} 조합이 상위 결과(최저 {worst})에서 빠졌다");
            }
        }

        // 3-1. Grouping similar sets: everything including grouped sets must be correct and the best must remain.
        let (got, count, _) = engine(case, &Mode { dominance: true, group: true, max_results: 3, slice: 53, ..exact });
        assert!(count <= 3);
        assert!(got.iter().all(|(k, v)| truth.get(k) == Some(v)), "case {n}: 묶은 결과가 정답에 없다");
        assert_eq!(max_free(case, &got), max_free(case, &truth), "case {n} forbid {forbid}: 묶은 결과 최선 불일치");

        // 3-2. Non-dominated sets ('Thorough' = keep defense): even with result limit 1, every non-dominated value of the answer must appear.
        let (got, _, last) = engine(case, &Mode { dominance: true, pareto: true, keep_defense: true, max_results: 1, slice: 41, ..exact });
        assert!(got.iter().all(|(k, v)| truth.get(k) == Some(v)), "case {n}: 파레토 모드 결과가 정답에 없다");
        assert_eq!(last.front, true_front(case, &truth), "case {n} forbid {forbid}: 밀리지 않는 세트 불일치");
        // 3-2s. Split work (parallel search): identical to the answer with exact settings; with the preview settings ('Thorough')
        // the results must be correct and the best and non-dominated sets must match.
        let (got, _, _) = engine(case, &Mode { split: true, ..exact });
        assert_eq!(got, truth, "case {n} forbid {forbid}: 나눠 맡은 정확한 검색이 정답과 다르다");
        let (got, _, last) = engine(case, &Mode { dominance: true, group: true, pareto: true, keep_defense: true, max_results: 3, slice: 47, split: true, ..exact });
        assert!(got.iter().all(|(k, v)| truth.get(k) == Some(v)), "case {n}: 나눠 맡은 결과가 정답에 없다");
        assert_eq!(max_free(case, &got), max_free(case, &truth), "case {n} forbid {forbid}: 나눠 맡은 검색 최선 불일치");
        assert_eq!(last.front, true_front(case, &truth), "case {n} forbid {forbid}: 나눠 맡은 검색 밀리지 않는 세트 불일치");
        // 3-2e. Starting from earlier results (seed): even when seeded with part of the answer plus random combinations (mostly invalid),
        // exact settings must equal the answer, and the preview settings must give correct results with the same best and non-dominated sets.
        {
            let idx = |id: u16| case.armor.iter().position(|a| a.id == id).unwrap() as u16;
            let mut seeds: Vec<([u16; PARTS], Option<u16>)> = truth
                .keys()
                .take(25)
                .map(|(ids, c)| (core::array::from_fn(|p| idx(ids[p])), (*c != usize::MAX).then_some(*c as u16)))
                .collect();
            let mut rng = Rng::new(n ^ 0x5eed);
            for _ in 0..25 {
                let pick: [u16; PARTS] = core::array::from_fn(|p| {
                    let ids: Vec<u16> = case.armor.iter().enumerate().filter(|(_, a)| a.part as usize == p).map(|(i, _)| i as u16).collect();
                    ids[rng.below(ids.len() as u64) as usize]
                });
                let c = (!case.charms.is_empty()).then(|| rng.below(case.charms.len() as u64) as u16);
                seeds.push((pick, c));
            }
            SEEDS.with(|v| *v.borrow_mut() = seeds);
            let (got, _, _) = engine(case, &Mode { seed: true, ..exact });
            assert_eq!(got, truth, "case {n} forbid {forbid}: seed 로 시작한 정확한 검색이 정답과 다르다");
            let (got, _, last) = engine(case, &Mode { dominance: true, group: true, pareto: true, keep_defense: true, max_results: 3, slice: 51, seed: true, ..exact });
            assert!(got.iter().all(|(k, v)| truth.get(k) == Some(v)), "case {n}: seed 로 시작한 결과가 정답에 없다");
            assert_eq!(max_free(case, &got), max_free(case, &truth), "case {n} forbid {forbid}: seed 로 시작한 검색 최선 불일치");
            assert_eq!(last.front, true_front(case, &truth), "case {n} forbid {forbid}: seed 로 시작한 검색 밀리지 않는 세트 불일치");
            SEEDS.with(|v| v.borrow_mut().clear());
        }
        // 3-2b. 'Fast' (keep defense off): results must be correct and the non-dominated values by free slots and parts to craft must match.
        let (got, _, last) = engine(case, &Mode { dominance: true, pareto: true, max_results: 1, slice: 43, ..exact });
        assert!(got.iter().all(|(k, v)| truth.get(k) == Some(v)), "case {n}: 빠른 파레토 결과가 정답에 없다");
        let fc = |s: &BTreeSet<(u8, u8, u16)>| -> BTreeSet<(u8, u8)> {
            let all: BTreeSet<(u8, u8)> = s.iter().map(|&(f, c, _)| (f, c)).collect();
            all.iter().filter(|&&(f, c)| !all.iter().any(|&(f2, c2)| (f2, c2) != (f, c) && f2 >= f && c2 <= c)).copied().collect()
        };
        assert_eq!(fc(&last.front), fc(&true_front(case, &truth)), "case {n} forbid {forbid}: 빠른 모드의 슬롯·만들 부위 밀리지 않는 값 불일치");

        if !truth.is_empty() {
            // 3-3. Lowest rarity first: even with result limit 2, the top must equal the answer's top.
            let (got, _, last) = engine(case, &Mode { dominance: true, priority: 2, max_results: 2, slice: 29, ..exact });
            assert!(got.iter().all(|(k, v)| truth.get(k) == Some(v)), "case {n}: 희귀도 모드 결과가 정답에 없다");
            let want = truth
                .iter()
                .map(|(k, used)| {
                    let a = armor_of(case, &k.0);
                    let refs: [&Armor; PARTS] = core::array::from_fn(|p| &a[p]);
                    rank_of(case, &refs, free_of(case, k, *used))
                })
                .min_by_key(|&(r, f, c)| (r, core::cmp::Reverse(f), c));
            assert_eq!(last.top, want, "case {n} forbid {forbid}: 희귀도 순 1위 불일치");

            // 3-4. Defense priority: the top's defense must equal the answer's highest defense.
            let (got, _, last) =
                engine(case, &Mode { dominance: true, keep_defense: true, priority: 1, max_results: 2, slice: 31, ..exact });
            assert!(got.iter().all(|(k, v)| truth.get(k) == Some(v)), "case {n}: 방어력 모드 결과가 정답에 없다");
            let best_def = truth.keys().map(|k| def_of(case, &k.0)).max();
            assert_eq!(last.top_def, best_def, "case {n} forbid {forbid}: 방어력 순 1위 불일치");

            // 3-5. Min defense: only at or above the median defense of the answer.
            let mut defs: Vec<u16> = truth.keys().map(|k| def_of(case, &k.0)).collect();
            defs.sort();
            let min_def = defs[defs.len() / 2];
            let want: Found = truth.iter().filter(|(k, _)| def_of(case, &k.0) >= min_def).map(|(k, v)| (*k, *v)).collect();
            assert_eq!(engine(case, &Mode { min_def, ..exact }).0, want, "case {n} forbid {forbid}: 최소 방어력 결과 불일치");
            let (got, _, _) = engine(case, &Mode { min_def, dominance: true, keep_defense: true, ..exact });
            assert!(got.iter().all(|(k, v)| want.get(k) == Some(v)), "case {n}: 최소 방어력 결과가 정답에 없다");
            assert_eq!(max_free(case, &got), max_free(case, &want), "case {n} forbid {forbid}: 최소 방어력 최선 불일치");
        }

        // 3-6. Fixed and excluded: must equal the answer filtered.
        let ids: Vec<&Armor> = case.armor.iter().filter(|a| usable(a)).collect();
        let fx = ids[rng.below(ids.len() as u64) as usize];
        let mut fixed = [0u16; PARTS];
        fixed[fx.part as usize] = fx.id;
        let want: Found = truth.iter().filter(|(k, _)| k.0[fx.part as usize] == fx.id).map(|(k, v)| (*k, *v)).collect();
        assert_eq!(engine(case, &Mode { fixed, ..exact }).0, want, "case {n} forbid {forbid}: 고정 결과 불일치");
        let (got, _, _) = engine(case, &Mode { fixed, dominance: true, ..exact });
        assert_eq!(max_free(case, &got), max_free(case, &want), "case {n} forbid {forbid}: 고정 최선 불일치");
        let ex = [ids[rng.below(ids.len() as u64) as usize].id, ids[rng.below(ids.len() as u64) as usize].id];
        let want: Found = truth.iter().filter(|(k, _)| !k.0.iter().any(|i| ex.contains(i))).map(|(k, v)| (*k, *v)).collect();
        assert_eq!(engine(case, &Mode { excluded: ex, ..exact }).0, want, "case {n} forbid {forbid}: 제외 결과 불일치");
        let (got, _, _) = engine(case, &Mode { excluded: ex, dominance: true, group: true, max_results: 3, ..exact });
        assert_eq!(max_free(case, &got), max_free(case, &want), "case {n} forbid {forbid}: 제외 최선 불일치");

        // 4. Relaxed mode (preview/plugin default): no wrong results; misses are only counted.
        if forbid {
            let (got, _, _) = engine(case, &Mode { dominance: true, relaxed: true, ..exact });
            assert!(got.iter().all(|(k, v)| truth.get(k).is_some_and(|t| t <= v)), "case {n}: 완화 모드가 틀린 결과를 냈다");
            st.relaxed_runs += 1;
            let miss = max_free(case, &got) != max_free(case, &truth);
            if miss {
                eprintln!("  완화 모드 놓침: case {n} (정답 최선 {:?}, 완화 {:?})", max_free(case, &truth), max_free(case, &got));
            }
            st.relaxed_miss += miss as u32;
        }

        // 5. Resistance condition: must equal the answer minus combinations with too little resistance.
        let mut min_res = [None; RES];
        min_res[(n as usize) % RES] = Some((n % 7) as i8 - 2);
        if n % 3 == 0 {
            min_res[(n as usize + 2) % RES] = Some(0);
        }
        // Conditions on every element (the search then walks coarse groups and expands them at the end)
        if n % 5 == 1 {
            for (e, m) in min_res.iter_mut().enumerate() {
                *m = Some(((n / 5 + e as u64) % 5) as i8 - 3);
            }
        }
        let res_ok = |ids: &[u16; PARTS]| {
            (0..RES).all(|e| {
                let sum: i32 = ids.iter().map(|id| armor_by_id(case, *id).res[e] as i32).sum();
                min_res[e].is_none_or(|m| sum >= m as i32)
            })
        };
        let want: Found = truth.iter().filter(|((ids, _), _)| res_ok(ids)).map(|(k, v)| (*k, *v)).collect();
        assert_eq!(engine(case, &Mode { min_res, ..exact }).0, want, "case {n} forbid {forbid}: 내성 조건 결과 불일치");
        let (got, _, _) = engine(case, &Mode { dominance: true, min_res, ..exact });
        assert!(got.iter().all(|(k, v)| want.get(k) == Some(v)), "case {n}: 내성 조건 결과가 정답에 없다");
        assert_eq!(max_free(case, &got), max_free(case, &want), "case {n} forbid {forbid}: 내성 조건 최선 불일치");
        st.res_feasible += !want.is_empty() as u32;
        // Resistance and minimum defense together (both in the joint value bound): median defense of the answer
        if !want.is_empty() {
            let mut defs: Vec<u16> = want.keys().map(|k| def_of(case, &k.0)).collect();
            defs.sort();
            let min_def = defs[defs.len() / 2];
            let both: Found = want.iter().filter(|(k, _)| def_of(case, &k.0) >= min_def).map(|(k, v)| (*k, *v)).collect();
            assert_eq!(engine(case, &Mode { min_res, min_def, ..exact }).0, both, "case {n} forbid {forbid}: 내성+방어력 결과 불일치");
            let (got, _, _) = engine(case, &Mode { min_res, min_def, dominance: true, keep_defense: true, ..exact });
            assert!(got.iter().all(|(k, v)| both.get(k) == Some(v)), "case {n}: 내성+방어력 결과가 정답에 없다");
            assert_eq!(max_free(case, &got), max_free(case, &both), "case {n} forbid {forbid}: 내성+방어력 최선 불일치");
        }
    }
}

#[test]
fn matches_brute_force() {
    let mut st = Stats::default();
    for n in 0..150u64 {
        let case = make_case(&mut Rng::new(n), SMALL);
        check_case(&case, n, &mut st);
    }
    println!(
        "해가 있는 경우 {}/{}, 완화 모드에서 최선을 놓친 경우 {}/{}, 내성 조건에서 해가 있는 경우 {}/{}",
        st.feasible, st.cases * 2, st.relaxed_miss, st.relaxed_runs, st.res_feasible, st.cases * 2
    );
    assert!(st.feasible > 60, "테스트 데이터가 너무 어렵다: {}", st.feasible);
}

/// Large comparison. STRESS_N (default 3000) data sets of mixed sizes, split across threads.
/// A case running longer than STRESS_SLOW seconds (default 120) is printed with its number (it can be rerun alone with
/// CHECK_N / DEBUG_N). When every thread is either done or stuck on one case longer than STRESS_GIVEUP seconds
/// (default 900), the test reports what finished and the stuck case numbers and ends (a stuck case cannot be interrupted).
#[test]
#[ignore]
fn stress_brute_force() {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    let total = env("STRESS_N", 3000);
    let seed = env("STRESS_SEED", 1_000_000);
    let slow = Duration::from_secs(env("STRESS_SLOW", 120));
    let giveup = Duration::from_secs(env("STRESS_GIVEUP", 900));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let t0 = Instant::now();
    // Per thread: the case it is checking now and since when (None = finished)
    let running: Arc<Mutex<Vec<Option<(u64, Instant)>>>> = Arc::new(Mutex::new(vec![None; threads]));
    let all = Arc::new(Mutex::new(Stats::default()));
    let done = Arc::new(Mutex::new(vec![false; threads]));
    for t in 0..threads {
        let (running, all, done) = (running.clone(), all.clone(), done.clone());
        std::thread::spawn(move || {
            let mut n = seed + t as u64;
            while n < seed + total {
                let mut r = Rng::new(n);
                // Mix sizes: small, 4 per part, many slots, 4 targets
                let p = match n % 4 {
                    0 => SMALL,
                    1 => Params { k: 4, ..SMALL },
                    2 => Params { rich: true, ..SMALL },
                    _ => Params { max_targets: 4, trees_hi: 10, ..SMALL },
                };
                let case = make_case(&mut r, p);
                running.lock().unwrap()[t] = Some((n, Instant::now()));
                let started = Instant::now();
                let mut st = Stats::default();
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_case(&case, n, &mut st)));
                if res.is_err() {
                    eprintln!("  실패한 case 번호 {n} (DEBUG_N={n} 로 다시 볼 수 있다)");
                    st.failed.push(n);
                }
                let took = started.elapsed();
                if took >= slow {
                    eprintln!("  느린 case {n}: {:.0}초 걸려 끝남", took.as_secs_f64());
                }
                {
                    let mut a = all.lock().unwrap();
                    a.cases += st.cases;
                    a.feasible += st.feasible;
                    a.relaxed_runs += st.relaxed_runs;
                    a.relaxed_miss += st.relaxed_miss;
                    a.res_feasible += st.res_feasible;
                    a.failed.extend(st.failed);
                }
                n += threads as u64;
            }
            running.lock().unwrap()[t] = None;
            done.lock().unwrap()[t] = true;
        });
    }
    // Watch: report slow cases once, and stop waiting when only stuck cases are left
    let mut reported: Vec<u64> = Vec::new();
    let mut stuck: Vec<(u64, f64)> = Vec::new();
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let now = running.lock().unwrap().clone();
        let finished = done.lock().unwrap().iter().filter(|&&d| d).count();
        for &(n, since) in now.iter().flatten() {
            if since.elapsed() >= slow && !reported.contains(&n) {
                reported.push(n);
                eprintln!("  느린 case {n}: {:.0}초째 계산 중 (CHECK_N={n} 로 따로 돌릴 수 있다)", since.elapsed().as_secs_f64());
            }
        }
        let busy: Vec<(u64, Instant)> = now.iter().flatten().copied().collect();
        if finished == threads {
            break;
        }
        if finished + busy.len() == threads && busy.iter().all(|(_, since)| since.elapsed() >= giveup) {
            stuck = busy.iter().map(|&(n, since)| (n, since.elapsed().as_secs_f64())).collect();
            break;
        }
    }
    let mut all = std::mem::take(&mut *all.lock().unwrap());
    all.failed.sort();
    println!(
        "대조 {}개 ({:.0}초): 해가 있는 경우 {}/{}, 완화 모드에서 최선을 놓친 경우 {}/{}, 내성 조건에서 해가 있는 경우 {}/{}",
        all.cases,
        t0.elapsed().as_secs_f64(),
        all.feasible,
        all.cases * 2,
        all.relaxed_miss,
        all.relaxed_runs,
        all.res_feasible,
        all.cases * 2
    );
    if !stuck.is_empty() {
        println!("끝나지 않은 case {}개 (확인 못 함): {:?}", stuck.len(), stuck.iter().map(|&(n, s)| format!("{n} ({s:.0}초)")).collect::<Vec<_>>());
    }
    assert!(all.failed.is_empty(), "실패한 case {}개: {:?}", all.failed.len(), all.failed);
    if !stuck.is_empty() {
        // The stuck threads cannot be stopped: end the process with the report above
        std::process::exit(2);
    }
}

/// Look at one failing random data set in detail: DEBUG_N=number
#[test]
#[ignore]
fn debug_case() {
    let n = env("DEBUG_N", 0);
    let p = match n % 4 {
        0 => SMALL,
        1 => Params { k: 4, ..SMALL },
        2 => Params { rich: true, ..SMALL },
        _ => Params { max_targets: 4, trees_hi: 10, ..SMALL },
    };
    let case = make_case(&mut Rng::new(n), p);
    println!("targets {:?} weapon {} neg_trees {:?}", case.targets, case.weapon_slots, case.neg_trees);
    for a in &case.armor {
        println!("  armor {} part {} kind {} slots {} skills {:?}", a.id, a.part, a.kind, a.slots, a.skills);
    }
    println!("  charms {:?}", case.charms);
    println!("  jewels {:?}", case.jewels.iter().map(|j| (j.size, j.pos, j.neg)).collect::<Vec<_>>());
    for forbid in [false, true] {
        let truth = brute(&case, forbid);
        let best = max_free(&case, &truth);
        println!("forbid {forbid}: 정답 최선 {best:?}");
        for (k, used) in &truth {
            if Some(free_of(&case, k, *used)) == best {
                println!("  정답 최선 조합 {:?} 호석 {}", k.0, k.1 as i64);
            }
        }
        let (got, _, _) = engine(&case, &Mode { forbid, dominance: true, ..EXACT });
        println!("  열등 제거 최선 {:?}", max_free(&case, &got));
        // What happens without the charms' negative points
        let mut c2 = case.clone();
        for c in c2.charms.iter_mut() {
            if c.skills[1].pt < 0 {
                c.skills[1] = Pt::default();
            }
        }
        let t2 = brute(&c2, forbid);
        let (g2, _, _) = engine(&c2, &Mode { forbid, dominance: true, ..EXACT });
        println!("  호석 음수 제거: 정답 {:?} 열등 제거 {:?}", max_free(&c2, &t2), max_free(&c2, &g2));
    }
}

/// Needed-charm mode: every returned result must be valid, and if some charm can work there must be a result.
#[test]
fn needed_charm_is_sound() {
    needed_charm(env("NEEDED_N", 30), env("NEEDED_SEED", 0x1234567890abcdef));
}

#[test]
#[ignore]
fn stress_needed_charm() {
    // Split across threads
    let count = env("NEEDED_N", 600);
    let seed = env("STRESS_SEED", 77);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    let per = count.div_ceil(threads);
    let hs: Vec<_> = (0..threads).map(|t| std::thread::spawn(move || needed_charm(per, seed + t * per))).collect();
    let failed = hs.into_iter().map(|h| h.join()).filter(|r| r.is_err()).count();
    assert_eq!(failed, 0, "미보유 호석 대조 실패한 스레드 {failed}개");
}

fn needed_charm(count: u64, seed: u64) {
    use skillsim_engine::charm::chance;
    let kinds = [CharmKind {
        second_threshold: 20,
        first: (2..=5).map(|tree| CharmSkill { tree, lo: 1, hi: 4 }).collect(),
        second: (2..=8).map(|tree| CharmSkill { tree, lo: 2, hi: 5 }).collect(),
        slots: [[40, 70, 90]; 20],
    }];
    // The best charms the table can produce. If these fail, no charm works.
    let mut best_charms = Vec::new();
    for a in 2..=5u16 {
        for b in 2..=8u16 {
            if a != b {
                best_charms.push(Charm { skills: [Pt { tree: a, pt: 4 }, Pt { tree: b, pt: 5 }], slots: 3 });
            }
        }
    }
    let (mut feasible, mut missed) = (0, 0);
    let mut missed_seeds = Vec::new();
    for n in 0..count {
        let mut rng = Rng::new(seed.wrapping_add(n));
        let mut case = make_case(&mut rng, SMALL);
        let forbid = n % 2 == 1;
        case.charms = best_charms.clone();
        let truth = brute(&case, forbid);
        let possible = !truth.is_empty();
        if std::env::var("NEEDED_DEBUG").is_ok() {
            println!("targets {:?} weapon {} forbid {forbid}", case.targets, case.weapon_slots);
            for a in &case.armor {
                println!("  armor {} part {} kind {} slots {} skills {:?}", a.id, a.part, a.kind, a.slots, a.skills);
            }
            println!("  jewels {:?}", case.jewels.iter().map(|j| (j.size, j.pos, j.neg)).collect::<Vec<_>>());
            for (k, used) in truth.iter().take(8) {
                println!("  정답 {:?} 호석 {:?} 사용 {used:?}", k.0, case.charms[k.1 as usize]);
            }
        }
        case.charms.clear();
        let q = Query {
            targets: &case.targets,
            kind: 1,
            sex: 1,
            rare_mask: 0xffff,
            weapon_slots: case.weapon_slots,
            exclude_flags: 0,
            jewel_rank: 3,
            jewel_ids: None,
            priority: 0,
            forbid_negative: forbid,
            negative_trees: &case.neg_trees,
            dominance: true,
            keep_defense: false,
            relaxed: false,
            fixed: [0; PARTS],
            excluded: &[],
            charms: &[],
            max_results: 50,
            min_def: 0,
            min_res: [None; RES],
            group_similar: false,
            charm_kinds: Some(&kinds),
            owned: &case.owned,
            pareto: true,
        };
        let mut s = Searcher::new(&case.armor, &case.jewels, &q).unwrap();
        while s.run(101) == Status::Paused {}
        for r in s.results() {
            let reps: [&Armor; PARTS] =
                core::array::from_fn(|p| &case.armor[s.armor_members(p, r.groups[p])[0] as usize]);
            assert!(chance(&kinds, &r.need) > 0.0, "case {n}: 표에 없는 호석을 요구했다 {:?}", r.need);
            check_result(&case, forbid, &s, r, &reps, Some(&r.need));
        }
        assert!(possible || s.results().is_empty(), "case {n}: 불가능한데 결과를 냈다");
        feasible += possible as u32;
        if possible && s.results().is_empty() {
            eprintln!("  필요한 호석 놓침: case {n}");
            missed += 1;
            missed_seeds.push(seed.wrapping_add(n));
        }
    }
    println!("필요한 호석: {count}개 중 가능한 경우 {feasible}, 그중 놓친 경우 {missed}");
    assert!(
        feasible >= count as u32 / 4 && missed == 0,
        "가능 {feasible}, 놓침 {missed} (NEEDED_SEED 로 다시 보기: {missed_seeds:?})"
    );
}

/// Real game data. Compare the preview settings (dominance, relaxed, grouping, non-dominated sets) with the exact settings.
#[test]
#[ignore]
fn stress_real_data() {
    use skillsim_engine::data;
    let blob = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../data/simdata.bin")).expect("simdata.bin 없음");
    let gd = std::sync::Arc::new(data::parse(&blob).expect("simdata.bin 형식 오류"));
    let total = env("REAL_N", 400);
    let seed = env("STRESS_SEED", 9_000_000);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()) as u64;
    let t0 = std::time::Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let gd = gd.clone();
            std::thread::spawn(move || real_worker(&gd, seed, total, t, threads))
        })
        .collect();
    let mut sum = RealStats::default();
    for h in handles {
        let s = h.join().expect("스레드 실패");
        sum.add(&s);
    }
    println!(
        "실제 데이터 {}건 ({:.0}초): 해 있음 {}, 미보유 호석으로만 해 있음 {}, 정확 기준이 오래 걸려 완화만 끈 기준으로 비교 {}, 그것도 예산 초과로 비교 못 함 {}, \
         미리보기 설정이 예산(5천만 노드) 초과 {}, 결과 검증 {}개 모두 통과",
        sum.runs,
        t0.elapsed().as_secs_f64(),
        sum.feasible,
        sum.needed_only,
        sum.ref_fallback,
        sum.ref_paused,
        sum.prod_paused,
        sum.verified
    );
    println!(
        "미리보기 설정 시간: 평균 {:.1} ms, 최대 {:.0} ms / 차이(최선 또는 존재 여부) {}건",
        sum.prod_ms / sum.runs.max(1) as f64,
        sum.prod_max_ms,
        sum.mismatch.len()
    );
    println!(
        "마이너스 계통 수정 효과(마이너스 금지 검색 중): 더 나은 세트를 찾음 {}건, 그중 전에는 '결과 없음'이던 경우 {}건",
        sum.neg_fix_gain, sum.neg_fix_found
    );
    for m in &sum.mismatch {
        println!("  차이: {m}");
    }
    assert!(sum.mismatch.is_empty(), "미리보기 설정이 정확한 설정과 다른 결과를 냈다");
}

#[derive(Default)]
struct RealStats {
    runs: u32,
    feasible: u32,
    needed_only: u32,
    ref_paused: u32,
    ref_fallback: u32,
    /// Cases where the negative-tree fix found a better set / among them, cases that had no result at all before
    neg_fix_gain: u32,
    neg_fix_found: u32,
    prod_paused: u32,
    verified: u32,
    prod_ms: f64,
    prod_max_ms: f64,
    mismatch: Vec<String>,
}

impl RealStats {
    fn add(&mut self, o: &RealStats) {
        self.runs += o.runs;
        self.feasible += o.feasible;
        self.needed_only += o.needed_only;
        self.ref_paused += o.ref_paused;
        self.ref_fallback += o.ref_fallback;
        self.neg_fix_gain += o.neg_fix_gain;
        self.neg_fix_found += o.neg_fix_found;
        self.prod_paused += o.prod_paused;
        self.verified += o.verified;
        self.prod_ms += o.prod_ms;
        self.prod_max_ms = self.prod_max_ms.max(o.prod_max_ms);
        self.mismatch.extend(o.mismatch.iter().cloned());
    }
}

fn real_worker(gd: &skillsim_engine::data::GameData, seed: u64, total: u64, t: u64, threads: u64) -> RealStats {
    let mut st = RealStats::default();
    let mut negative_trees: Vec<u16> = gd.skills.iter().filter(|s| s.threshold < 0).map(|s| s.tree).collect();
    negative_trees.sort();
    negative_trees.dedup();
    // Selectable skills: 10 points or more, special trees excluded
    let picks: Vec<&Skill> = gd
        .skills
        .iter()
        .filter(|s| s.threshold >= 10 && ![TREE_TORSO_UP, TREE_SECRET, TREE_TALISMAN].contains(&s.tree))
        .collect();
    let mut n = seed + t;
    while n < seed + total {
        let mut r = Rng::new(n);
        n += threads;
        // Targets: 1-5. Half are picked from trees with decorations so a solution is likely.
        let mut targets: Vec<Target> = Vec::new();
        for _ in 0..r.range(1, 5) {
            let s = loop {
                let s = picks[r.below(picks.len() as u64) as usize];
                if r.below(2) == 0 || gd.jewels.iter().any(|j| j.pos.tree == s.tree) {
                    break s;
                }
            };
            if targets.iter().all(|x| x.tree != s.tree) {
                targets.push(Target { tree: s.tree, need: s.threshold as i16 });
            }
        }
        // Owned charms: 0-4, drawn from the charm table
        let mut charms = Vec::new();
        for _ in 0..r.below(5) {
            let k = &gd.charm_kinds[r.below(gd.charm_kinds.len() as u64) as usize];
            let f = k.first[r.below(k.first.len() as u64) as usize];
            let a = Pt { tree: f.tree, pt: r.range(f.lo.max(1) as i64, f.hi as i64) as i8 };
            let mut b = Pt::default();
            if r.below(2) == 0 {
                let s = k.second[r.below(k.second.len() as u64) as usize];
                if s.tree != a.tree {
                    b = Pt { tree: s.tree, pt: if r.below(3) == 0 { -r.range(1, s.lo.max(1) as i64) } else { r.range(1, s.hi as i64) } as i8 };
                }
            }
            charms.push(Charm { skills: [a, b], slots: r.below(4) as u8 });
        }
        let kind = r.range(1, 2) as u8;
        let weapon_slots = r.below(4) as u8;
        let forbid = r.below(4) != 0;
        let owned: Vec<u16> = gd.armor.iter().filter(|_| r.below(25) == 0).map(|a| a.id).collect();
        let case = Case {
            armor: gd.armor.clone(),
            jewels: gd.jewels.clone(),
            charms,
            targets,
            weapon_slots,
            owned,
            neg_trees: negative_trees.clone(),
        };
        let base = Query {
            targets: &case.targets,
            kind,
            sex: 1,
            rare_mask: 0xffff,
            weapon_slots,
            exclude_flags: 0,
            jewel_rank: 3,
            jewel_ids: None,
            priority: 0,
            forbid_negative: forbid,
            negative_trees: &negative_trees,
            dominance: true,
            keep_defense: false,
            relaxed: true,
            fixed: [0; PARTS],
            excluded: &[],
            charms: &case.charms,
            max_results: 100,
            min_def: 0,
            min_res: [None; RES],
            group_similar: true,
            charm_kinds: None,
            owned: &case.owned,
            pareto: !case.owned.is_empty(),
        };
        st.runs += 1;
        // Preview settings
        let t1 = std::time::Instant::now();
        let mut prod = Searcher::new(&case.armor, &case.jewels, &base).unwrap();
        let prod_done = prod.run(50_000_000) == Status::Done;
        let ms = t1.elapsed().as_secs_f64() * 1e3;
        st.prod_ms += ms;
        st.prod_max_ms = st.prod_max_ms.max(ms);
        st.prod_paused += !prod_done as u32;
        // Verify results (including the weapon kind)
        let mut check = |s: &Searcher, r: &SetResult, need: Option<&Charm>| {
            let reps: [&Armor; PARTS] = core::array::from_fn(|p| &case.armor[s.armor_members(p, r.groups[p])[0] as usize]);
            for p in 0..PARTS {
                for &i in s.armor_members(p, r.groups[p]) {
                    let a = &case.armor[i as usize];
                    assert!(a.kind == 0 || a.kind == kind, "case {n}: 다른 무기 종류의 방어구");
                    assert_eq!(a.part as usize, p, "case {n}: 다른 부위의 방어구");
                }
            }
            let charm = need.or_else(|| s.charm_members(r.charm).first().map(|&i| &case.charms[i as usize]));
            if let Err(e) = verify(&case, forbid, s, r, &reps, charm) {
                panic!("case {} 목표 {:?}: {e}", n - threads, case.targets);
            }
            st.verified += 1;
        };
        for res in prod.results() {
            check(&prod, res, None);
            for v in &res.variants {
                check(&prod, v, None);
            }
        }
        for f in prod.front() {
            check(&prod, &f, None);
        }
        // Reference: exact settings with dominance, relaxed and grouping off (top only)
        // Compare with the behavior before the fix (every tree at -10 counted as negative): count cases where the fix found a better set.
        if forbid {
            let old = Query { negative_trees: &[], ..base };
            let mut s = Searcher::new(&case.armor, &case.jewels, &old).unwrap();
            if s.run(50_000_000) == Status::Done {
                let ob = s.results().iter().map(|r| r.free).max();
                let nb = prod.results().iter().map(|r| r.free).max();
                if prod_done && nb > ob {
                    st.neg_fix_gain += 1;
                    if ob.is_none() {
                        st.neg_fix_found += 1;
                    }
                }
            }
        }
        // If it takes too long, compare with a reference that only has dominance on (relaxed off) instead.
        let exact = Query { dominance: false, relaxed: false, group_similar: false, pareto: false, max_results: 1, ..base };
        let mut reference = Searcher::new(&case.armor, &case.jewels, &exact).unwrap();
        let mut ref_done = reference.run(300_000_000) == Status::Done;
        if !ref_done {
            st.ref_fallback += 1;
            let strict = Query { dominance: true, ..exact };
            reference = Searcher::new(&case.armor, &case.jewels, &strict).unwrap();
            ref_done = reference.run(2_000_000_000) == Status::Done;
        }
        if !ref_done {
            st.ref_paused += 1;
        }
        for res in reference.results() {
            check(&reference, res, None);
        }
        let best = |s: &Searcher| s.results().iter().map(|r| r.free).max();
        let (pb, rb) = (best(&prod), best(&reference));
        if prod_done && ref_done && pb != rb {
            st.mismatch.push(format!("case {} 목표 {:?} 종류 {kind} 무기 {weapon_slots} 금지 {forbid}: 미리보기 {pb:?}, 정확 {rb:?}", n - threads, case.targets));
        }
        st.feasible += rb.is_some() as u32;
        // If owned charms fail, unowned charm mode (same as the preview)
        if prod_done && prod.results().is_empty() {
            let needed = Query { charm_kinds: Some(&gd.charm_kinds), ..base };
            let mut s = Searcher::new(&case.armor, &case.jewels, &needed).unwrap();
            if s.run(50_000_000) == Status::Done && !s.results().is_empty() {
                st.needed_only += 1;
                for res in s.results().to_vec() {
                    assert!(skillsim_engine::charm::chance(&gd.charm_kinds, &res.need) > 0.0, "case {n}: 얻을 수 없는 호석");
                    check(&s, &res, Some(&res.need));
                }
            }
        }
    }
    st
}

/// Re-run the same comparison for one number that failed in the large comparison: CHECK_N=number
#[test]
#[ignore]
fn check_one() {
    let n = env("CHECK_N", 0);
    let p = match n % 4 {
        0 => SMALL,
        1 => Params { k: 4, ..SMALL },
        2 => Params { rich: true, ..SMALL },
        _ => Params { max_targets: 4, trees_hi: 10, ..SMALL },
    };
    let case = make_case(&mut Rng::new(n), p);
    check_case(&case, n, &mut Stats::default());
}

/// Where the time of one data set goes: CHECK_N=number. The engine (exact mode) and the brute-force reference, timed apart.
#[test]
#[ignore]
fn time_case() {
    let n = env("CHECK_N", 0);
    let p = match n % 4 {
        0 => SMALL,
        1 => Params { k: 4, ..SMALL },
        2 => Params { rich: true, ..SMALL },
        _ => Params { max_targets: 4, trees_hi: 10, ..SMALL },
    };
    let case = make_case(&mut Rng::new(n), p);
    for forbid in [false, true] {
        let t = std::time::Instant::now();
        let (got, _, _) = engine(&case, &Mode { forbid, ..EXACT });
        let te = t.elapsed().as_secs_f64();
        println!("case {n} forbid {forbid}: engine {te:.2}s, {} results", got.len());
        let t = std::time::Instant::now();
        let truth = brute(&case, forbid);
        println!("case {n} forbid {forbid}: brute {:.2}s, {} results", t.elapsed().as_secs_f64(), truth.len());
    }
}
