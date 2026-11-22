//! Search job that runs a little every frame. If owned charms give no results, it moves on to unowned charms.
//! The set search (Opts::SEARCH) runs to the end with no limit: the user can stop at any time with 'stop and show results'
//! (user decision 2026-11-18. The old 10s -> 20s -> 30s limits and 'continue searching' were removed).
//! Cause analysis, extra skills and alternatives keep their limits (node_limit/time_ms).
use crate::{clock, db::Db, set::Set};
use alloc::vec::Vec;
use skillsim_engine::{
    charm,
    search::{Query, Searcher, Status, Target},
    types::*,
};

pub const BASE_MS: u64 = 10_000;

/// Search conditions (values of the condition screen)
#[derive(Clone, PartialEq)]
pub struct Cond {
    pub targets: Vec<(u16, i16)>, // (tree, required points)
    pub skill_ids: Vec<u16>,
    pub kind: u8,
    pub weapon: u8,
    pub ranks: [bool; 3],
    pub forbid: bool,
    pub priority: u8,
    pub arena: bool,
    pub event: bool,
    /// Also use reward armor (no smithy recipe) that is not owned yet
    pub reward: bool,
    /// Also use deviant armor that is not owned yet (owned deviant armor is always used, from its start rank on)
    pub deviant: bool,
    /// Only armor and decorations the smithy lists right now (owned armor always counts; reward armor follows `reward`)
    pub smithy: bool,
    /// Hunter gender: 1 male, 2 female (armor for the other gender is left out)
    pub sex: u8,
    pub min_def: u16,
    pub min_res: [Option<i8>; RES],
    pub fixed: [u16; PARTS],
    pub fixed_charm: Option<usize>,
    pub excluded: Vec<u16>,
    /// Decorations the user excluded (separate from armor: the id ranges overlap)
    pub excluded_jewels: Vec<u16>,
    /// Use this one charm instead of the owned charms (saved sets, extra skills/alternatives of unowned-charm results)
    pub charm_override: Option<Charm>,
}

impl Cond {
    /// One line naming the condition (the extra-skill memory's key). charm_override is not part of it.
    pub fn encode(&self) -> alloc::string::String {
        use alloc::format;
        let list = |v: &[u16]| v.iter().map(|x| format!("{}", x)).collect::<Vec<_>>().join(",");
        format!(
            "{}#{}#{}#{}#{}{}{}#{}#{}#{}#{}#{}#{}#{}#{}#{}#{}#{}#{}#{}#{}",
            self.targets.iter().map(|(t, n)| format!("{}:{}", t, n)).collect::<Vec<_>>().join(","),
            list(&self.skill_ids),
            self.kind,
            self.weapon,
            self.ranks[0] as u8,
            self.ranks[1] as u8,
            self.ranks[2] as u8,
            self.forbid as u8,
            self.priority,
            self.arena as u8,
            self.event as u8,
            self.min_def,
            self.min_res.iter().map(|r| r.map_or(alloc::string::String::from("-"), |v| format!("{}", v))).collect::<Vec<_>>().join(","),
            list(&self.fixed),
            self.fixed_charm.map_or(alloc::string::String::from("-"), |c| format!("{}", c)),
            list(&self.excluded),
            self.reward as u8,
            self.sex,
            self.deviant as u8,
            self.smithy as u8,
            list(&self.excluded_jewels),
        )
    }
}

/// Search options
#[derive(Clone, Copy)]
pub struct Opts {
    /// End as soon as one set is found (cause analysis, extra skills)
    pub first: bool,
    /// Skip the owned-charm phase and go to unowned charms
    pub needed_only: bool,
    /// Move on to unowned charms when owned charms find nothing
    pub allow_needed: bool,
    pub dominance: bool,
    pub group: bool,
    pub max: usize,
    pub node_limit: u64,
    pub time_ms: u64,
    /// Split work with the helper threads (set search only; a helper takes one job at a time)
    pub parallel: bool,
    /// When owned charms find nothing, stop and ask before moving to unowned charms (State::AskNeeded)
    pub ask_needed: bool,
    /// An earlier search already showed that 'this condition (or a looser one) fails with owned charms': skip the owned-charm phase
    pub owned_empty: bool,
    /// Only build the searcher and stop (start with begin after deciding, by charm signature, whether to use remembered results)
    pub defer: bool,
}

impl Opts {
    pub const SEARCH: Opts = Opts {
        first: false,
        needed_only: false,
        allow_needed: true,
        dominance: true,
        group: true,
        max: 30, // results shown (100 meant little to most players: 2026-11-19)
        node_limit: u64::MAX,
        time_ms: u64::MAX,
        parallel: false,
        ask_needed: false,
        owned_empty: false,
        defer: false,
    };
    /// 'Search' on the condition screen
    pub const MAIN: Opts = Opts { parallel: true, ask_needed: true, ..Opts::SEARCH };
}

/// Set while the simulator window is closed: running searches (and helper threads) pause until it opens again,
/// so a search left running does not cost game FPS while hunting.
pub static SUSPEND: AtomicBool = AtomicBool::new(false);

// ---- Helper compute threads (parallel search) ----
// In a set search, several searchers with the same conditions run at once, each taking torso candidates from the ticket counter (Job.root).
// One is the compute thread (Job.step, inside the shared lock), the others are helper threads (helper_run(i), only their slot's lock).
// Helper count is the Forge menu's 'Search threads' - 1 (HELPER_COUNT, at most MAX_HELPERS).
// Helper searchers borrow Job.data, so they must be taken out of their slots before the Job goes away (Job::drop).
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

struct HelperWork {
    searcher: Searcher<'static>,
    done: bool,
}
struct HelperSlot {
    lock: AtomicBool,
    work: UnsafeCell<Option<HelperWork>>,
}
unsafe impl Sync for HelperSlot {}
/// Max helper slots (up to 4 search threads = compute thread + 3 helpers)
pub const MAX_HELPERS: usize = 3;
static HELPERS: [HelperSlot; MAX_HELPERS] = [const { HelperSlot { lock: AtomicBool::new(false), work: UnsafeCell::new(None) } }; MAX_HELPERS];
/// Number of helpers to use (setting: search threads - 1). Read when a search starts.
pub static HELPER_COUNT: AtomicU32 = AtomicU32::new(1);
/// The draw or game frame is waiting for the shared lock: the computation finishes only its current small piece and returns, handing over the lock.
/// (The computation held the lock for 5 ms at a time, dropping 240 fps to 140 fps with laggy input: 2026-11-18 Recomp)
pub static YIELD: AtomicBool = AtomicBool::new(false);
pub fn should_yield() -> bool {
    YIELD.load(Ordering::Relaxed)
}
/// Target time (ticks) of one computation piece. Longer pieces are shrunk: a yielding waiter waits at most this long.
const CHUNK_TICKS: u64 = clock::PER_MS / 2;

/// Are there helper threads (set when lib starts them)? Without them the search runs alone.
pub static HELPER_THREAD: AtomicBool = AtomicBool::new(false);
/// For progress (per helper): sets found, nodes used, ticks spent computing,
/// progress inside the current torso candidate (millionths, u32::MAX = choosing the torso)
static HELPER_FOUND: [AtomicU32; MAX_HELPERS] = [const { AtomicU32::new(0) }; MAX_HELPERS];
static HELPER_NODES: [AtomicU64; MAX_HELPERS] = [const { AtomicU64::new(0) }; MAX_HELPERS];
static HELPER_TICKS: [AtomicU64; MAX_HELPERS] = [const { AtomicU64::new(0) }; MAX_HELPERS];
static HELPER_PART: [AtomicU32; MAX_HELPERS] = [const { AtomicU32::new(u32::MAX) }; MAX_HELPERS];
/// Interval (ms) at which searchers exchange found sets. Keeps pruning as strong as a single searcher.
const SHARE_MS: u64 = 50;

fn with_helper<R>(i: usize, f: impl FnOnce(&mut Option<HelperWork>) -> R) -> R {
    let slot = &HELPERS[i];
    while slot.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let r = f(unsafe { &mut *slot.work.get() });
    slot.lock.store(false, Ordering::Release);
    r
}

/// Called by the i-th helper thread: run its searcher for up to ms. True if there was work.
/// The slot lock is released after every piece (about 0.25-0.5 ms): so the compute thread does not wait long on this slot while
/// holding the shared lock to exchange or merge sets (it used to hold all 5 ms, so more threads made the draw wait: 2026-11-18).
/// No helper threads are started any more (lib.rs): kept with the split search so it can come back if needed.
#[allow(dead_code)]
pub fn helper_run(i: usize, ms: u64) -> bool {
    static CHUNK: [AtomicU32; MAX_HELPERS] = [const { AtomicU32::new(2000) }; MAX_HELPERS];
    if SUSPEND.load(Ordering::Relaxed) {
        return false; // window closed: wait
    }
    let start = clock::ticks();
    let mut did = false;
    loop {
        let r = with_helper(i, |w| {
            let w = w.as_mut()?;
            if w.done {
                return None;
            }
            let chunk = CHUNK[i].load(Ordering::Relaxed) as u64;
            let before = w.searcher.nodes;
            let t = clock::ticks();
            if w.searcher.run(chunk) == Status::Done {
                w.done = true;
            }
            let spent = clock::ticks().saturating_sub(t);
            HELPER_TICKS[i].fetch_add(spent, Ordering::Relaxed);
            HELPER_NODES[i].fetch_add(w.searcher.nodes - before, Ordering::Relaxed);
            HELPER_FOUND[i].store(w.searcher.results().len() as u32, Ordering::Relaxed);
            HELPER_PART[i].store(w.searcher.branch_progress().map_or(u32::MAX, |p| (p * 1e6) as u32), Ordering::Relaxed);
            let next = if spent > CHUNK_TICKS { (chunk / 2).max(100) } else if spent * 4 < CHUNK_TICKS { (chunk * 2).min(50_000) } else { chunk };
            CHUNK[i].store(next as u32, Ordering::Relaxed);
            Some(w.done)
        });
        match r {
            None => break,
            Some(done) => {
                did = true;
                if done {
                    break;
                }
            }
        }
        if clock::ms_since(start) >= ms {
            break;
        }
    }
    did
}

pub const RANK_RARES: [(u8, u8); 3] = [(1, 3), (4, 7), (8, 11)];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Paused,
    /// No set with owned charms: asking whether to search unowned charms (Opts::ask_needed)
    AskNeeded,
    Done,
}

struct Data {
    targets: Vec<Target>,
    excluded: Vec<u16>,
    charms: Vec<Charm>,
    charm_ids: Vec<usize>,
    owned: Vec<u16>,
    negative: Vec<u16>,
    /// Decorations allowed ('smithy only' list, minus excluded ones; sorted), None when every one up to the rank is allowed
    jewel_ids: Option<Vec<u16>>,
    /// Rarity priority in stages: excluded armor per stage (index = rarity cap), empty when not staged
    stage_ex: Vec<Vec<u16>>,
}

/// Rarity priority with owned charms in stages (engine search.rs 'Rarity priority in stages')
struct Stages {
    /// Current rarity cap and the last one
    cap: u8,
    max: u8,
    /// Free-slot query of the stages (its excluded list is replaced per stage)
    query: Query<'static>,
    /// Sets kept from finished stages (rarity order), and the current stage's non-dominated sets
    kept: Vec<Set>,
    best: Vec<Set>,
}

/// Time the progress bar shows 100% between the end of the owned-charm phase and the unowned-charm question
const ASK_SHOW_MS: u64 = 450;

pub struct Job {
    // Fields are dropped in declaration order: searcher borrows data, so it comes first.
    searcher: Option<Searcher<'static>>,
    needed_query: Option<Query<'static>>,
    data: Data,
    gd: &'static skillsim_engine::data::GameData,
    pub state: State,
    /// Owned-charm (false) / unowned-charm (true) phase
    pub needed: bool,
    pub round: u32,
    window_start: u64,
    pub window_ms: u64,
    nodes_left: u64,
    node_limit: u64,
    /// End as soon as one set is found (for cause analysis and extra skills)
    first: bool,
    pub weapon: u8,
    pub min_def: u16,
    pub skill_ids: Vec<u16>,
    /// Whether the search ran to the end (did not hit a limit) when it finished
    pub complete: bool,
    /// Nodes per step. Grows and shrinks with device speed (so frames are not interrupted).
    chunk: u64,
    /// Measurements (log): start time, ticks spent computing, nodes so far (both phases)
    pub started: u64,
    pub busy_ticks: u64,
    pub nodes: u64,
    /// Number of charms used
    pub charm_count: usize,
    /// Split-work ticket counter (shared by the searchers). Boxed so its address does not change.
    root: alloc::boxed::Box<AtomicU32>,
    /// Does it split work with helper threads, and does a helper currently hold a searcher
    parallel: bool,
    /// Number of helpers currently holding a searcher (0 = alone)
    helpers: usize,
    ask_needed: bool,
    /// Last time found sets were exchanged with the helpers, and the (mine, helpers') counts then
    last_share: u64,
    shared_counts: [(usize, usize); MAX_HELPERS],
    /// Charm signature of the owned-charm search (if equal, the owned-charm results for that condition are equal)
    pub charm_sig: Option<skillsim_engine::search::CharmSignature>,
    /// First-phase query kept until begin
    first_query: Option<Query<'static>>,
    /// Rarity priority with owned charms: the owned-charm phase runs in stages
    stages: Option<Stages>,
    max_results: usize,
    /// The owned-charm phase was skipped from memory / number of sets seeded from earlier results
    pub owned_skipped: bool,
    pub seeded: usize,
    /// Measurements (log): ms and nodes until the owned-charm phase ended, ms waited at the question
    pub owned_ms: u64,
    pub owned_nodes: u64,
    pub ask_wait_ms: u64,
    ask_started: u64,
    /// PC query.exe arguments for the same conditions (logged for reproduction)
    pub args: alloc::string::String,
}

/// Search results: (results, non-dominated sets, unowned charms or not, searched to the end)
#[derive(Clone)]
pub struct Found {
    pub sets: Vec<Set>,
    pub best: Vec<Set>,
    pub needed: bool,
    pub complete: bool,
}

impl Job {
    /// first: end at the first set. needed_only: skip the owned-charm phase. node_limit/time_ms: initial limits.
    pub fn new(db: &Db, c: &Cond, o: Opts) -> Option<Job> {
        let (first, node_limit, time_ms) = (o.first, o.node_limit, o.time_ms);
        let mut rare_mask = 0u16;
        for (on, (lo, hi)) in c.ranks.iter().zip(RANK_RARES) {
            if *on {
                for r in lo..=hi {
                    rare_mask |= 1 << r;
                }
            }
        }
        // Rarity 11 is deviant armor only: let it through the rarity mask and filter it below by its start rank
        if c.ranks.iter().any(|&on| on) {
            rare_mask |= 1 << 11;
        }
        let jewels = db.jewel_use(&c.ranks, c.smithy, &c.excluded_jewels);
        let jewel_rank = jewels.rank;
        let jewel_ids: Option<Vec<u16>> = jewels.ids(db);
        let charm_ids: Vec<usize> = match (c.charm_override, c.fixed_charm) {
            (Some(_), _) => alloc::vec![usize::MAX],
            (None, Some(i)) if i < db.inv().charms.len() => alloc::vec![i],
            _ => (0..db.inv().charms.len()).collect(),
        };
        // Reward armor that is not owned cannot be made: left out unless the user allows it
        let mut excluded = c.excluded.clone();
        excluded.push(crate::db::PLACEHOLDER_ARMOR);
        if !c.reward {
            let owned = &db.inv().owned;
            excluded.extend(db.reward_armor.iter().copied().filter(|id| !owned.contains(id)));
        }
        // Deviant armor: usable when an enabled rank is at or above its start rank, and (option on or owned)
        let owned = &db.inv().owned;
        // Arena / event (coin) armor: owned pieces are always used, the option adds the unowned ones
        excluded.extend(
            db.gd
                .armor
                .iter()
                .filter(|a| (a.flags & FLAG_ARENA != 0 && !c.arena || a.flags & FLAG_EVENT_COIN != 0 && !c.event) && !owned.contains(&a.id))
                .map(|a| a.id),
        );
        // Smithy only: armor with a recipe must be on the smithy list (or owned). Reward armor has no recipe: it follows `reward`.
        if c.smithy {
            if let Some(s) = db.inv().smithy.as_ref() {
                excluded.extend(
                    db.gd
                        .armor
                        .iter()
                        .filter(|a| !owned.contains(&a.id) && !s.armor.contains(&a.id) && !db.reward_armor.contains(&a.id))
                        .map(|a| a.id),
                );
            }
        }
        excluded.extend(
            db.gd
                .armor
                .iter()
                .filter(|a| {
                    let start = a.deviant_rank() as usize;
                    start > 0 && !(c.ranks[start - 1..].iter().any(|&on| on) && (c.deviant || owned.contains(&a.id)))
                })
                .map(|a| a.id),
        );
        let data = Data {
            targets: c.targets.iter().map(|&(tree, need)| Target { tree, need }).collect(),
            excluded,
            charms: match c.charm_override {
                Some(ch) => alloc::vec![ch],
                None => charm_ids.iter().map(|&i| db.inv().charms[i]).collect(),
            },
            charm_ids,
            owned: db.inv().owned.iter().copied().collect(),
            negative: db.negative_trees.clone(),
            jewel_ids,
            stage_ex: Vec::new(),
        };
        // The borrowed values are heap buffers of data. They do not change while the Job lives, and the searcher is dropped first.
        let st = |v: &[Target]| unsafe { core::mem::transmute::<&[Target], &'static [Target]>(v) };
        let su = |v: &[u16]| unsafe { core::mem::transmute::<&[u16], &'static [u16]>(v) };
        let sc = |v: &[Charm]| unsafe { core::mem::transmute::<&[Charm], &'static [Charm]>(v) };
        let q = Query {
            negative_trees: su(&data.negative),
            targets: st(&data.targets),
            kind: c.kind,
            sex: if c.sex == 2 { 2 } else { 1 },
            rare_mask,
            weapon_slots: c.weapon,
            // Coin armor is excluded by id above (owned pieces stay), not by flag
            exclude_flags: 0,
            jewel_rank,
            jewel_ids: data.jewel_ids.as_deref().map(|v| unsafe { core::mem::transmute::<&[u16], &'static [u16]>(v) }),
            priority: c.priority,
            forbid_negative: c.forbid,
            dominance: o.dominance,
            // Minimum defense needs the higher-defense armor kept. Free-slot priority keeps it too, so the recommended
            // sets are exact down to defense ('thorough', always on since 2026-11-19); with rarity priority that made the
            // search up to ~50x larger (70M nodes for two skills), so it is off there.
            keep_defense: c.min_def > 0 || c.priority == 0,
            relaxed: true,
            fixed: c.fixed,
            excluded: su(&data.excluded),
            charms: sc(&data.charms),
            max_results: o.max,
            min_def: c.min_def,
            min_res: c.min_res,
            group_similar: o.group,
            charm_kinds: None,
            owned: su(&data.owned),
            pareto: o.group && !data.owned.is_empty(),
        };
        // Searches with a fixed charm do not look for unowned charms.
        let gd = db.gd;
        let needed_query = (o.allow_needed && c.fixed_charm.is_none() && c.charm_override.is_none())
            // The defense keeping of 'Thorough' is for recommended sets (collected only in the owned-charm phase): not used in the unowned-charm phase
            // (using it grew that phase from 3.91M to 58.8M nodes: 2026-11-18)
            .then(|| Query { charm_kinds: Some(&gd.charm_kinds), keep_defense: c.min_def > 0, ..q });
        let start_needed = o.needed_only && needed_query.is_some();
        let (first_query, needed_query) = if start_needed { (needed_query.unwrap(), None) } else { (q, needed_query) };
        // Rarity priority, owned charms, a full search (not 'first set only', not one charm like alternatives / extra skills)
        let mut data = data;
        let mut stages = None;
        let first_query = if c.priority == 2 && !start_needed && !o.first && c.charm_override.is_none() {
            let max = skillsim_engine::search::stage_max_rarity(&gd.armor);
            data.stage_ex = (0..=max)
                .map(|cap| data.excluded.iter().copied().chain(skillsim_engine::search::stage_excluded(&gd.armor, &data.owned, cap)).collect())
                .collect();
            let query = Query { priority: 0, ..first_query };
            stages = Some(Stages { cap: 0, max, query, kept: Vec::new(), best: Vec::new() });
            Query { excluded: su(&data.stage_ex[0]), ..query }
        } else {
            first_query
        };
        let searcher = Searcher::new(&gd.armor, &gd.jewels, &first_query).ok()?;
        let charm_count = data.charms.len();
        let parallel = o.parallel && !o.first && HELPER_THREAD.load(Ordering::Acquire);
        // For the log (reproducing slow conditions on PC): main searches only. Building it for frequent small searches (alternatives, cause analysis) is waste.
        let args = if !o.parallel { alloc::string::String::new() } else {
            use alloc::format;
            let j = |v: &[u16]| v.iter().map(|x| format!("{}", x)).collect::<Vec<_>>().join(",");
            format!(
                "thorough={} kind={} sex={} weapon={} forbid={} rare={} jewelrank={} jewels={} priority={} exflags={} mindef={} minres={} targets={} fixed={} excluded={} owned={} pareto={} max={} group={} dominance={} needed={} charms={}",
                (c.priority == 0) as u8,
                first_query.kind,
                first_query.sex,
                first_query.weapon_slots,
                first_query.forbid_negative as u8,
                first_query.rare_mask,
                first_query.jewel_rank,
                data.jewel_ids.as_deref().map_or(alloc::string::String::from("-"), j),
                c.priority,
                first_query.exclude_flags,
                first_query.min_def,
                first_query.min_res.iter().map(|r| r.map_or(alloc::string::String::from("-"), |v| format!("{}", v))).collect::<Vec<_>>().join(","),
                data.targets.iter().map(|t| format!("{}:{}", t.tree, t.need)).collect::<Vec<_>>().join(","),
                j(&first_query.fixed),
                j(&data.excluded),
                j(&data.owned),
                first_query.pareto as u8,
                first_query.max_results,
                first_query.group_similar as u8,
                first_query.dominance as u8,
                (needed_query.is_some() || start_needed) as u8,
                data.charms.iter().map(|c| format!("{}:{}:{}:{}:{}", c.skills[0].tree, c.skills[0].pt, c.skills[1].tree, c.skills[1].pt, c.slots)).collect::<Vec<_>>().join(";"),
            )
        };
        Some(Job {
            searcher: Some(searcher),
            needed_query,
            data,
            gd,
            state: State::Running,
            needed: start_needed,
            round: 0,
            window_start: clock::ticks(),
            window_ms: time_ms,
            nodes_left: node_limit,
            node_limit,
            first,
            weapon: c.weapon,
            min_def: c.min_def,
            skill_ids: c.skill_ids.clone(),
            complete: false,
            chunk: 500,
            started: clock::ticks(),
            busy_ticks: 0,
            nodes: 0,
            charm_count,
            args,
            root: alloc::boxed::Box::new(AtomicU32::new(0)),
            parallel,
            helpers: 0,
            ask_needed: o.ask_needed,
            last_share: 0,
            shared_counts: [(usize::MAX, usize::MAX); MAX_HELPERS],
            owned_skipped: false,
            seeded: 0,
            charm_sig: None,
            first_query: None,
            stages,
            max_results: o.max,
            owned_ms: 0,
            owned_nodes: 0,
            ask_wait_ms: 0,
            ask_started: 0,
        })
        .map(|mut j| {
            if !j.needed {
                j.charm_sig = j.searcher.as_ref().map(|s| s.charm_signature());
            }
            j.first_query = Some(Query { ..first_query });
            if !o.defer {
                j.begin(o.owned_empty);
            }
            j
        })
    }

    /// Start the search (call directly when built with Opts::defer). owned_empty: skip the owned-charm phase.
    pub fn begin(&mut self, owned_empty: bool) {
        let Some(first) = self.first_query.take() else { return };
        if owned_empty && !self.needed && self.needed_query.is_some() {
            // Skip the owned-charm phase: ask right away (MAIN) or go to unowned charms
            self.owned_skipped = true;
            if self.ask_needed {
                self.state = State::AskNeeded;
                self.ask_started = clock::ticks();
            } else {
                let q = self.needed_query.take().unwrap();
                self.searcher = Searcher::new(&self.gd.armor, &self.gd.jewels, &q).ok();
                self.needed = true;
                self.split_now(&q);
            }
            return;
        }
        self.split_now(&first);
    }

    /// Start from an earlier search's sets (owned-charm phase, before searching only). Only those valid now get in (engine seed).
    /// Returns the number of sets added.
    pub fn seed(&mut self, db: &Db, sets: &[Set]) -> usize {
        if self.needed || self.state != State::Running || self.data.charm_ids.first() == Some(&usize::MAX) {
            return 0;
        }
        let Some(s) = self.searcher.as_mut() else { return 0 };
        let mut n = 0;
        for set in sets {
            if set.saved_charm.is_some() {
                continue; // Saved sets / unowned-charm results: the charm is not in the owned list
            }
            let Some(armor) = (0..PARTS).map(|p| db.armor_pos(set.head(p))).collect::<Option<Vec<u16>>>() else { continue };
            let charm = match usize::try_from(set.charm) {
                Ok(c) => match self.data.charm_ids.iter().position(|&x| x == c) {
                    Some(i) => Some(i as u16),
                    None => continue,
                },
                Err(_) => None,
            };
            if s.seed(core::array::from_fn(|p| armor[p]), charm) {
                n += 1;
            }
        }
        self.seeded = n;
        n
    }

    /// Start split work: reset the ticket counter and hand a searcher with the same conditions to each helper.
    fn split_now(&mut self, _q: &Query<'static>) {
        if !self.parallel || self.state != State::Running {
            return;
        }
        let n = (HELPER_COUNT.load(Ordering::Relaxed) as usize).min(MAX_HELPERS);
        if n == 0 {
            return;
        }
        self.root.store(0, Ordering::Relaxed);
        // The ticket counter keeps its address until the Job ends (boxed). Helper searchers are taken out before Job::drop.
        let root: &'static AtomicU32 = unsafe { &*(&*self.root as *const AtomicU32) };
        if let Some(s) = self.searcher.as_mut() {
            s.share_root(root);
        }
        for i in 0..n {
            // Copy the compute thread's searcher (running Searcher::new per helper took 30-40 ms each on Recomp)
            let Some(mut other) = self.searcher.as_ref().map(|s| s.fork()) else { break };
            other.share_root(root);
            HELPER_FOUND[i].store(0, Ordering::Relaxed);
            HELPER_NODES[i].store(0, Ordering::Relaxed);
            HELPER_PART[i].store(u32::MAX, Ordering::Relaxed);
            with_helper(i, |w| *w = Some(HelperWork { searcher: other, done: false }));
            self.shared_counts[i] = (usize::MAX, usize::MAX);
            self.helpers = i + 1;
        }
    }

    /// Take the helpers' searchers back and merge them. With wait=false, returns false without taking them if any is still running.
    fn join_helper(&mut self, wait: bool) -> bool {
        if self.helpers == 0 {
            return true;
        }
        if !wait && !(0..self.helpers).all(|i| with_helper(i, |w| w.as_ref().is_none_or(|w| w.done))) {
            return false;
        }
        for i in 0..self.helpers {
            let Some(h) = with_helper(i, |w| w.take()) else { continue };
            self.busy_ticks += HELPER_TICKS[i].swap(0, Ordering::Relaxed);
            if let Some(s) = self.searcher.as_mut() {
                let before = s.nodes;
                s.merge(h.searcher);
                self.nodes += s.nodes - before;
            }
        }
        self.helpers = 0;
        true
    }

    /// 'Yes' after asking whether to search with unowned charms
    pub fn continue_needed(&mut self) {
        if self.state != State::AskNeeded {
            return;
        }
        self.ask_wait_ms += clock::ms_since(self.ask_started);
        if let Some(q) = self.needed_query.take() {
            self.searcher = None;
            self.searcher = Searcher::new(&self.gd.armor, &self.gd.jewels, &q).ok();
            self.needed = true;
            self.split_now(&q);
            self.state = State::Running;
        } else {
            self.state = State::Done;
        }
    }

    /// When the owned-charm phase ends and the unowned-charm question comes, show the bar filling to 100% briefly first
    /// (when the owned-charm phase ended very quickly, it jumped from 0% straight to the question, which looked odd)
    pub fn ask_ready(&self) -> bool {
        self.state == State::AskNeeded && (self.owned_skipped || clock::ms_since(self.ask_started) >= ASK_SHOW_MS)
    }

    pub fn progress(&self) -> f32 {
        if self.state == State::AskNeeded {
            return 1.; // The owned-charm phase was searched to the end
        }
        let Some(s) = self.searcher.as_ref() else { return 1. };
        // Staged search: each stage an equal share
        let share = |p: f32| match (&self.stages, self.needed) {
            (Some(st), false) => ((st.cap as f32 + p) / (st.max as f32 + 1.)).min(0.999),
            _ => p,
        };
        if self.helpers == 0 {
            return share(s.progress());
        }
        // Split work: (finished torsos + each searcher's progress inside its current torso) / torso count
        let len = s.root_len().max(1) as f32;
        let claimed = (self.root.load(Ordering::Relaxed) as f32).min(len);
        let (mut active, mut part) = (0f32, 0f32);
        if let Some(p) = s.branch_progress() {
            active += 1.;
            part += p;
        }
        for i in 0..self.helpers {
            let v = HELPER_PART[i].load(Ordering::Relaxed);
            if v != u32::MAX {
                active += 1.;
                part += v as f32 / 1e6;
            }
        }
        share(((claimed - active).max(0.) + part) / len)
    }

    /// Exchange the sets found so far with the helpers (for pruning, every SHARE_MS)
    fn share(&mut self) {
        if self.helpers == 0 || clock::ms_since(self.last_share) < SHARE_MS {
            return;
        }
        self.last_share = clock::ticks();
        let Some(s) = self.searcher.as_mut() else { return };
        // The compute thread's searcher collects and redistributes in the middle (helpers pass sets to each other through it).
        // Skip when neither side found anything new since last time (it is inside the shared lock, so keep it short).
        for i in 0..self.helpers {
            let seen = &mut self.shared_counts[i];
            with_helper(i, |w| {
                if let Some(w) = w.as_mut() {
                    let now = (s.found_count(), w.searcher.found_count());
                    if now != *seen {
                        s.absorb(&w.searcher);
                        w.searcher.absorb(s);
                        *seen = (s.found_count(), w.searcher.found_count());
                    }
                }
            });
        }
    }
    pub fn found(&self) -> usize {
        // Searchers exchange found sets, so do not add them; take the larger (close to the total without overlap)
        let helper = (0..self.helpers).map(|i| HELPER_FOUND[i].load(Ordering::Relaxed) as usize).max().unwrap_or(0);
        let kept = match (&self.stages, self.needed) {
            (Some(st), false) => st.kept.len(),
            _ => 0,
        };
        kept + self.searcher.as_ref().map_or(0, |s| s.results().len()).max(helper)
    }
    /// (free slots, defense) of every result including grouped sets (for tests)
    pub fn flat_ranks(&self) -> Vec<(u8, u16)> {
        let mut v: Vec<(u8, u16)> = self
            .searcher
            .as_ref()
            .map(|s| s.results().iter().flat_map(|r| core::iter::once(r).chain(r.variants.iter())).map(|r| (r.free, r.def)).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Nodes so far including the helpers (progress and log)
    pub fn total_nodes(&self) -> u64 {
        self.nodes + (0..self.helpers).map(|i| HELPER_NODES[i].load(Ordering::Relaxed)).sum::<u64>()
    }
    /// Run only this frame's share (at most frame_ms).
    pub fn step(&mut self, frame_ms: u64) {
        if self.state != State::Running {
            return;
        }
        let frame = clock::ticks();
        loop {
            let Some(s) = self.searcher.as_mut() else { return };
            if self.first {
                s.skip_chance_tables(); // stops at the first set: the chance tables would be built and thrown away
            }
            let before = s.nodes;
            let t = clock::ticks();
            let st = s.run(self.chunk.min(self.nodes_left.max(1)));
            let s_nodes = s.nodes;
            // Shrink when one step takes over half the frame share, grow when it uses less than 1/8
            // (Recomp runs the plugin in an interpreter, hundreds of times slower than on PC)
            let spent = clock::ticks().saturating_sub(t);
            self.busy_ticks += spent;
            self.nodes += s_nodes - before;
            self.share();
            let s = self.searcher.as_mut().unwrap();
            // One piece is about 0.25-0.5 ms: so it can be handed over within that time when the screen waits for the lock
            let budget = (frame_ms.min(1000) * clock::PER_MS).min(CHUNK_TICKS * 2);
            if spent * 2 > budget {
                self.chunk = (self.chunk / 2).max(100);
            } else if spent * 4 < budget {
                self.chunk = (self.chunk * 2).min(50_000);
            }
            self.nodes_left = self.nodes_left.saturating_sub(s.nodes - before);
            let hit = self.first && !s.results().is_empty();
            if st == Status::Done || hit {
                // With split work, this phase ends only when the helpers are done too (all tickets are taken, so soon)
                if !self.join_helper(false) {
                    return;
                }
                if !self.needed && self.next_stage() {
                    continue;
                }
                let empty = match (&self.stages, self.needed) {
                    (Some(st), false) => st.kept.is_empty(),
                    _ => self.searcher.as_ref().is_none_or(|s| s.results().is_empty()),
                };
                if !hit && empty && !self.needed && self.needed_query.is_some() {
                    self.owned_ms = clock::ms_since(self.started);
                    self.owned_nodes = self.nodes;
                    if self.ask_needed {
                        self.ask_started = clock::ticks();
                        // Nothing with owned charms: ask whether to search with unowned charms
                        self.state = State::AskNeeded;
                        return;
                    }
                    let q = self.needed_query.take().unwrap();
                    // Nothing with owned charms: go to unowned charms
                    self.searcher = None;
                    self.searcher = Searcher::new(&self.gd.armor, &self.gd.jewels, &q).ok();
                    self.needed = true;
                    self.split_now(&q);
                    continue;
                }
                self.state = State::Done;
                self.complete = true;
                return;
            }
            if self.nodes_left == 0 || clock::ms_since(self.window_start) >= self.window_ms {
                self.state = State::Paused;
                return;
            }
            if clock::ms_since(frame) >= frame_ms || should_yield() {
                return;
            }
        }
    }

    /// Staged search: keep the finished stage's sets of exactly its rarity, then start the next stage while the results
    /// are not full. false when the stages are over (or not staged).
    fn next_stage(&mut self) -> bool {
        let gd = self.gd;
        let (cap, max) = match &self.stages {
            Some(st) => (st.cap, st.max),
            None => return false,
        };
        let Some(s) = self.searcher.as_ref() else { return false };
        let room = self.max_results.saturating_sub(self.stages.as_ref().map_or(0, |st| st.kept.len()));
        let new: Vec<Set> = s
            .sorted_results()
            .iter()
            .filter(|r| skillsim_engine::search::set_rarity(s, &gd.armor, &self.data.owned, r) == cap)
            .take(room)
            .map(|r| self.convert(gd, s, r))
            .collect();
        let best: Vec<Set> = s.front().iter().map(|r| self.convert(gd, s, r)).collect();
        let st = self.stages.as_mut().unwrap();
        st.kept.extend(new);
        st.best = best;
        if st.kept.len() >= self.max_results || cap >= max {
            return false;
        }
        st.cap = cap + 1;
        let q = Query { excluded: unsafe { core::mem::transmute::<&[u16], &'static [u16]>(&self.data.stage_ex[st.cap as usize]) }, ..st.query };
        self.searcher = None;
        self.searcher = Searcher::new(&gd.armor, &gd.jewels, &q).ok();
        self.split_now(&q);
        self.searcher.is_some()
    }

    /// Continue searching: raise the limit (10 s first, then 20 s, then fixed at 30 s).
    pub fn resume(&mut self) {
        self.round += 1;
        let k = (self.round + 1).min(3) as u64;
        self.window_ms = BASE_MS * k;
        self.nodes_left = self.node_limit * k;
        self.window_start = clock::ticks();
        self.state = State::Running;
    }

    /// End with the results so far (including the helpers' share).
    pub fn finish(&mut self) {
        self.join_helper(true);
        self.state = State::Done;
    }

    pub fn result(&self, db: &Db) -> Found {
        // Staged owned-charm search: the kept sets (a search stopped mid-stage adds that stage's sets so far)
        if let (Some(st), false) = (&self.stages, self.needed) {
            let mut sets = st.kept.clone();
            let mut best = st.best.clone();
            if self.state != State::Done || !self.complete {
                if let Some(s) = self.searcher.as_ref() {
                    let room = self.max_results.saturating_sub(sets.len());
                    sets.extend(
                        s.sorted_results()
                            .iter()
                            .filter(|r| skillsim_engine::search::set_rarity(s, &db.gd.armor, &self.data.owned, r) == st.cap)
                            .take(room)
                            .map(|r| self.convert(db.gd, s, r)),
                    );
                    best = s.front().iter().map(|r| self.convert(db.gd, s, r)).collect();
                }
            }
            return Found { sets, best, needed: false, complete: self.complete };
        }
        let Some(s) = self.searcher.as_ref() else {
            return Found { sets: Vec::new(), best: Vec::new(), needed: self.needed, complete: self.complete };
        };
        let sets = s.sorted_results().iter().map(|r| self.convert(db.gd, s, r)).collect();
        let best = s.front().iter().map(|r| self.convert(db.gd, s, r)).collect();
        Found { sets, best, needed: self.needed, complete: self.complete }
    }

    fn convert(&self, gd: &skillsim_engine::data::GameData, s: &Searcher, r: &skillsim_engine::search::SetResult) -> Set {
        let parts = core::array::from_fn(|p| {
            let members = s.armor_members(p, r.groups[p]);
            let head = gd.armor[members[0] as usize].def;
            // Only alternatives whose total defense still meets the condition (the first of a group has the highest defense)
            let ids: Vec<u16> = members
                .iter()
                .map(|&i| &gd.armor[i as usize])
                .filter(|a| r.def - head + a.def >= self.min_def)
                .map(|a| a.id)
                .collect();
            if ids.is_empty() { alloc::vec![gd.armor[members[0] as usize].id] } else { ids }
        });
        let loc = |p: &skillsim_engine::search::Placed| if p.torso { crate::set::LOC_TORSO } else if p.charm { crate::set::LOC_CHARM } else { crate::set::LOC_ANY };
        let jewels = r.jewels[..r.nj as usize].iter().map(|p| (s.jewel(p.jewel).id, loc(p))).collect();
        let charm = s.charm_members(r.charm).first().map_or(-1, |&i| match self.data.charm_ids.get(i as usize) {
            Some(&c) if c != usize::MAX => c as i32,
            _ => -1,
        });
        let override_charm = (self.data.charm_ids.first() == Some(&usize::MAX)).then(|| self.data.charms[0]);
        let (saved_charm, charm_rare) = if let Some(c) = override_charm {
            (Some(c), 0)
        } else if self.needed {
            let rare = charm::best_kind(&gd.charm_kinds, &r.need).map_or(0, |(i, _)| charm::rarity(i, &gd.charm_kinds[i], &r.need));
            (Some(r.need), rare)
        } else {
            (None, 0)
        };
        Set {
            free: r.free,
            bonus: r.bonus,
            def: r.def,
            charm: if self.needed { -1 } else { charm },
            parts,
            jewels,
            saved_charm,
            weapon: self.weapon,
            targets: self.skill_ids.clone(),
            needed: self.needed,
            chance: r.chance as f64 / 1e15,
            charm_rare,
            tags: 0,
        }
    }
}

impl Drop for Job {
    /// Helper searchers borrow this Job's data: take them out of the helper slots and drop them before data is freed.
    fn drop(&mut self) {
        for i in 0..self.helpers {
            with_helper(i, |w| *w = None);
            HELPER_TICKS[i].store(0, Ordering::Relaxed);
        }
    }
}
