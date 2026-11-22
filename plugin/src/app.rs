//! Screen state and controller input. A port of the preview preview/ui.py (must behave the same).
use crate::{
    db::Db,
    generated::strings as k,
    job::{Cond, Found, Job, Opts, State as JobState},
    set::Set,
};
use alloc::{string::String, vec::Vec};
use skillsim_engine::types::*;

pub const MAX_TARGETS: usize = 8;
/// Window size relative to the game picture, fixed (the scale and position settings were removed, 2026-11-20).
/// Leaves room for other mods' panels (Better Skillinfo etc.) on the left: the window sits on the right.
pub const UI_SCALE: f32 = 0.8;
pub const FIX_ROWS: usize = 6;
pub const PART_ROWS: usize = 7;
pub const CHARM_ROW: usize = 5;
pub const RES_STEPS: [Option<i8>; 6] = [None, Some(0), Some(5), Some(10), Some(15), Some(20)];
pub const MIN_DEF: [u16; 7] = [0, 300, 400, 500, 600, 700, 800];
pub const EXTRA_ROWS: usize = 9;
/// Rows of the 'Armor to use' popup: low, high, G, deviant, arena, quest reward, event
pub const ARMOR_USE: usize = 7;
/// Extra-skill results kept for the session
const EXTRA_CACHE: usize = 32;
pub const MAX_SAVED: usize = 48;
pub const LIST_ROWS: usize = 7; // room below the list for the 'why recommended' box (larger text)
pub const SAVED_ROWS: usize = 8;
pub const SKILL_ROWS: usize = 11;
pub const RECENT_MAX: usize = SKILL_ROWS;
pub const MANAGE_ROWS: usize = 11;
pub const DIAG_BUDGET: u64 = 30_000_000;
pub const DIAG_TIME_MS: u64 = 5_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    X,
    Y,
    L,
    R,
    ZL,
    ZR,
    Plus,
    Minus,
}
use Button::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Opt {
    Kind,
    Weapon,
    Armor,
    Smithy,
    MinDef,
    Resist,
    Priority,
    Forbid,
    Search,
}
pub const OPTIONS: [Opt; 9] = [Opt::Kind, Opt::Weapon, Opt::Armor, Opt::Smithy, Opt::MinDef, Opt::Resist, Opt::Priority, Opt::Forbid, Opt::Search];
impl Opt {
    /// Number of dropdown choices (0 if not a dropdown)
    pub fn choices(self) -> usize {
        match self {
            Opt::Kind => 2,
            Opt::Weapon => 4,
            Opt::Priority => 2,
            Opt::MinDef => MIN_DEF.len(),
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Main,
    Skills,
    Results,
    Saved,
    Manage,
    Box,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Parts,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Excluded,
    ResetTargets,
    ResetFixed,
}
/// Saved sets open with − (same as the results screen), so they are not in this menu
pub const MENU: [MenuItem; 3] = [MenuItem::Excluded, MenuItem::ResetTargets, MenuItem::ResetFixed];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    ResetTargets,
    ResetFixed,
    Release(u16),
    ReleaseAll,
    Save,
    Exclude(u16),
    /// Exclude a decoration (from the exclude list of a part row)
    ExcludeJewel(u16),
    ReleaseJewel(u16),
    DeleteSaved(usize),
    ExtraAny,
    LeaveResults,
}
impl Confirm {
    /// Is 'yes' the default cursor (others are hard to undo, so 'no')
    fn yes_default(self) -> bool {
        matches!(self, Confirm::Save | Confirm::Exclude(_) | Confirm::ExcludeJewel(_) | Confirm::ExtraAny)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    NoResult,
    NoResultFixed,
    Needed,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    DiagWait,
    Loading,
    /// No set with owned charms: also search with unowned charms?
    AskNeeded,
    Message,
    Confirm,
    Diag,
    Notice,
}
/// An entry of the results' exclude list (X on a part row): the part's armor and the decorations placed in that part
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExItem {
    Armor(u16),
    Jewel(u16),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FixItem {
    Equipped,
    Box,
    Release,
}

/// Cause-analysis suggestions
#[derive(Clone)]
pub enum DiagLabel {
    Forbid,
    MinDef,
    Res,
    Fixed,
    Excluded,
    Smithy,
    Lower(u16, u16),
    Remove(u16),
}
#[derive(Clone)]
pub struct DiagChange {
    forbid: Option<bool>,
    mindef: Option<usize>,
    res: Option<[usize; RES]>,
    fixed: Option<([u16; PARTS], Option<usize>)>,
    /// Excluded armor and decorations (both released together)
    excluded: Option<(Vec<u16>, Vec<u16>)>,
    targets: Option<Vec<u16>>,
    smithy: Option<bool>,
}
impl DiagChange {
    const NONE: DiagChange = DiagChange { forbid: None, mindef: None, res: None, fixed: None, excluded: None, targets: None, smithy: None };
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    Todo,
    Ok,
    No,
    Partial,
}
pub struct DiagRun {
    cands: Vec<(DiagLabel, DiagChange)>,
    /// Condition to test per candidate (prepared so the searcher can be built outside the lock)
    conds: Vec<Cond>,
    status: Vec<Check>,
    job: Option<(usize, Job)>,
    pass: u8,
    next: usize,
    start: u64,
    pub cancel: bool,
}

/// Computes one set's alternatives part by part (the compute thread, outside the lock, a little at a time).
/// The set under the cursor gets the lists (for the popup); other sets only get counts (the 'N alternatives' tag) in advance.
pub struct AltRun {
    key: usize,
    /// Number raised whenever the result list changes: discard on mismatch
    generation: u32,
    cur: Set,
    part: usize,
    job: Option<Job>,
    /// Also build the popup lists (only for the set under the cursor)
    want_lists: bool,
    lists: Vec<AltPart>,
    /// Per part: armor choices including the current armor (low 16 bits) and worthwhile alternatives (high 16 bits:
    /// not strictly worse, excluding the current armor) -- see alt_count
    counts: [u32; PARTS],
    /// Owned armor at the start (alt_order runs outside the lock and must not read the inventory)
    owned: alloc::collections::BTreeSet<u16>,
    /// Search condition for the compute thread to build outside the lock (take_alt_build), and whether it is being built
    pending: Option<Cond>,
    waiting: bool,
}

/// Alternatives of part p in display order, and where the strictly worse ones start (drawn dimmed).
/// The current group comes first; other armor (first occurrence) follows, sorted by free slots, ownership and defense.
/// Strictly worse: an entry above has free slots, defense and ownership at least as good (sweep with the best defense so far).
pub fn alt_order(
    db: &crate::db::Db,
    owned: &alloc::collections::BTreeSet<u16>,
    cur: &Set,
    p: usize,
    others: impl Iterator<Item = (u16, u8, u32)>,
) -> (Vec<(u16, u8, Option<u32>)>, usize) {
    let mut out: Vec<(u16, u8, Option<u32>)> = cur.parts[p].iter().map(|&id| (id, cur.free, None)).collect();
    let mine = &cur.parts[p];
    let mut seen: alloc::collections::BTreeSet<u16> = mine.iter().copied().collect();
    for (id, free, set) in others {
        if seen.insert(id) {
            out.push((id, free, Some(set)));
        }
    }
    out.sort_by_key(|x| (core::cmp::Reverse(x.1), !owned.contains(&x.0), core::cmp::Reverse(db.armor(x.0).map_or(0, |a| a.def))));
    let (mut best_all, mut best_owned) = (None::<u16>, None::<u16>);
    let mut keep = Vec::with_capacity(out.len());
    let mut worse = Vec::new();
    for x in out {
        let def = db.armor(x.0).map_or(0, |a| a.def);
        let is_owned = owned.contains(&x.0);
        let beaten = if is_owned { best_owned.is_some_and(|d| d >= def) } else { best_all.is_some_and(|d| d >= def) };
        if is_owned {
            best_owned = Some(best_owned.map_or(def, |d| d.max(def)));
        }
        best_all = Some(best_all.map_or(def, |d| d.max(def)));
        if beaten && x.2.is_some() { worse.push(x) } else { keep.push(x) }
    }
    let from = keep.len();
    keep.extend(worse);
    (keep, from)
}

/// Packed count for the tags: total choices (low 16 bits) and worthwhile alternatives (high 16 bits)
fn alt_count(cur: &Set, p: usize, order: &[(u16, u8, Option<u32>)], worse_from: usize) -> u32 {
    let useful = order[..worse_from].iter().filter(|x| x.0 != cur.head(p)).count().min(0xffff) as u32;
    (order.len().min(0xffff) as u32) | useful << 16
}

/// Packed count for a part without a search (fixed part or no searcher): its own group only
fn group_count(cur: &Set, p: usize) -> u32 {
    let n = cur.parts[p].len().min(0xffff) as u32;
    n | n.saturating_sub(1) << 16
}

/// Alternatives for one part: each found set is stored once and the armor ids point at it.
/// (Cloning the whole set per armor id used hundreds of KB when a part had 1000+ alternatives.)
#[derive(Default)]
pub struct AltPart {
    pub sets: Vec<Set>,
    /// (armor id, free slots, index into `sets`)
    pub items: Vec<(u16, u8, u32)>,
}

impl AltRun {
    /// Outside the lock: run the current part's alternative search for up to ms and, when done, store the count (and the list)
    pub fn work(&mut self, db: &'static crate::db::Db, ms: u64) {
        let Some(job) = self.job.as_mut() else { return };
        job.step(ms);
        if job.state != JobState::Running {
            let found = self.job.take().unwrap().result(db);
            let p = self.part;
            let (order, worse_from) = if self.want_lists {
                let list = App::alt_collect(&self.cur, p, found);
                let r = alt_order(db, &self.owned, &self.cur, p, list.items.iter().copied());
                self.lists.push(list);
                r
            } else {
                // Count only: no set copies (building the lists was the heaviest part of the alternatives)
                let items = found.sets.iter().enumerate().flat_map(|(i, s)| s.parts[p].iter().map(move |&id| (id, s.free, i as u32)));
                alt_order(db, &self.owned, &self.cur, p, items)
            };
            self.counts[p] = alt_count(&self.cur, p, &order, worse_from);
            self.part += 1;
        }
    }
}

/// Copy of the progress the draw uses while the search runs outside the lock
pub struct JobView {
    pub needed: bool,
    pub found: usize,
    pub started: u64,
    pub progress: f32,
}

/// Search options for alternatives
const ALT_OPTS: Opts = Opts { allow_needed: false, dominance: false, group: false, max: 2000, node_limit: 3_000_000, time_ms: 360, ..Opts::SEARCH };

/// Building a searcher (heavy: 30-40 ms per searcher on Recomp, two with split work) happens outside the shared lock.
/// Inside the lock the draw waited over 100 ms, skipped the frame and the window flickered (2026-11-18).
pub enum BuildReq {
    /// New search (conditions, options, earlier results to start from, memories to compare by charm signature)
    New(&'static crate::db::Db, Cond, Opts, Vec<Set>, Reuse),
    /// Next step after 'search with unowned charms: yes'
    Continue(Job),
}
/// Earlier search memories compared by charm signature: complete results for the same conditions and owned armor,
/// and 'nothing with owned charms' conditions. Used even if the owned charm list differs, when the signature is the same (every new charm is worse or equal).
pub struct Reuse {
    /// (charm signature, results, owned charm list at that time: to remap the results' charm indices to the current list)
    results: Vec<(skillsim_engine::search::CharmSignature, Found, Vec<skillsim_engine::types::Charm>)>,
    empty: Vec<(Cond, skillsim_engine::search::CharmSignature)>,
}

/// The smithy list's part of a search memory key: its hash with 'smithy only', 0 without (the list does not matter then)
fn smithy_sig(db: &Db, c: &Cond) -> u64 {
    if c.smithy { db.inv().smithy.as_ref().map_or(1, |s| s.sig) } else { 0 }
}
/// Built result: a search, or remembered results
pub enum Built {
    Job(Job),
    Cached(Found, Vec<skillsim_engine::types::Charm>),
}
impl BuildReq {
    pub fn run(self) -> Option<Built> {
        match self {
            BuildReq::New(db, c, o, seeds, reuse) => {
                let mut job = Job::new(db, &c, Opts { defer: true, ..o })?;
                // If a memory with the same charm signature exists, do not search again
                if let Some(sig) = job.charm_sig.clone() {
                    if let Some((_, f, charms)) = reuse.results.iter().find(|(s, _, _)| *s == sig) {
                        return Some(Built::Cached(f.clone(), charms.clone()));
                    }
                }
                let empty = o.owned_empty
                    || job.charm_sig.as_ref().is_some_and(|sig| reuse.empty.iter().any(|(m, s)| s == sig && at_least_as_strict(&c, m)));
                if !empty {
                    job.seed(db, &seeds);
                }
                job.begin(empty);
                Some(Built::Job(job))
            }
            BuildReq::Continue(mut job) => {
                job.continue_needed();
                Some(Built::Job(job))
            }
        }
    }
}
/// Search being built (for the loading window)
pub struct Building {
    pub generation: u32,
    pub needed: bool,
    pub started: u64,
    req: Option<BuildReq>,
}

/// Key of the decoration placement cache: screen, set index, representative armor per part, charm, decoration count
pub type PlacementKey = (u8, usize, [u16; PARTS], i32, usize);

/// A complete search result (with the conditions and owned equipment at that time)
struct CachedSearch {
    cond: Cond,
    charms: Vec<skillsim_engine::types::Charm>,
    owned: alloc::collections::BTreeSet<u16>,
    /// smithy_sig at that time (0 without 'smithy only')
    smithy: u64,
    /// Charm signature of the owned charms at that time
    sig: Option<skillsim_engine::search::CharmSignature>,
    found: Found,
}

/// Condition confirmed to have no set with owned charms (with the owned charms at that time)
struct OwnedEmpty {
    cond: Cond,
    charms: Vec<skillsim_engine::types::Charm>,
    /// Owned armor and smithy_sig at that time: newly owned (or newly listed) armor can make a set possible
    owned: alloc::collections::BTreeSet<u16>,
    smithy: u64,
    sig: Option<skillsim_engine::search::CharmSignature>,
}

/// Is new as strict as or stricter than old: if old had no set, new has none either.
/// When the target skills are the same or more/higher, usable armor and slots are the same or fewer, and conditions are the same or stricter.
fn at_least_as_strict(new: &Cond, old: &Cond) -> bool {
    old.targets.iter().all(|&(tree, need)| new.targets.iter().any(|&(t, n)| t == tree && n >= need))
        && new.kind == old.kind
        && new.sex == old.sex
        && new.weapon <= old.weapon
        && (0..3).all(|i| !new.ranks[i] || old.ranks[i])
        && (new.forbid || !old.forbid)
        && (!new.arena || old.arena)
        && (!new.event || old.event)
        && (!new.reward || old.reward)
        && (!new.deviant || old.deviant)
        && (new.smithy || !old.smithy)
        && new.min_def >= old.min_def
        && (0..RES).all(|e| match old.min_res[e] {
            None => true,
            Some(v) => new.min_res[e].is_some_and(|w| w >= v),
        })
        && (0..PARTS).all(|p| old.fixed[p] == 0 || new.fixed[p] == old.fixed[p])
        && old.excluded.iter().all(|x| new.excluded.contains(x))
        && old.excluded_jewels.iter().all(|x| new.excluded_jewels.contains(x))
        && new.fixed_charm.is_none()
        && old.fixed_charm.is_none()
        && new.charm_override.is_none()
        && old.charm_override.is_none()
}

/// Search measurements (for the log)
pub struct SearchStat {
    pub how: &'static str,
    pub nodes: u64,
    pub wall_ms: u64,
    pub busy_ms: u64,
    pub found: usize,
    pub needed: bool,
    pub charms: usize,
    pub targets: usize,
    /// Whether the owned-charm phase was skipped from memory, and how many sets were seeded from earlier results
    pub owned_skipped: bool,
    pub seeded: usize,
    /// For tests: (free slots, defense) of every result including grouped sets
    #[cfg_attr(target_arch = "arm", allow(dead_code))] // read by plugin-hosttest
    pub flat: Vec<(u8, u16)>,
    /// Owned-charm phase (ms, nodes), ms spent waiting at the question
    pub owned_ms: u64,
    pub owned_nodes: u64,
    pub ask_wait_ms: u64,
}
impl SearchStat {
    fn of(job: &Job, how: &'static str) -> Self {
        SearchStat {
            how,
            nodes: job.total_nodes(),
            wall_ms: crate::clock::ms_since(job.started),
            busy_ms: job.busy_ticks / crate::clock::PER_MS,
            found: job.found(),
            needed: job.needed,
            charms: job.charm_count,
            targets: job.skill_ids.len(),
            owned_skipped: job.owned_skipped,
            seeded: job.seeded,
            flat: if cfg!(target_arch = "arm") { Vec::new() } else { job.flat_ranks() },
            owned_ms: job.owned_ms,
            owned_nodes: job.owned_nodes,
            ask_wait_ms: job.ask_wait_ms,
        }
    }
}

/// Extra-skill search (runs in pieces per frame)
pub struct ExtraRun {
    cond: Cond,
    any_charm: bool,
    skills: Vec<u16>,
    next: usize,
    failed_tree: u16,
    found: Vec<u16>,
    job: Option<(u16, Job)>,
    /// For the log: start time, skills tried, tries that hit the time/node limit
    start: u64,
    tried: u32,
    limited: u32,
    /// Ticks spent building jobs / stepping them (where the time goes)
    build_ticks: u64,
    step_ticks: u64,
    nodes: u64,
    /// Skills dropped by the point bound without a search
    skipped: u32,
    /// Cache key of this run (extra_cache)
    key: String,
}

impl ExtraRun {
    /// Log line when the work ends
    pub fn summary(&self) -> alloc::string::String {
        alloc::format!(
            "extra skills: {} found, {} tried, {} skipped by the bound ({} hit the limit), any charm={}, {} ms (build {} ms, search {} ms, {} nodes)",
            self.found.len(),
            self.tried,
            self.skipped,
            self.limited,
            self.any_charm,
            crate::clock::ms_since(self.start),
            self.build_ticks / crate::clock::PER_MS,
            self.step_ticks / crate::clock::PER_MS,
            self.nodes
        )
    }
    /// Progress (skills checked, all skills)
    pub fn progress(&self) -> (u16, u16) {
        (self.next.min(self.skills.len()) as u16, self.skills.len() as u16)
    }
}

pub struct App {
    pub db: &'static Db,
    pub open: bool,
    pub language: usize,
    pub layout: usize,
    /// 'Enable logging' (Forge menu): our info lines, search arguments and frame stats go to the log. Off by default.
    pub logging: bool,
    /// Screen state number: raised by anything that can change what is drawn (buttons, search results, inventory, settings).
    /// If unchanged, the draw re-issues last frame's draw commands (ui::render).
    pub rev: u64,
    /// Clickable control under the mouse (tab, pager arrow, button prompt): drawn highlighted
    pub hover: Option<Target>,
    // Conditions
    pub targets: Vec<u16>,
    pub kind: u8,
    pub weapon: usize,
    pub ranks: [bool; 3],
    pub forbid: bool,
    pub priority: usize,
    pub arena: bool,
    pub event: bool,
    /// Use reward armor (no smithy recipe) even when not owned
    pub reward: bool,
    /// Use deviant armor even when not owned (from its start rank on)
    pub deviant: bool,
    /// Only armor and decorations the smithy lists right now
    pub smithy: bool,
    pub mindef: usize,
    pub res: [usize; RES],
    pub fixed: [u16; PARTS],
    pub fixed_charm: Option<usize>,
    pub excluded: Vec<u16>,
    /// Excluded decorations (from the results' exclude list, X on a part row)
    pub excluded_jewels: Vec<u16>,
    pub saved: Vec<Set>,
    pub recent: Vec<u16>,
    // Screen
    pub screen: Screen,
    pub col: usize,
    pub row: usize,
    pub drop: Option<(Opt, usize)>,
    pub sub: Option<(Opt, usize)>,
    pub subdrop: Option<(usize, usize)>,
    pub menu: Option<usize>,
    pub confirm: Option<Confirm>,
    pub fixmenu: Option<(usize, usize)>,
    /// Exclude list of the part row under the cursor is open (cursor)
    pub exmenu: Option<usize>,
    pub fix_from: Screen,
    pub box_slot: usize,
    pub bidx: usize,
    pub btop: usize,
    pub tab: usize,
    pub sidx: usize,
    pub stop: usize,
    pub results: Vec<Set>,
    pub best_n: usize,
    pub show_all: bool,
    pub ridx: usize,
    pub rtop: usize,
    pub focus: Focus,
    pub pidx: usize,
    pub dpage: usize,
    /// Alternatives popup: (armor id, free slots, index into the cached alternative sets of this part or None for the same group)
    pub popup: Option<Vec<(u16, u8, Option<u32>)>>,
    pub aidx: usize,
    alt_cache: Option<(usize, Vec<AltPart>)>,
    /// Alternatives being computed (part by part, a little at a time)
    alt_run: Option<AltRun>,
    /// Armor choices per part for every result set (the 'N alternatives' tag). Counted in advance from near the cursor.
    alt_all: Vec<Option<[u32; PARTS]>>,
    /// Raised whenever the result list changes (alternative work started before is discarded)
    alt_gen: u32,
    /// Counts of the finished parts of the set being computed (set index, finished parts, counts): tags appear part by part
    alt_partial: Option<(usize, usize, [u32; PARTS])>,
    /// Index in the alternatives list where the strictly worse entries start (drawn dimmed)
    pub alt_worse_from: core::cell::Cell<usize>,
    /// Part whose A was pressed before its list existed: open the popup as soon as the list is ready
    alt_popup_wait: Option<usize>,
    /// Decoration placement (for drawing): not recomputed while (set index, screen, armor/charm) stay the same
    pub placement_cache: core::cell::RefCell<Option<(PlacementKey, ([Vec<u16>; 7], [u8; 7]))>>,
    /// Measurements of the finished search (lib logs and clears them)
    pub last_search: Option<SearchStat>,
    /// Conditions confirmed to have no set with owned charms (this game session only, at most 8)
    owned_empty: Vec<OwnedEmpty>,
    /// Complete search results (no new search for exactly the same conditions and owned equipment; this game session only, at most 4)
    result_cache: Vec<CachedSearch>,
    /// The last search was shown from memory (lib logs it and clears the flag)
    pub cache_hit: bool,
    /// A search just finished and the results screen was shown (for the result sound; taken by the compute thread)
    pub search_done: bool,
    /// Current search conditions and whether the 'nothing with owned charms' memory was used
    search_cond: Option<(Cond, bool)>,
    /// The unowned-charm search was declined: skip the no-result cause analysis
    skip_diag: bool,
    /// Search being built outside the lock (the loading window shows meanwhile)
    pub building: Option<Building>,
    /// The compute thread runs the search outside the lock: the draw shows the loading window from this copy
    pub job_out: Option<JobView>,
    build_generation: u32,
    /// PC tests: build searchers right inside tick() (no threads)
    pub inline_build: bool,
    /// Value shown on the loading bar (smoothly follows the real progress) and its phase (true in the unowned-charm phase)
    pub load_shown: core::cell::Cell<(f32, bool)>,
    pub midx: usize,
    pub message: u16,
    pub message_slot: usize,
    pub dialog_sel: usize,
    pub notice: Option<Notice>,
    pub diag: Vec<(DiagLabel, DiagChange)>,
    pub diag_run: Option<DiagRun>,
    pub job: Option<Job>,
    pub needed_mode: bool,
    /// The current results were shown from search memory
    pub results_cached: bool,
    pub complete: bool,
    pub stat_nodes: u64,
    pub extra: Option<Vec<u16>>,
    pub extra_run: Option<ExtraRun>,
    /// Cause analysis / extra skills running on the compute thread outside the lock (copies and flags the draw uses meanwhile)
    pub diag_out: Option<(usize, usize)>,
    pub diag_cancel: bool,
    pub extra_out: bool,
    /// The next search continues with unowned charms without asking if owned charms find nothing: after picking a
    /// cause-analysis suggestion or an unowned-charm extra skill, and after fixing / releasing a part or excluding armor or
    /// a decoration on unowned-charm results (the user already chose unowned charms; a narrower condition skips the
    /// owned-charm phase from memory, and the question only repeated that choice)
    auto_needed: bool,
    /// Re-search from unowned-charm results (fix / release / exclude): unowned-charm results then come without the
    /// 'searched with unowned charms' notice (nothing changed for the user). Taken by run_search into search_quiet.
    quiet_needed: bool,
    search_quiet: bool,
    pub extra_any: bool,
    /// Extra-skill search progress (checked, all) for the progress bar
    pub extra_prog: (u16, u16),
    /// Cancel the extra-skill search when the compute thread puts it back
    extra_cancel: bool,
    /// Extra-skill list: category tab (index into extra_tabs())
    pub etab: usize,
    /// Log line of the last finished extra-skill search (ExtraRun::summary)
    pub extra_summary: Option<alloc::string::String>,
    /// Finished extra-skill searches of this game session (newest first): key -> result. Kept in memory only
    /// (not saved): the same set is often checked again within a session, and a new session starts clean.
    extra_cache: Vec<(String, ExtraEnd)>,
    pub eidx: usize,
    pub etop: usize,
    pub saved_from: Screen,
    pub results_pos: (usize, usize),
    pub compare: bool,
    pub dirty: bool,
}

#[derive(PartialEq)]
pub struct Snapshot {
    view: Vec<usize>,
    pub tab: usize,
    /// List pages (first row of results/saved sets, equipment box, skills, extra skills, and the detail page)
    pub page: [usize; 5],
    overlay: (Option<(Opt, usize)>, Option<(Opt, usize)>, Option<(usize, usize)>, Option<usize>, Option<(usize, usize)>, Option<usize>),
    confirm: Option<Confirm>,
    notice: Option<Notice>,
    pub message: u16,
    data: String,
}

/// List cursor movement: up/down wraps within the page, left/right moves a page keeping the row position.
pub fn move_cursor(index: usize, count: usize, rows: usize, b: Button) -> (usize, usize) {
    if count == 0 {
        return (0, 0);
    }
    let index = match b {
        Up | Down => {
            let start = index / rows * rows;
            let size = rows.min(count - start);
            start + (index - start + if b == Down { 1 } else { size - 1 }) % size
        }
        _ => {
            let pages = count.div_ceil(rows);
            let page = (index / rows + if b == Right { 1 } else { pages - 1 }) % pages;
            (count - 1).min(page * rows + index % rows)
        }
    };
    (index, index / rows * rows)
}

/// Something on screen the mouse can point at (recorded while drawing, ui.rs)
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Target {
    /// Condition screen cell (column, row)
    Main(usize, usize),
    Drop(usize),
    Sub(usize),
    SubDrop(usize),
    Menu(usize),
    Fix(usize),
    /// Row of the results' exclude list
    Ex(usize),
    Manage(usize),
    Box(usize),
    Skill(usize),
    /// Results / saved list row (index into rows())
    Row(usize),
    Extra(usize),
    Part(usize),
    Alt(usize),
    /// Button inside a dialog
    Dialog(usize),
    /// Skill picker category tab
    Tab(usize),
    /// Extra-skill list category tab
    ETab(usize),
    /// Pager arrow (true = next page) and the layer of the list it pages
    Page(bool, u8),
    /// A drawn button prompt (hint line, L/R, ZL/ZR): clicking it presses that button. Always clickable.
    Hint(Button),
}

impl Target {
    /// Drawing layer: only the topmost layer on screen takes the mouse (a popup over the list)
    pub fn layer(self) -> u8 {
        match self {
            Target::Page(_, l) => l,
            Target::Main(..) | Target::Manage(_) | Target::Box(_) | Target::Skill(_) | Target::Row(_) | Target::Part(_) | Target::Tab(_) | Target::Hint(_) => 0,
            Target::Drop(_) | Target::Sub(_) | Target::Menu(_) | Target::Fix(_) | Target::Ex(_) | Target::Extra(_) | Target::Alt(_) | Target::ETab(_) => 1,
            Target::SubDrop(_) => 2,
            Target::Dialog(_) => 3,
        }
    }
}

/// Rows per page of the alternatives popup
pub const ALT_ROWS: usize = 9;

fn step(i: usize, n: usize, b: Button) -> usize {
    if n == 0 {
        return 0;
    }
    if b == Down { (i + 1) % n } else { (i + n - 1) % n }
}

impl App {
    pub fn new(db: &'static Db) -> App {
        App {
            db,
            open: false,
            language: 0, // English until the user picks a language
            layout: 0,
            logging: false,
            rev: 0,
            hover: None,
            results_cached: false,
            targets: Vec::new(),
            kind: 1,
            weapon: 0,
            ranks: [true, true, true],
            forbid: true,
            priority: 0,
            arena: false,
            event: false,
            reward: false,
            deviant: false,
            smithy: false,
            mindef: 0,
            res: [0; RES],
            fixed: [0; PARTS],
            fixed_charm: None,
            excluded: Vec::new(),
            excluded_jewels: Vec::new(),
            saved: Vec::new(),
            recent: Vec::new(),
            screen: Screen::Main,
            col: 0,
            row: 0,
            drop: None,
            sub: None,
            subdrop: None,
            menu: None,
            confirm: None,
            fixmenu: None,
            exmenu: None,
            fix_from: Screen::Main,
            box_slot: 0,
            bidx: 0,
            btop: 0,
            tab: 1,
            sidx: 0,
            stop: 0,
            results: Vec::new(),
            best_n: 0,
            show_all: false,
            ridx: 0,
            rtop: 0,
            focus: Focus::List,
            pidx: 0,
            dpage: 0,
            popup: None,
            aidx: 0,
            alt_cache: None,
            alt_run: None,
            alt_all: Vec::new(),
            alt_gen: 0,
            alt_partial: None,
            alt_popup_wait: None,
            alt_worse_from: core::cell::Cell::new(usize::MAX),
            placement_cache: core::cell::RefCell::new(None),
            last_search: None,
            owned_empty: Vec::new(),
            result_cache: Vec::new(),
            cache_hit: false,
            search_done: false,
            search_cond: None,
            skip_diag: false,
            building: None,
            job_out: None,
            build_generation: 0,
            inline_build: false,
            load_shown: core::cell::Cell::new((0., false)),
            midx: 0,
            message: 0,
            message_slot: 0,
            dialog_sel: 0,
            notice: None,
            diag: Vec::new(),
            diag_run: None,
            job: None,
            needed_mode: false,
            complete: true,
            stat_nodes: 0,
            extra: None,
            extra_run: None,
            diag_out: None,
            diag_cancel: false,
            extra_out: false,
            auto_needed: false,
            quiet_needed: false,
            search_quiet: false,
            extra_any: false,
            extra_prog: (0, 0),
            extra_cancel: false,
            etab: 0,
            extra_summary: None,
            extra_cache: Vec::new(),
            eidx: 0,
            etop: 0,
            saved_from: Screen::Main,
            results_pos: (0, 0),
            compare: false,
            dirty: false,
        }
    }

    fn persist(&mut self) {
        self.dirty = true;
    }

    // ---- Common ----
    /// Rows: results (recommended + fold row + other sets) or saved sets. None is the fold row.
    pub fn rows(&self) -> Vec<Option<usize>> {
        if self.screen == Screen::Saved {
            return (0..self.saved.len()).map(Some).collect();
        }
        if self.best_n == 0 || self.results.len() == self.best_n {
            return (0..self.results.len()).map(Some).collect();
        }
        let mut out: Vec<Option<usize>> = (0..self.best_n).map(Some).collect();
        out.push(None);
        if self.show_all {
            out.extend((self.best_n..self.results.len()).map(Some));
        }
        out
    }

    pub fn store(&self) -> &Vec<Set> {
        if self.screen == Screen::Saved { &self.saved } else { &self.results }
    }

    pub fn current_index(&self) -> Option<usize> {
        self.rows().get(self.ridx).copied().flatten()
    }
    pub fn current(&self) -> Option<&Set> {
        self.current_index().and_then(|i| self.store().get(i))
    }

    pub fn cond(&self) -> Cond {
        self.cond_with(&self.targets, self.forbid, self.mindef, self.res, self.fixed, self.fixed_charm, &self.excluded)
    }

    #[allow(clippy::too_many_arguments)]
    fn cond_with(&self, targets: &[u16], forbid: bool, mindef: usize, res: [usize; RES], fixed: [u16; PARTS], fixed_charm: Option<usize>, excluded: &[u16]) -> Cond {
        let skills: Vec<&Skill> = targets.iter().filter_map(|&id| self.db.skill(id)).collect();
        Cond {
            targets: skills.iter().map(|s| (s.tree, s.threshold as i16)).collect(),
            skill_ids: skills.iter().map(|s| s.id).collect(),
            kind: self.kind,
            weapon: self.weapon as u8,
            ranks: self.ranks,
            forbid,
            priority: self.priority as u8,
            arena: self.arena,
            event: self.event,
            reward: self.reward,
            deviant: self.deviant,
            smithy: self.smithy,
            sex: if self.db.inv().sex == 2 { 2 } else { 1 },
            min_def: MIN_DEF[mindef],
            min_res: core::array::from_fn(|e| RES_STEPS[res[e]]),
            fixed,
            fixed_charm,
            excluded: excluded.to_vec(),
            excluded_jewels: self.excluded_jewels.clone(),
            charm_override: None,
        }
    }

    /// Alternatives list for part p [(armor id, free slots, set index or None)]. For search results, every armor that still meets
    /// the conditions when only that part changes (fixed parts excluded). Saved sets show only their saved group.
    /// Never waits in the game (None if not ready yet: the popup opens when the compute thread finishes it).
    pub fn part_alts(&mut self, p: usize) -> Option<Vec<(u16, u8, Option<u32>)>> {
        if self.inline_build {
            while self.alts_cached(p).is_none() && self.screen == Screen::Results && self.current().is_some() {
                self.ensure_alts(u64::MAX);
            }
        }
        self.alts_cached(p)
    }

    /// The result list changed: drop all alternative memories
    fn reset_alts(&mut self) {
        self.alt_cache = None;
        self.alt_run = None;
        self.alt_all.clear();
        self.alt_partial = None;
        self.alt_popup_wait = None;
        self.alt_gen = self.alt_gen.wrapping_add(1);
    }

    /// Next set to compute alternatives for: the set under the cursor (with lists) -> sets nearest the cursor (counts only)
    fn next_alt_target(&self) -> Option<(usize, bool)> {
        let cur = self.current_index();
        if let Some(k) = cur {
            if self.alt_cache.as_ref().is_none_or(|(c, _)| *c != k) {
                return Some((k, true));
            }
        }
        let rows = self.rows();
        let n = rows.len();
        for d in 1..n {
            for i in [self.ridx.checked_add(d), self.ridx.checked_sub(d)].into_iter().flatten() {
                if let Some(Some(k)) = rows.get(i) {
                    if self.alt_all.get(*k).is_some_and(|c| c.is_none()) {
                        return Some((*k, false));
                    }
                }
            }
        }
        None
    }

    /// Prepare alternatives a little at a time (compute thread, inside the lock: light work only). So that 'N alternatives' shows
    /// just by resting the cursor on a result, counts are taken in advance from the sets near the cursor. Heavy work runs outside the lock (take_alt_build, take_alt_work).
    pub fn ensure_alts(&mut self, ms: u64) {
        let before = self.alt_view();
        self.ensure_alts_inner(ms);
        if self.alt_view() != before {
            self.touch();
        }
    }

    fn ensure_alts_inner(&mut self, ms: u64) {
        if self.screen != Screen::Results {
            return;
        }
        if self.alt_all.len() != self.results.len() {
            self.alt_all = alloc::vec![None; self.results.len()];
        }
        // Popup waiting for its list
        if let Some(p) = self.alt_popup_wait {
            if self.focus != Focus::Parts || self.pidx != p || self.popup.is_some() {
                self.alt_popup_wait = None;
            } else if let Some(alts) = self.alts_cached(p) {
                self.alt_popup_wait = None;
                self.open_alt_popup(alts);
            }
        }
        if self.alt_run.as_ref().is_some_and(|r| r.generation != self.alt_gen) {
            self.alt_run = None;
        }
        // A set the cursor left only finishes its counts (popup lists are dropped since copying sets is heavy)
        let cur_key = self.current_index();
        if let Some(r) = self.alt_run.as_mut() {
            if r.want_lists && Some(r.key) != cur_key {
                r.want_lists = false;
                r.lists.clear();
            }
        }
        if self.alt_run.is_none() {
            let Some((key, want_lists)) = self.next_alt_target() else { return };
            let Some(cur) = self.results.get(key).cloned() else { return };
            self.alt_run = Some(AltRun {
                key,
                generation: self.alt_gen,
                cur,
                part: 0,
                job: None,
                want_lists,
                lists: Vec::new(),
                counts: [0; PARTS],
                owned: self.db.inv().owned.clone(),
                pending: None,
                waiting: false,
            });
        }
        let start = crate::clock::ticks();
        let mut run = self.alt_run.take().unwrap();
        loop {
            self.alt_partial = Some((run.key, run.part, run.counts));
            if run.part >= PARTS {
                if let Some(c) = self.alt_all.get_mut(run.key) {
                    *c = Some(run.counts);
                }
                if run.want_lists {
                    self.alt_cache = Some((run.key, run.lists));
                }
                self.alt_partial = None;
                return;
            }
            if self.fixed[run.part] != 0 {
                run.counts[run.part] = group_count(&run.cur, run.part);
                run.lists.push(AltPart::default());
                run.part += 1;
                continue;
            }
            if run.job.is_none() {
                if run.waiting {
                    break; // The compute thread is building the searcher outside the lock
                }
                let c = self.alt_cond(&run.cur, run.part);
                if self.inline_build {
                    match Job::new(self.db, &c, ALT_OPTS) {
                        Some(job) => run.job = Some(job),
                        None => {
                            run.counts[run.part] = group_count(&run.cur, run.part);
                            run.lists.push(AltPart::default());
                            run.part += 1;
                            continue;
                        }
                    }
                } else {
                    // Build the searcher outside the lock
                    run.pending = Some(c);
                    run.waiting = true;
                    break;
                }
            }
            if !self.inline_build {
                break; // Computing and converting results happen on the compute thread outside the lock (take_alt_work -> AltRun::work -> put_alt_work)
            }
            let left = ms.saturating_sub(crate::clock::ms_since(start)).max(1);
            run.work(self.db, left);
            if crate::clock::ms_since(start) >= ms || crate::job::should_yield() {
                break;
            }
        }
        self.alt_run = Some(run);
    }

    /// The set the alternatives popup cursor would switch to (for the detail panel): the alternative set with that armor
    /// first, or the current set with that group member first. None without a popup.
    pub fn popup_preview(&self) -> Option<Set> {
        let popup = self.popup.as_ref()?;
        let &(aid, _, res) = popup.get(self.aidx)?;
        let p = self.pidx;
        let mut set = match res {
            Some(idx) => self.alt_cache.as_ref()?.1.get(p)?.sets.get(idx as usize)?.clone(),
            None => self.current()?.clone(),
        };
        if let Some(at) = set.parts.get(p)?.iter().position(|&x| x == aid) {
            let v = set.parts[p].remove(at);
            set.parts[p].insert(0, v);
        }
        Some(set)
    }

    /// The mouse points at (click = false) or clicks (true) a target: move the cursor there. Returns the button a click
    /// presses (A), or None. Hovering parts of the right panel only moves within the parts focus.
    pub fn mouse_target(&mut self, t: Target, click: bool) -> Option<Button> {
        match t {
            Target::Main(c, r) => {
                self.col = c;
                self.row = r;
            }
            Target::Drop(i) => {
                if let Some((o, _)) = self.drop {
                    self.drop = Some((o, i));
                }
            }
            Target::Sub(i) => {
                if let Some((o, _)) = self.sub {
                    self.sub = Some((o, i));
                }
            }
            Target::SubDrop(i) => {
                if let Some((e, _)) = self.subdrop {
                    self.subdrop = Some((e, i));
                }
            }
            Target::Menu(i) => {
                if self.menu.is_some() {
                    self.menu = Some(i);
                }
            }
            Target::Fix(i) => {
                if let Some((s, _)) = self.fixmenu {
                    self.fixmenu = Some((s, i));
                }
            }
            Target::Ex(i) => {
                if self.exmenu.is_some() {
                    self.exmenu = Some(i);
                }
            }
            Target::Manage(r) => self.midx = r,
            Target::Box(r) => self.bidx = r,
            Target::Skill(r) => self.sidx = r,
            Target::Row(r) => self.ridx = r,
            Target::Extra(r) => self.eidx = r,
            Target::Part(p) => {
                if self.focus != Focus::Parts {
                    if !click {
                        return None; // hovering the parts while in the list does not leave the list
                    }
                    self.focus = Focus::Parts;
                    self.pidx = p;
                    return None; // the first click selects the part; a second click opens it
                }
                if click && self.pidx != p {
                    self.pidx = p;
                    return None;
                }
                self.pidx = p;
            }
            Target::Alt(r) => self.aidx = r,
            Target::Dialog(i) => self.dialog_sel = i,
            // Tabs, pager arrows and button prompts act on click only (hovering does nothing)
            Target::Tab(i) => {
                if click && i < self.tab_count() && i != self.tab {
                    self.tab = i;
                    self.sidx = 0;
                    self.stop = 0;
                }
                return None;
            }
            Target::ETab(i) => {
                if click && i != self.etab && i < self.extra_tabs().len() {
                    self.etab = i;
                    self.eidx = 0;
                    self.etop = 0;
                }
                return None;
            }
            Target::Page(right, layer) => {
                if !click {
                    return None;
                }
                // The result list pager pages the list even while the parts on the right have the focus
                if layer == 0 && matches!(self.screen, Screen::Results | Screen::Saved) && self.popup.is_none() {
                    self.focus = Focus::List;
                }
                return Some(if right { Button::Right } else { Button::Left });
            }
            Target::Hint(b) => return click.then_some(b),
        }
        click.then_some(Button::A)
    }

    fn open_alt_popup(&mut self, alts: Vec<(u16, u8, Option<u32>)>) {
        let Some(cur) = self.current() else { return };
        if alts.len() > 1 {
            self.aidx = alts.iter().position(|x| x.0 == cur.head(self.pidx)).unwrap_or(0);
            self.popup = Some(alts);
        }
    }

    /// Uses only what was already computed (used by the draw). None if not computed yet.
    pub fn alts_cached(&self, p: usize) -> Option<Vec<(u16, u8, Option<u32>)>> {
        let cur = self.current()?;
        if self.screen != Screen::Results || self.fixed[p] != 0 {
            // Saved sets / fixed parts: only the set's own group
            self.alt_worse_from.set(usize::MAX);
            return Some(cur.parts[p].iter().map(|&id| (id, cur.free, None)).collect());
        }
        let key = self.current_index().unwrap_or(usize::MAX);
        let (k, lists) = self.alt_cache.as_ref()?;
        if *k != key {
            return None;
        }
        let (order, from) = alt_order(self.db, &self.db.inv().owned, cur, p, lists[p].items.iter().copied());
        self.alt_worse_from.set(from);
        Some(order)
    }

    /// Length of the alternatives list. Called every frame by the draw, so it uses the precomputed counts.
    /// 0 (no tag) if not counted yet. For the set being computed, finished parts show first.
    pub fn alts_len(&self, p: usize) -> usize {
        let Some(cur) = self.current() else { return 0 };
        if self.screen != Screen::Results || self.fixed[p] != 0 {
            return cur.parts[p].len();
        }
        (self.alt_packed(p) & 0xffff) as usize
    }

    /// Worthwhile alternatives of part p (not strictly worse, excluding the current armor): the 'N alternatives' tag
    pub fn alts_useful(&self, p: usize) -> usize {
        let Some(cur) = self.current() else { return 0 };
        if self.screen != Screen::Results || self.fixed[p] != 0 {
            return cur.parts[p].len().saturating_sub(1);
        }
        (self.alt_packed(p) >> 16) as usize
    }

    fn alt_packed(&self, p: usize) -> u32 {
        let key = self.current_index().unwrap_or(usize::MAX);
        if let Some(Some(c)) = self.alt_all.get(key) {
            return c[p];
        }
        match self.alt_partial {
            Some((k, done, c)) if k == key && p < done => c[p],
            _ => 0,
        }
    }

    /// Compute thread: take the pending alternative search, if any (AltRun::work outside the lock, returned by put_alt_work)
    pub fn take_alt_work(&mut self) -> Option<AltRun> {
        if self.alt_run.as_ref().is_some_and(|r| r.job.is_some()) { self.alt_run.take() } else { None }
    }
    /// Drop it if the result list changed meanwhile (keep counting even if the cursor moved, since the counts are prefetched)
    pub fn put_alt_work(&mut self, run: AltRun) {
        let before = self.alt_view();
        self.put_alt_work_inner(run);
        if self.alt_view() != before {
            self.touch();
        }
    }
    fn put_alt_work_inner(&mut self, run: AltRun) {
        if self.alt_run.is_none() && self.screen == Screen::Results && run.generation == self.alt_gen {
            self.alt_partial = Some((run.key, run.part, run.counts));
            self.alt_run = Some(run);
        }
    }

    /// Compute thread: take the alternative search to build outside the lock (set index, part, condition)
    pub fn take_alt_build(&mut self) -> Option<(usize, usize, Cond)> {
        let run = self.alt_run.as_mut()?;
        run.pending.take().map(|c| (run.key, run.part, c))
    }
    pub fn alt_build(db: &'static crate::db::Db, c: &Cond) -> Option<Job> {
        Job::new(db, c, ALT_OPTS)
    }
    /// Put back the built alternative search (dropped if the result list changed meanwhile)
    pub fn put_alt_job(&mut self, key: usize, part: usize, job: Option<Job>) {
        let Some(run) = self.alt_run.as_mut() else { return };
        if run.key != key || run.part != part || !run.waiting {
            return;
        }
        run.waiting = false;
        match job {
            Some(j) => run.job = Some(j),
            None => {
                run.counts[part] = group_count(&run.cur, part);
                run.lists.push(AltPart::default());
                run.part += 1;
            }
        }
    }

    pub(crate) fn alt_cond(&self, cur: &Set, p: usize) -> Cond {
        let mut c = self.cond();
        c.fixed = core::array::from_fn(|q| cur.head(q));
        c.fixed[p] = 0;
        c.weapon = cur.weapon;
        c.charm_override = cur.charm_data(self.db);
        c.fixed_charm = None;
        if c.charm_override.is_none() {
            // Set without a charm: fix an empty charm
            c.charm_override = Some(Charm { skills: [Pt::default(); 2], slots: 0 });
        }
        // Runs in pieces on the compute thread. The limit (ALT_OPTS 360 ms) is wall-clock; on hitting it the list is partial.
        c
    }

    pub(crate) fn alt_collect(cur: &Set, p: usize, found: Found) -> AltPart {
        let mut out = AltPart::default();
        for mut set in found.sets {
            if set.saved_charm.is_some() && cur.saved_charm.is_none() {
                set.saved_charm = None;
                set.charm = cur.charm;
            }
            set.needed = cur.needed;
            set.chance = cur.chance;
            set.charm_rare = cur.charm_rare;
            set.targets = cur.targets.clone();
            let at = out.sets.len() as u32;
            for &id in &set.parts[p] {
                out.items.push((id, set.free, at));
            }
            out.sets.push(set);
        }
        out
    }

    pub fn dialog(&self) -> Option<(Dialog, Vec<u16>)> {
        if self.diag_run.is_some() || self.diag_out.is_some() {
            return Some((Dialog::DiagWait, alloc::vec![k::H_SKIP]));
        }
        if self.building.is_some() || self.job_out.is_some() {
            return Some((Dialog::Loading, alloc::vec![k::H_STOP_SHOW, k::H_CANCEL]));
        }
        if let Some(job) = &self.job {
            if job.ask_ready() {
                return Some((Dialog::AskNeeded, alloc::vec![k::H_YES, k::H_NO]));
            }
            if job.state == JobState::Paused {
                return Some((Dialog::Loading, alloc::vec![k::H_CONTINUE, k::H_SHOW_NOW]));
            }
            // No limit: the user can finish with what was found or cancel at any time
            return Some((Dialog::Loading, alloc::vec![k::H_STOP_SHOW, k::H_CANCEL]));
        }
        if self.message != 0 {
            return Some((Dialog::Message, alloc::vec![k::H_CONFIRM]));
        }
        if self.confirm.is_some() {
            return Some((Dialog::Confirm, alloc::vec![k::H_YES, k::H_NO]));
        }
        if self.notice.is_some() && matches!(self.screen, Screen::Results | Screen::Saved) {
            if !self.diag.is_empty() && matches!(self.notice, Some(Notice::NoResult | Notice::NoResultFixed)) {
                let mut b = alloc::vec![0u16; self.diag.len()];
                b.push(k::H_CLOSE);
                return Some((Dialog::Diag, b));
            }
            return Some((Dialog::Notice, alloc::vec![k::H_CONFIRM]));
        }
        None
    }

    fn ask(&mut self, action: Confirm) {
        self.confirm = Some(action);
        self.dialog_sel = if action.yes_default() { 0 } else { 1 };
    }

    pub fn is_saved(&self, r: &Set) -> bool {
        self.saved.iter().any(|s| r.same_gear(s, self.db))
    }

    fn close_dialog(&mut self, kind: Dialog, yes: bool, pick: usize) {
        self.dialog_sel = 0;
        match kind {
            Dialog::Message => self.message = 0,
            Dialog::Diag => {
                let chosen = if yes { self.diag.get(pick).cloned() } else { None };
                self.notice = None;
                self.diag.clear();
                let Some((_, ch)) = chosen else {
                    self.screen = Screen::Main;
                    return;
                };
                self.apply_change(&ch);
                // Cause-analysis suggestions were verified including unowned charms: search with owned charms first (use those results if any),
                // otherwise continue with unowned charms without asking again
                self.auto_needed = true;
                self.run_search();
            }
            Dialog::Confirm => {
                let action = self.confirm.take();
                if yes {
                    if let Some(a) = action {
                        self.do_confirmed(a);
                    }
                }
            }
            Dialog::AskNeeded => {
                if yes {
                    if let Some(job) = self.job.take() {
                        self.start_build(BuildReq::Continue(job), true);
                    }
                } else if let Some(mut job) = self.job.take() {
                    // Do not search unowned charms: the owned phase found nothing, so there is nothing to show.
                    // Drop the search and stay on the current screen (no empty progress window, no cause analysis).
                    job.finish();
                    self.last_search = Some(SearchStat::of(&job, "declined"));
                }
            }
            _ => {
                if self.notice != Some(Notice::Needed) {
                    self.screen = Screen::Main;
                }
                self.notice = None;
            }
        }
    }

    fn apply_change(&mut self, ch: &DiagChange) {
        if let Some(v) = ch.forbid {
            self.forbid = v;
        }
        if let Some(v) = ch.mindef {
            self.mindef = v;
        }
        if let Some(v) = ch.res {
            self.res = v;
        }
        if let Some((f, c)) = ch.fixed {
            self.fixed = f;
            self.fixed_charm = c;
        }
        if let Some((armor, jewels)) = &ch.excluded {
            self.excluded = armor.clone();
            self.excluded_jewels = jewels.clone();
        }
        if let Some(v) = &ch.targets {
            self.targets = v.clone();
        }
    }

    fn do_confirmed(&mut self, action: Confirm) {
        match action {
            Confirm::ResetTargets => {
                self.targets.clear();
                if self.col == 0 {
                    self.row = 0;
                }
            }
            Confirm::ResetFixed => {
                self.fixed = [0; PARTS];
                self.fixed_charm = None;
            }
            Confirm::Release(id) => {
                self.excluded.retain(|&x| x != id);
                self.midx = self.midx.min((self.excluded.len() + self.excluded_jewels.len()).saturating_sub(1));
            }
            Confirm::ReleaseJewel(id) => {
                self.excluded_jewels.retain(|&x| x != id);
                self.midx = self.midx.min((self.excluded.len() + self.excluded_jewels.len()).saturating_sub(1));
            }
            Confirm::ReleaseAll => {
                self.excluded.clear();
                self.excluded_jewels.clear();
                self.midx = 0;
            }
            Confirm::Save => {
                if let Some(cur) = self.current().cloned() {
                    let mut s = cur;
                    s.saved_charm = s.charm_data(self.db);
                    s.charm = -1;
                    s.tags = 0;
                    self.saved.push(s);
                }
            }
            Confirm::Exclude(id) => {
                self.excluded.push(id);
                self.persist();
                self.research_from_results();
                self.run_search();
                return;
            }
            Confirm::ExcludeJewel(id) => {
                if !self.excluded_jewels.contains(&id) {
                    self.excluded_jewels.push(id);
                }
                self.persist();
                self.research_from_results();
                self.run_search();
                return;
            }
            Confirm::DeleteSaved(i) => {
                if i < self.saved.len() {
                    self.saved.remove(i);
                }
                self.ridx = self.ridx.min(self.saved.len().saturating_sub(1));
                self.rtop = self.ridx / SAVED_ROWS * SAVED_ROWS;
            }
            Confirm::ExtraAny => {
                self.start_extra(true);
                return;
            }
            Confirm::LeaveResults => self.screen = Screen::Main,
        }
        self.persist();
    }

    /// The screen must be redrawn
    pub fn touch(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// Parts of the screen changed by alternative computation: tags, popup, list markers
    fn alt_view(&self) -> ([u32; PARTS], bool, Option<usize>, usize) {
        (core::array::from_fn(|p| self.alt_packed(p)), self.popup.is_some(), self.alt_popup_wait, self.alt_all.iter().filter(|c| c.is_some()).count())
    }

    /// Does result set i have other armor to swap in (counted sets only; false until counted)
    pub fn row_has_alts(&self, i: usize) -> bool {
        self.alt_all.get(i).copied().flatten().is_some_and(|c| c.iter().any(|&n| n >> 16 > 0))
    }

    pub fn press(&mut self, b: Button) {
        self.touch();
        if let Some((kind, buttons)) = self.dialog() {
            let n = buttons.len();
            self.dialog_sel = self.dialog_sel.min(n - 1);
            match kind {
                Dialog::DiagWait => {
                    if matches!(b, A | B) {
                        if let Some(d) = self.diag_run.as_mut() {
                            d.cancel = true;
                        } else {
                            self.diag_cancel = true; // Running outside the lock: cancel when it comes back
                        }
                    }
                }
                Dialog::Loading if self.job.is_none() => {
                    // Building the searcher: both 'stop' and 'cancel' cancel (nothing found yet)
                    if matches!(b, Left | Right) {
                        self.dialog_sel = 1 - self.dialog_sel;
                    } else if matches!(b, A | B) {
                        self.building = None;
                    }
                }
                Dialog::Loading => {
                    let job = self.job.as_mut().unwrap();
                    if job.state == JobState::AskNeeded {
                        // Owned-charm phase done, just before the question (bar at 100%): ignore buttons
                    } else if job.state != JobState::Paused {
                        if matches!(b, Left | Right) {
                            self.dialog_sel = 1 - self.dialog_sel;
                        } else if b == A && self.dialog_sel == 0 {
                            job.finish(); // tick() takes the result and shows the results screen
                        } else if matches!(b, A | B) {
                            self.last_search = Some(SearchStat::of(job, "cancel"));
                            self.job = None;
                        }
                    } else if matches!(b, Left | Right) {
                        self.dialog_sel = 1 - self.dialog_sel;
                    } else if b == A && self.dialog_sel == 0 {
                        job.resume();
                    } else if matches!(b, A | B) {
                        job.finish();
                    }
                }
                Dialog::Diag => match b {
                    Up | Down => self.dialog_sel = step(self.dialog_sel, n, b),
                    A => {
                        let pick = self.dialog_sel;
                        self.close_dialog(kind, pick < self.diag.len(), pick);
                    }
                    B => self.close_dialog(kind, false, 0),
                    _ => {}
                },
                _ => match b {
                    Left | Right if n > 1 => self.dialog_sel = 1 - self.dialog_sel,
                    A => {
                        let yes = buttons[self.dialog_sel] == k::H_YES;
                        self.close_dialog(kind, yes, 0);
                    }
                    B => self.close_dialog(kind, false, 0),
                    _ => {}
                },
            }
            return;
        }
        if self.exmenu.is_some() {
            self.on_exmenu(b);
            return;
        }
        if self.fixmenu.is_some() {
            self.on_fixmenu(b);
            return;
        }
        match self.screen {
            Screen::Main => self.on_main(b),
            Screen::Skills => self.on_skills(b),
            Screen::Results | Screen::Saved => self.on_results(b),
            Screen::Manage => self.on_manage(b),
            Screen::Box => self.on_box(b),
        }
    }

    /// Apply the inventory read from the game. If the charm list changed, drop results that use its indices.
    pub fn apply_inventory(&mut self, inv: crate::db::Inv) {
        self.touch();
        let old = self.db.inv();
        if *old == inv {
            return;
        }
        let charms_changed = old.charms != inv.charms;
        self.db.set_inv(inv);
        let n = self.db.inv().charms.len();
        if self.fixed_charm.is_some_and(|i| i >= n) {
            self.fixed_charm = None;
            self.dirty = true;
        }
        self.reset_alts();
        if charms_changed && !self.results.is_empty() {
            self.results.clear();
            self.best_n = 0;
            if self.screen == Screen::Results {
                self.screen = Screen::Main;
                self.focus = Focus::List;
                self.popup = None;
                self.extra = None;
            }
        }
    }

    /// Map remembered charm indices (from the charm list at that time) to the current list. If the same charm is gone,
    /// store the charm data in the set itself (saved_charm). Getting or selling charms can change the list order.
    fn remap_charms(&self, found: &mut Found, old: &[skillsim_engine::types::Charm]) {
        let now = &self.db.inv().charms;
        for s in found.sets.iter_mut().chain(found.best.iter_mut()) {
            let c = match (usize::try_from(s.charm).ok().and_then(|i| old.get(i)), s.saved_charm) {
                (Some(&c), _) => c,
                (None, Some(c)) if !s.needed => c,
                _ => continue,
            };
            match now.iter().position(|x| *x == c) {
                Some(i) => {
                    s.charm = i as i32;
                    s.saved_charm = None;
                }
                None => {
                    s.charm = -1;
                    s.saved_charm = Some(c);
                }
            }
        }
    }

    /// Forget the search memory (another character: other owned equipment). The memory lives only in this game session:
    /// it is not saved, so an updated engine never shows results of an older one.
    pub fn clear_cache(&mut self) {
        self.result_cache.clear();
        self.owned_empty.clear();
    }

    /// For sound cues: compare before and after a button to see what changed (screen, cursors, overlays, saved conditions).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            view: alloc::vec![
                self.screen as usize,
                self.focus as usize,
                self.col,
                self.row,
                self.box_slot,
                self.bidx,
                self.btop,
                self.sidx,
                self.stop,
                self.ridx,
                self.rtop,
                self.pidx,
                self.dpage,
                self.aidx,
                self.midx,
                self.eidx,
                self.etop,
                self.dialog_sel,
                self.compare as usize,
                self.show_all as usize,
                self.popup.is_some() as usize,
                self.extra.is_some() as usize,
                (self.diag_run.is_some() || self.diag_out.is_some()) as usize,
                self.extra_busy() as usize,
                self.job.as_ref().map_or(9, |j| j.state as usize),
                self.dialog().map_or(99, |d| d.0 as usize),
            ],
            // Skill picker tab and extra-skill tab together (either change plays the tab sound)
            tab: self.tab * 64 + self.etab,
            page: [self.rtop, self.btop, self.stop, self.etop, self.dpage],
            overlay: (self.drop, self.sub, self.subdrop, self.menu, self.fixmenu, self.exmenu),
            confirm: self.confirm,
            notice: self.notice,
            message: self.message,
            data: self.encode(),
        }
    }

    pub fn is_fixed(&self, slot: usize) -> bool {
        if slot == CHARM_ROW { self.fixed_charm.is_some() } else { self.fixed[slot] != 0 }
    }

    /// Controls available now: (button, string key). Order: others -> X, Y -> A, B.
    pub fn hints(&mut self) -> Vec<(Button, u16)> {
        if self.dialog().is_some() {
            return Vec::new();
        }
        let mut out = self.own_hints();
        let order = |b: Button| match b {
            X => 1,
            Y => 2,
            A => 3,
            B => 4,
            _ => 0,
        };
        out.sort_by_key(|h| order(h.0));
        out
    }

    fn own_hints(&mut self) -> Vec<(Button, u16)> {
        if self.fixmenu.is_some() || self.exmenu.is_some() {
            return alloc::vec![(A, k::H_OK), (B, k::H_CLOSE)];
        }
        match self.screen {
            Screen::Main => {
                if self.subdrop.is_some() || self.drop.is_some() || self.menu.is_some() {
                    return alloc::vec![(A, k::H_OK), (B, k::H_CLOSE)];
                }
                if let Some((opt, _)) = self.sub {
                    return alloc::vec![(A, if opt == Opt::Armor { k::H_TOGGLE } else { k::H_CHANGE }), (B, k::H_CLOSE)];
                }
                let mut out = Vec::new();
                if self.col == 0 {
                    let on_skill = self.row < self.targets.len();
                    if on_skill {
                        out.push((A, k::H_CHANGE_SKILL));
                        out.push((X, k::H_REMOVE));
                    } else if self.targets.len() < MAX_TARGETS {
                        out.push((A, k::H_ADD_SKILL));
                    }
                    if !self.targets.is_empty() {
                        out.push((Y, k::H_REMOVE_ALL));
                    }
                } else if self.col == 1 {
                    let on_search = OPTIONS[self.row] == Opt::Search;
                    out.push((A, if on_search { k::H_OK } else { k::H_CHANGE }));
                    out.push((Y, k::H_EXCLUDED));
                } else {
                    out.push((A, k::H_FIXSET));
                    if self.is_fixed(self.row) {
                        out.push((X, k::H_RELEASE));
                    }
                    if (0..FIX_ROWS).any(|r| self.is_fixed(r)) {
                        out.push((Y, k::H_RELEASE_ALL));
                    }
                }
                out.push((Minus, k::H_SAVED_VIEW));
                out.push((Plus, k::H_SEARCH));
                out
            }
            Screen::Skills => alloc::vec![(A, k::H_TOGGLE), (B, k::H_RETURN)],
            Screen::Manage => {
                let mut out = Vec::new();
                if !self.manage_items().is_empty() {
                    out.push((X, k::H_RELEASE));
                    out.push((Y, k::H_RELEASE_ALL));
                }
                out.push((B, k::H_BACK));
                out
            }
            Screen::Box => {
                let mut out = Vec::new();
                if !self.box_items().is_empty() {
                    out.push((A, k::H_OK));
                }
                out.push((B, k::H_BACK));
                out
            }
            Screen::Results | Screen::Saved => {
                if let Some(extra) = &self.extra {
                    let mut out = Vec::new();
                    if !extra.is_empty() {
                        out.push((A, k::H_ADDSKILL));
                    }
                    out.push((B, k::H_CLOSE));
                    return out;
                }
                if self.extra_busy() {
                    return alloc::vec![(B, k::H_CANCEL)];
                }
                if self.popup.is_some() {
                    return alloc::vec![(ZR, k::H_PAGE), (A, k::H_USE), (B, k::H_CLOSE)];
                }
                if self.focus == Focus::List {
                    let rows = self.rows();
                    if self.screen == Screen::Results && rows.get(self.ridx) == Some(&None) {
                        return alloc::vec![(A, if self.show_all { k::H_FOLD } else { k::H_UNFOLD }), (B, k::H_BACK)];
                    }
                    let Some(cur) = self.current().cloned() else { return alloc::vec![(B, k::H_BACK)] };
                    if self.screen == Screen::Saved {
                        return alloc::vec![(X, k::H_DELETE), (A, k::H_OK), (B, k::H_BACK)];
                    }
                    let mut out = alloc::vec![
                        (Plus, if self.compare { k::H_COMPARE_OFF } else { k::H_COMPARE }),
                        (Minus, k::H_SAVED_VIEW),
                        (X, k::H_EXTRA),
                        (Y, if self.is_saved(&cur) { k::H_SAVE_DONE } else { k::H_SAVE }),
                    ];
                    if !self.compare {
                        out.push((A, k::H_OK)); // No 'decide' on the compare screen
                    }
                    out.push((B, k::H_BACK));
                    return out;
                }
                let mut out = Vec::new();
                let cur = self.current().cloned();
                if self.pidx < PARTS && self.alts_len(self.pidx) > 1 {
                    out.push((A, k::H_ALTS));
                }
                if self.screen == Screen::Results {
                    if !self.exclude_items().is_empty() {
                        out.push((X, k::H_EXCLUDE));
                    }
                    if self.pidx < PARTS || (self.pidx == CHARM_ROW && cur.is_some_and(|c| c.charm >= 0)) {
                        out.push((Y, if self.is_fixed(self.pidx) { k::H_FIXCHANGE } else { k::H_FIX }));
                    }
                }
                out.push((B, k::H_BACK));
                out
            }
        }
    }

    // ---- Search ----
    /// A re-search from the results screen (fix / release / exclude). On unowned-charm results the user already chose
    /// unowned charms: owned charms first, then unowned without asking again (auto_needed), and no notice (quiet_needed)
    fn research_from_results(&mut self) {
        self.auto_needed = self.needed_mode;
        self.quiet_needed = self.needed_mode;
    }

    pub fn run_search(&mut self) {
        let ask_needed = !core::mem::take(&mut self.auto_needed);
        self.search_quiet = core::mem::take(&mut self.quiet_needed);
        if self.targets.is_empty() {
            self.message = k::MSG_NO_TARGET;
            return;
        }
        if !self.ranks.iter().any(|&r| r) {
            self.message = k::MSG_NO_RANK;
            return;
        }
        if self.smithy && self.db.inv().smithy.is_none() {
            self.message = k::MSG_SMITHY_UNREAD;
            return;
        }
        for &id in self.targets.iter().rev() {
            self.recent.retain(|&x| x != id);
            self.recent.insert(0, id);
        }
        self.recent.truncate(RECENT_MAX);
        let c = self.cond();
        // Set search: split with helper threads; if owned charms find nothing, ask whether to search with unowned charms.
        // The compute thread builds the searcher outside the lock (take_build / finish_build). The loading window shows meanwhile.
        self.job = None;
        // Reuse 0: if complete results exist for exactly the same conditions and owned equipment, show them at once
        // (including results that went on to unowned charms)
        let inv = self.db.inv();
        let sig = smithy_sig(self.db, &c);
        if let Some(e) = self.result_cache.iter().find(|e| e.cond == c && e.charms == inv.charms && e.owned == inv.owned && e.smithy == sig) {
            let mut found = e.found.clone();
            let old = e.charms.clone();
            self.remap_charms(&mut found, &old);
            self.search_cond = Some((c, false));
            self.cache_hit = true;
            self.skip_diag = false;
            self.dialog_sel = 0;
            self.finish_search(found);
            return;
        }
        // Reuse 1: if as strict as or stricter than a remembered 'nothing with owned charms' condition, skip the owned-charm phase
        let inv = self.db.inv();
        let known_empty =
            self.owned_empty.iter().any(|m| m.charms == inv.charms && m.owned == inv.owned && m.smithy == sig && at_least_as_strict(&c, &m.cond));
        // Reuse 2: re-check earlier (owned-charm) results under the current conditions and start with them in the results
        let seeds = if self.needed_mode { Vec::new() } else { self.results.iter().take(200).cloned().collect() };
        // Reuse 3: even if the owned charm list differs, use 0 and 1 when the charm signature matches (compared outside the lock after building)
        let reuse = Reuse {
            results: self
                .result_cache
                .iter()
                .filter(|e| e.cond == c && e.owned == inv.owned && e.smithy == sig)
                .filter_map(|e| e.sig.clone().map(|s| (s, e.found.clone(), e.charms.clone())))
                .collect(),
            empty: self
                .owned_empty
                .iter()
                .filter(|m| m.owned == inv.owned && m.smithy == sig)
                .filter_map(|m| m.sig.clone().map(|s| (m.cond.clone(), s)))
                .collect(),
        };
        self.search_cond = Some((c.clone(), known_empty));
        let o = Opts { owned_empty: known_empty, ask_needed, ..Opts::MAIN };
        self.start_build(BuildReq::New(self.db, c, o, seeds, reuse), false);
        self.skip_diag = false;
        self.load_shown.set((0., false));
        self.dialog_sel = 0;
        self.persist();
    }

    fn start_build(&mut self, req: BuildReq, needed: bool) {
        self.build_generation = self.build_generation.wrapping_add(1);
        self.building = Some(Building { generation: self.build_generation, needed, started: crate::clock::ticks(), req: Some(req) });
    }

    /// Compute thread: take the search to build (inside the lock). It runs run() outside the lock and returns it with finish_build.
    pub fn take_build(&mut self) -> Option<(u32, BuildReq)> {
        let b = self.building.as_mut()?;
        b.req.take().map(|r| (b.generation, r))
    }

    /// Put back the built search. Dropped if it was cancelled or a new search started meanwhile.
    pub fn finish_build(&mut self, generation: u32, built: Option<Built>) {
        self.touch();
        if self.building.as_ref().is_none_or(|b| b.generation != generation) {
            return;
        }
        self.building = None;
        match built {
            Some(Built::Job(job)) => self.job = Some(job),
            Some(Built::Cached(mut found, charms)) => {
                // Newly obtained charms do not help with this condition (same charm signature): show the remembered results
                self.remap_charms(&mut found, &charms);
                self.cache_hit = true;
                self.finish_search(found);
            }
            None => self.message = k::MSG_NO_TARGET,
        }
    }

    fn build_now(&mut self) {
        if let Some((g, r)) = self.take_build() {
            let job = r.run();
            self.finish_build(g, job);
        }
    }

    /// Every frame: advance the search, cause analysis and extra skills a little.
    pub fn tick(&mut self, frame_ms: u64) {
        if self.inline_build {
            self.build_now();
        }
        if let Some(job) = self.job.as_mut() {
            let was = job.state;
            job.step(frame_ms);
            self.after_job_step(was, None);
            return;
        }
        // Cause analysis / extra skills: in the game the compute thread runs them outside the lock (take_diag, take_extra); PC tests run them here
        if self.diag_run.is_some() {
            if self.inline_build {
                self.tick_diag(frame_ms);
            }
            return;
        }
        if self.extra_run.is_some() && self.inline_build {
            self.tick_extra(frame_ms);
        }
    }

    /// Compute thread: take the running search (inside the lock). It steps it outside the lock and returns it with put_job.
    /// Meanwhile the draw shows the loading window from the job_out copy. Buttons are handled only after this thread puts it back, so they never overlap.
    pub fn take_job(&mut self) -> Option<Job> {
        if self.job.as_ref()?.state != JobState::Running {
            return None;
        }
        let job = self.job.take()?;
        self.job_out = Some(JobView { needed: job.needed, found: job.found(), started: job.started, progress: job.progress() });
        Some(job)
    }

    /// Put back the search run outside the lock. found: results already converted outside when it finished.
    pub fn put_job(&mut self, job: Job, was: JobState, found: Option<Found>) {
        self.touch();
        self.job_out = None;
        self.job = Some(job);
        self.after_job_step(was, found);
    }

    fn after_job_step(&mut self, was: JobState, found: Option<Found>) {
        let Some(job) = self.job.as_mut() else { return };
        {
            if job.state == JobState::Paused && was != JobState::Paused {
                self.dialog_sel = 0; // Default cursor: continue searching
            }
            if job.state == JobState::AskNeeded && was != JobState::AskNeeded && !job.owned_skipped {
                // The owned-charm phase finished with no results: remember this condition (stricter conditions skip it next time)
                if let Some((c, _)) = self.search_cond.clone() {
                    let charms = self.db.inv().charms.clone();
                    let owned = self.db.inv().owned.clone();
                    let smithy = smithy_sig(self.db, &c);
                    self.owned_empty.retain(|m| !(m.charms == charms && m.owned == owned && m.smithy == smithy && at_least_as_strict(&m.cond, &c)));
                    self.owned_empty.insert(0, OwnedEmpty { cond: c, charms, owned, smithy, sig: job.charm_sig.clone() });
                    self.owned_empty.truncate(8);
                }
            }
            if job.state == JobState::Done {
                let job = self.job.take().unwrap();
                let found = found.unwrap_or_else(|| job.result(self.db));
                self.last_search = Some(SearchStat::of(&job, if job.complete { "done" } else { "stopped" }));
                // Complete results are remembered with the conditions and owned equipment (shown at once next time for exactly the same conditions).
                // Results that were stopped or ended with 'unowned charms: no' are not remembered (a new search could differ).
                if job.complete {
                    if let Some((c, _)) = self.search_cond.clone() {
                        let inv = self.db.inv();
                        let smithy = smithy_sig(self.db, &c);
                        let entry = CachedSearch { cond: c, charms: inv.charms.clone(), owned: inv.owned.clone(), smithy, sig: job.charm_sig.clone(), found: found.clone() };
                        self.result_cache.retain(|e| !(e.cond == entry.cond && e.charms == entry.charms && e.owned == entry.owned && e.smithy == entry.smithy));
                        self.result_cache.insert(0, entry);
                        self.result_cache.truncate(4);
                    }
                }
                self.stat_nodes = 0;
                self.finish_search(found);
            }
        }
    }

    fn finish_search(&mut self, found: Found) {
        self.touch();
        self.results_cached = self.cache_hit;
        self.results = found.sets;
        self.needed_mode = found.needed;
        self.complete = found.complete;
        self.arrange(found.best);
        self.screen = Screen::Results;
        self.search_done = true;
        self.ridx = 0;
        self.rtop = 0;
        self.focus = Focus::List;
        self.pidx = 0;
        self.popup = None;
        self.extra = None;
        self.reset_alts();
        self.diag.clear();
        if self.results.is_empty() {
            self.notice = Some(if self.fixed_charm.is_some() { Notice::NoResultFixed } else { Notice::NoResult });
            if !core::mem::take(&mut self.skip_diag) {
                self.start_diag();
            }
        } else {
            self.notice = if self.needed_mode && !self.search_quiet { Some(Notice::Needed) } else { None };
        }
        self.persist();
    }

    fn arrange(&mut self, mut best: Vec<Set>) {
        if best.is_empty() {
            self.best_n = 0;
            self.show_all = true;
            return;
        }
        let top_free = best.iter().map(|r| r.free).max().unwrap_or(0);
        let top_def = best.iter().map(|r| r.def).max().unwrap_or(0);
        let jewels = self.db.jewel_use(&self.ranks, self.smithy, &self.excluded_jewels);
        for r in best.iter_mut() {
            let c = r.craft(self.db);
            // 'Most free slots' only if the free slots can activate something more: otherwise the set is just worse
            // (more parts to craft or lower defense for slots that hold nothing useful)
            let slots = r.free == top_free && r.free > 0 && r.free_slots_useful(self.db, &jewels);
            r.tags = (c == 0) as u8 | (slots as u8) << 1 | ((r.def == top_def) as u8) << 2;
        }
        // Only sets with a reason stay in the recommended block; the others go first in the rest
        let (tagged, untagged): (Vec<Set>, Vec<Set>) = best.into_iter().partition(|r| r.tags != 0);
        let mut best = tagged;
        let key = |r: &Set| ((0..PARTS).map(|p| r.head(p)).collect::<Vec<_>>(), r.charm);
        let seen: Vec<_> = best.iter().chain(untagged.iter()).map(key).collect();
        let rest: Vec<Set> = untagged.into_iter().chain(core::mem::take(&mut self.results).into_iter().filter(|r| !seen.contains(&key(r)))).collect();
        self.best_n = best.len();
        self.show_all = false;
        best.extend(rest);
        self.results = best;
    }

    // ---- Cause analysis ----
    fn start_diag(&mut self) {
        let mut cands: Vec<(DiagLabel, DiagChange)> = Vec::new();
        if self.forbid {
            cands.push((DiagLabel::Forbid, DiagChange { forbid: Some(false), ..DiagChange::NONE }));
        }
        if self.mindef != 0 {
            cands.push((DiagLabel::MinDef, DiagChange { mindef: Some(0), ..DiagChange::NONE }));
        }
        if self.res.iter().any(|&i| RES_STEPS[i].is_some()) {
            cands.push((DiagLabel::Res, DiagChange { res: Some([0; RES]), ..DiagChange::NONE }));
        }
        if self.fixed.iter().any(|&f| f != 0) || self.fixed_charm.is_some() {
            cands.push((DiagLabel::Fixed, DiagChange { fixed: Some(([0; PARTS], None)), ..DiagChange::NONE }));
        }
        if !self.excluded.is_empty() || !self.excluded_jewels.is_empty() {
            cands.push((DiagLabel::Excluded, DiagChange { excluded: Some((Vec::new(), Vec::new())), ..DiagChange::NONE }));
        }
        // The smithy filter hides armor and decorations without a trace: worth naming when it is the cause
        if self.smithy {
            cands.push((DiagLabel::Smithy, DiagChange { smithy: Some(false), ..DiagChange::NONE }));
        }
        for (i, &id) in self.targets.iter().enumerate() {
            let Some(s) = self.db.skill(id) else { continue };
            if let Some(low) = self.db.tree_skills(s.tree).filter(|x| x.threshold > 1 && x.threshold < s.threshold).max_by_key(|x| x.threshold) {
                let mut t = self.targets.clone();
                t[i] = low.id;
                cands.push((DiagLabel::Lower(id, low.id), DiagChange { targets: Some(t), ..DiagChange::NONE }));
            }
        }
        if self.targets.len() > 1 {
            for (i, &id) in self.targets.iter().enumerate() {
                let mut t = self.targets.clone();
                t.remove(i);
                cands.push((DiagLabel::Remove(id), DiagChange { targets: Some(t), ..DiagChange::NONE }));
            }
        }
        let n = cands.len();
        let conds = cands.iter().map(|(_, ch)| self.diag_cond(ch)).collect();
        self.diag_run = Some(DiagRun { cands, conds, status: alloc::vec![Check::Todo; n], job: None, pass: 1, next: 0, start: crate::clock::ticks(), cancel: false });
    }

    pub fn diag_progress(&self) -> (usize, usize) {
        if let Some(p) = self.diag_out {
            return p;
        }
        self.diag_run.as_ref().map_or((0, 0), |d| d.progress())
    }

    fn diag_cond(&self, ch: &DiagChange) -> Cond {
        let targets = ch.targets.clone().unwrap_or_else(|| self.targets.clone());
        let (fixed, fixed_charm) = ch.fixed.unwrap_or((self.fixed, self.fixed_charm));
        let (excluded, jewels) = ch.excluded.clone().unwrap_or_else(|| (self.excluded.clone(), self.excluded_jewels.clone()));
        let mut c = self.cond_with(&targets, ch.forbid.unwrap_or(self.forbid), ch.mindef.unwrap_or(self.mindef), ch.res.unwrap_or(self.res), fixed, fixed_charm, &excluded);
        c.excluded_jewels = jewels;
        c.smithy = ch.smithy.unwrap_or(self.smithy);
        c
    }

    /// PC tests (inline_build): no lock, so do it right here
    fn tick_diag(&mut self, frame_ms: u64) {
        let mut d = self.diag_run.take().unwrap();
        let done = d.work(self.db, frame_ms);
        self.put_diag(d, done);
    }

    /// Compute thread: take the cause analysis (DiagRun::work outside the lock, returned by put_diag).
    /// Building a searcher per candidate took over 100 ms on Recomp; inside the lock the window flickered (2026-11-19).
    pub fn take_diag(&mut self) -> Option<DiagRun> {
        let d = self.diag_run.take()?;
        self.diag_out = Some(d.progress());
        Some(d)
    }

    pub fn put_diag(&mut self, mut d: DiagRun, done: Option<Vec<(DiagLabel, DiagChange)>>) {
        self.touch();
        self.diag_out = None;
        if core::mem::take(&mut self.diag_cancel) {
            d.cancel = true;
        }
        match done {
            Some(list) => {
                self.diag = list;
                self.dialog_sel = 0;
            }
            None => self.diag_run = Some(d),
        }
    }

    /// Compute thread: take the extra-skill work (ExtraRun::work outside the lock, returned by put_extra)
    pub fn take_extra(&mut self) -> Option<ExtraRun> {
        let e = self.extra_run.take()?;
        self.extra_out = true;
        Some(e)
    }

    pub fn put_extra(&mut self, e: ExtraRun, done: Option<ExtraEnd>) {
        self.touch();
        self.extra_out = false;
        self.extra_prog = e.progress();
        if done.is_some() {
            self.extra_summary = Some(e.summary());
        }
        if core::mem::take(&mut self.extra_cancel) {
            return; // B while it ran: drop it
        }
        match done {
            Some(end) => {
                // Remember it for this game session (the same set asks again often: list -> extra skills -> back -> again)
                self.extra_cache.retain(|(k, _)| *k != e.key);
                self.extra_cache.insert(0, (e.key.clone(), end.clone()));
                self.extra_cache.truncate(EXTRA_CACHE);
                self.apply_extra_end(end);
            }
            None => self.extra_run = Some(e),
        }
    }

    fn apply_extra_end(&mut self, end: ExtraEnd) {
        match end {
            ExtraEnd::AskAny => self.ask(Confirm::ExtraAny), // None with the current charm: ask whether to search with unowned charms
            ExtraEnd::NoneAny => self.message = k::EXTRA_NONE_ANY,
            ExtraEnd::Found(best) => {
                self.extra = Some(best);
                self.eidx = 0;
                self.etop = 0;
                self.etab = 0;
            }
        }
    }
}

/// When the extra-skill work ends
#[derive(Clone)]
pub enum ExtraEnd {
    AskAny,
    NoneAny,
    Found(Vec<u16>),
}

impl DiagRun {
    fn progress(&self) -> (usize, usize) {
        (self.job.as_ref().map_or(self.next, |j| j.0), self.cands.len())
    }

    /// Compute for up to ms. When done, the chosen suggestions (empty if cancelled).
    pub fn work(&mut self, db: &'static Db, frame_ms: u64) -> Option<Vec<(DiagLabel, DiagChange)>> {
        const LIMIT: usize = 3;
        let d = self;
        let frame = crate::clock::ticks();
        loop {
            let elapsed = crate::clock::ms_since(d.start);
            let settled = {
                // Are LIMIT suggestions settled, from the highest priority
                let mut n = 0;
                let mut done = true;
                for s in &d.status {
                    if matches!(s, Check::Todo | Check::Partial) {
                        done = false;
                        break;
                    }
                    n += (*s == Check::Ok) as usize;
                    if n >= LIMIT {
                        break;
                    }
                }
                done
            };
            if d.job.is_none() && (d.cancel || elapsed >= DIAG_TIME_MS || settled || d.pass == 3) {
                return Some(if d.cancel {
                    Vec::new()
                } else {
                    (0..d.cands.len()).filter(|&i| d.status[i] == Check::Ok).take(LIMIT).map(|i| d.cands[i].clone()).collect()
                });
            }
            if let Some((i, job)) = d.job.as_mut() {
                job.step(frame_ms.saturating_sub(crate::clock::ms_since(frame)).max(1));
                if job.state != JobState::Running || d.cancel {
                    let i = *i;
                    d.status[i] = if job.found() > 0 { Check::Ok } else if job.state == JobState::Done && job.complete { Check::No } else { Check::Partial };
                    d.job = None;
                }
            } else {
                // Next candidate: pass 1 gives every candidate an equal time share, pass 2 gives the remaining time to candidates that hit the limit
                let left = DIAG_TIME_MS.saturating_sub(elapsed);
                let share = DIAG_TIME_MS / d.cands.len().max(1) as u64;
                let next = if d.pass == 1 {
                    (d.next < d.cands.len()).then_some(d.next)
                } else {
                    (d.next..d.cands.len()).find(|&i| d.status[i] == Check::Partial)
                };
                match next {
                    Some(i) => {
                        let ms = if d.pass == 1 { share.min(left) } else { left };
                        d.next = i + 1;
                        // Only 'is it possible' matters: stop at the first set, and use unowned (any) charms from the start.
                        let o = Opts { first: true, needed_only: true, max: 1, node_limit: DIAG_BUDGET, time_ms: ms.max(1), ..Opts::SEARCH };
                        match Job::new(db, &d.conds[i], o) {
                            Some(job) => d.job = Some((i, job)),
                            None => d.status[i] = Check::No,
                        }
                    }
                    None if d.pass == 1 => {
                        d.pass = 2;
                        d.next = 0;
                    }
                    None => d.pass = 3, // Both passes done
                }
            }
            if crate::clock::ms_since(frame) >= frame_ms || crate::job::should_yield() {
                return None;
            }
        }
    }
}

impl App {

    // ---- Extra skills ----
    fn start_extra(&mut self, any_charm: bool) {
        let Some(cur) = self.current().cloned() else { return };
        let mut c = self.cond_with(&cur.targets, self.forbid, 0, [0; RES], core::array::from_fn(|p| cur.head(p)), None, &[]);
        c.weapon = cur.weapon;
        c.min_res = [None; RES];
        c.charm_override = if any_charm { None } else { Some(cur.charm_data(self.db).unwrap_or(Charm { skills: [Pt::default(); 2], slots: 0 })) };
        // Same set and conditions as an earlier check in this session: show that result at once.
        // The result depends only on these (fixed armor, charm, weapon slots, targets, ranks, the smithy's decoration list
        // with 'smithy only'): not on the owned items.
        let key = alloc::format!("{}|{:?}|{}|{}", c.encode(), c.charm_override, any_charm, smithy_sig(self.db, &c));
        if let Some(end) = self.extra_cache.iter().find(|(k, _)| *k == key).map(|(_, end)| end.clone()) {
            self.extra_any = any_charm;
            self.extra_summary = None;
            self.apply_extra_end(end);
            return;
        }
        let mut skills: Vec<&Skill> = self.db.gd.skills.iter().filter(|s| s.threshold >= 2 && s.tree != TREE_TORSO_UP).collect();
        skills.sort_by_key(|s| (s.tree, s.threshold));
        self.extra_run = Some(ExtraRun {
            cond: c,
            any_charm,
            skills: skills.iter().map(|s| s.id).collect(),
            next: 0,
            failed_tree: 0,
            found: Vec::new(),
            job: None,
            start: crate::clock::ticks(),
            tried: 0,
            limited: 0,
            build_ticks: 0,
            step_ticks: 0,
            nodes: 0,
            skipped: 0,
            key,
        });
        self.extra_prog = (0, skills.len() as u16);
        self.extra_any = any_charm;
    }

    /// PC tests (inline_build): right here
    fn tick_extra(&mut self, frame_ms: u64) {
        let mut e = self.extra_run.take().unwrap();
        let done = e.work(self.db, frame_ms);
        self.put_extra(e, done);
    }
}

impl ExtraRun {
    /// Compute for up to ms (outside the lock). The result when done.
    pub fn work(&mut self, db: &'static Db, frame_ms: u64) -> Option<ExtraEnd> {
        let e = self;
        let frame = crate::clock::ticks();
        loop {
            if let Some((id, job)) = e.job.as_mut() {
                let t = crate::clock::ticks();
                job.step(frame_ms);
                e.step_ticks += crate::clock::ticks() - t;
                if job.state != JobState::Running {
                    let ok = job.found() > 0;
                    e.tried += 1;
                    e.nodes += job.nodes;
                    e.limited += (!ok && !job.complete) as u32;
                    let id = *id;
                    let tree = db.skill(id).map_or(0, |s| s.tree);
                    if ok {
                        e.found.push(id);
                    } else {
                        e.failed_tree = tree; // If a lower level fails, higher levels fail too
                    }
                    e.job = None;
                }
            } else if e.next >= e.skills.len() {
                // Only the highest level per tree
                let mut best: Vec<u16> = Vec::new();
                for &id in &e.found {
                    let s = db.skill(id).unwrap();
                    match best.iter().position(|&b| db.skill(b).unwrap().tree == s.tree) {
                        Some(i) => {
                            if db.skill(best[i]).unwrap().threshold < s.threshold {
                                best[i] = id;
                            }
                        }
                        None => best.push(id),
                    }
                }
                return Some(if best.is_empty() && !e.any_charm {
                    ExtraEnd::AskAny
                } else if best.is_empty() {
                    ExtraEnd::NoneAny
                } else {
                    ExtraEnd::Found(best)
                });
            } else {
                let id = e.skills[e.next];
                e.next += 1;
                let Some(s) = db.skill(id) else { continue };
                if s.tree == e.failed_tree {
                    continue;
                }
                // Most skills cannot fit this set at all: the most points the tree can reach (fixed armor, best decorations
                // in every slot, the charm) is a cheap upper bound, so those skip the search (same result, no job built)
                if point_bound(db, &e.cond, e.any_charm, s.tree) < s.threshold as i32 {
                    e.skipped += 1;
                    e.failed_tree = s.tree;
                    continue;
                }
                let mut t = e.cond.targets.clone();
                match t.iter().position(|x| x.0 == s.tree) {
                    Some(i) if t[i].1 >= s.threshold as i16 => continue, // Level already in the targets
                    Some(i) => t[i].1 = s.threshold as i16,
                    None if t.len() < MAX_TARGETS => t.push((s.tree, s.threshold as i16)),
                    None => continue,
                }
                let c = Cond { targets: t, ..e.cond.clone() };
                let o = Opts { first: true, needed_only: e.any_charm, allow_needed: e.any_charm, dominance: false, group: false, max: 1, node_limit: 200_000, time_ms: 1_000, ..Opts::SEARCH };
                let t = crate::clock::ticks();
                let job = Job::new(db, &c, o);
                e.build_ticks += crate::clock::ticks() - t;
                if let Some(job) = job {
                    e.job = Some((id, job));
                }
            }
            if crate::clock::ms_since(frame) >= frame_ms || crate::job::should_yield() {
                return None;
            }
        }
    }
}

/// Upper bound of a tree's points for the extra-skill search, where the armor is fixed to the current set:
/// fixed armor points + the best decorations in every slot (torso slots and points counted once more per Torso Up part)
/// + the most the charm can give. Ignores negatives from decorations and the slots the targets need, so it never cuts a
/// possible skill. With a part not fixed it gives no bound.
fn point_bound(db: &Db, c: &Cond, any_charm: bool, tree: u16) -> i32 {
    let mut armor: [Option<&Armor>; PARTS] = [None; PARTS];
    for p in 0..PARTS {
        match db.armor(c.fixed[p]) {
            Some(a) => armor[p] = Some(a),
            None => return i32::MAX,
        }
    }
    let torso_up = (0..PARTS).filter(|&p| p != TORSO && armor[p].is_some_and(|a| a.skills.iter().any(|s| s.tree == TREE_TORSO_UP && s.pt > 0))).count() as i32;
    // Best points in a container of n slots (n = 0..3) with decorations of this tree (unbounded knapsack)
    let jewels = db.jewel_use(&c.ranks, c.smithy, &c.excluded_jewels);
    let mut best = [0i32; 4];
    for n in 1..4 {
        for j in db.gd.jewels.iter().filter(|j| j.pos.tree == tree && j.pos.pt > 0 && jewels.allows(j)) {
            if (j.size as usize) <= n {
                best[n] = best[n].max(best[n - j.size as usize] + j.pos.pt as i32);
            }
        }
        best[n] = best[n].max(best[n - 1]);
    }
    let mut total = 0;
    for (p, a) in armor.iter().enumerate() {
        let a = a.unwrap();
        let own: i32 = a.skills.iter().filter(|s| s.tree == tree).map(|s| s.pt as i32).sum();
        let here = own + best[(a.slots as usize).min(3)];
        total += if p == TORSO { here * (1 + torso_up) } else { here };
    }
    total += best[(c.weapon as usize).min(3)];
    let charm = match (&c.charm_override, any_charm) {
        (Some(ch), false) => ch.skills.iter().filter(|s| s.tree == tree).map(|s| s.pt as i32).sum::<i32>() + best[(ch.slots as usize).min(3)],
        // Any charm: the most any charm kind can roll for this tree in either skill, plus the best 3-slot decorations
        _ => {
            let table = db
                .gd
                .charm_kinds
                .iter()
                .map(|k| k.first.iter().chain(k.second.iter()).filter(|cs| cs.tree == tree).map(|cs| cs.hi as i32).sum::<i32>())
                .max()
                .unwrap_or(0);
            let owned = db.inv().charms.iter().map(|ch| ch.skills.iter().filter(|s| s.tree == tree).map(|s| s.pt as i32).sum::<i32>()).max().unwrap_or(0);
            table.max(owned) + best[3]
        }
    };
    total + charm.max(0)
}

impl App {

    /// Does the compute thread have work to continue right away (search, cause analysis, extra skills, alternatives)
    pub fn busy(&self) -> bool {
        self.building.is_some()
            || self.job_out.is_some()
            || self.job.as_ref().is_some_and(|j| j.state == JobState::Running) || self.diag_run.is_some() || self.diag_out.is_some() || self.extra_busy() || self.alt_run.is_some()
    }

    /// Extra skills split by the skill picker's categories: (tab number as in the picker, skills). Empty categories
    /// are left out; a skill in no category goes to the last tab.
    pub fn extra_tabs(&self) -> Vec<(usize, Vec<u16>)> {
        let Some(extra) = &self.extra else { return Vec::new() };
        let mut placed = alloc::vec![false; extra.len()];
        let mut out: Vec<(usize, Vec<u16>)> = Vec::new();
        for (c, ids) in self.db.categories.iter().enumerate() {
            let mut items = Vec::new();
            for &id in ids {
                if let Some(i) = extra.iter().position(|&x| x == id) {
                    if !placed[i] {
                        placed[i] = true;
                        items.push(id);
                    }
                }
            }
            if !items.is_empty() {
                out.push((c + 1, items));
            }
        }
        let rest: Vec<u16> = extra.iter().zip(&placed).filter(|(_, p)| !**p).map(|(&id, _)| id).collect();
        if !rest.is_empty() {
            match out.last_mut() {
                Some(last) => last.1.extend(rest),
                None => out.push((self.db.categories.len(), rest)),
            }
        }
        out
    }

    /// Extra skills of the current tab
    pub fn extra_list(&self) -> Vec<u16> {
        let mut tabs = self.extra_tabs();
        if self.etab < tabs.len() { tabs.swap_remove(self.etab).1 } else { Vec::new() }
    }

    pub fn extra_busy(&self) -> bool {
        self.extra_run.is_some() || self.extra_out
    }

    // ---- Fixed ----
    pub fn fix_items(&self, slot: usize) -> Vec<FixItem> {
        // If already fixed to the equipped piece, drop 'Use equipped'
        let inv = self.db.inv();
        let worn_fixed = if slot == CHARM_ROW {
            inv.equipped_charm.is_some() && self.fixed_charm == inv.equipped_charm
        } else {
            inv.equipped_armor[slot] != 0 && self.fixed[slot] == inv.equipped_armor[slot]
        };
        let mut v = if worn_fixed { alloc::vec![FixItem::Box] } else { alloc::vec![FixItem::Equipped, FixItem::Box] };
        if self.is_fixed(slot) {
            v.push(FixItem::Release);
        }
        v
    }

    /// Exclude list of the part row under the cursor (results screen): its armor (armor rows), then the decorations
    /// placed in it (each once)
    pub fn exclude_items(&self) -> Vec<ExItem> {
        let Some(cur) = self.current() else { return Vec::new() };
        let mut out = Vec::new();
        if self.pidx < PARTS {
            out.push(ExItem::Armor(cur.head(self.pidx)));
        }
        let container = if self.pidx < PARTS { self.pidx } else if self.pidx == CHARM_ROW { crate::set::CHARM } else { crate::set::WEAPON };
        let (placed, _) = cur.placement(self.db);
        for &j in &placed[container] {
            if !out.contains(&ExItem::Jewel(j)) {
                out.push(ExItem::Jewel(j));
            }
        }
        out
    }

    fn on_exmenu(&mut self, b: Button) {
        let idx = self.exmenu.unwrap_or(0);
        let items = self.exclude_items();
        match b {
            Up | Down if !items.is_empty() => self.exmenu = Some(step(idx, items.len(), b)),
            B => self.exmenu = None,
            A => {
                self.exmenu = None;
                match items.get(idx) {
                    Some(ExItem::Armor(id)) => self.ask(Confirm::Exclude(*id)),
                    Some(ExItem::Jewel(id)) => self.ask(Confirm::ExcludeJewel(*id)),
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn open_fixmenu(&mut self, slot: usize) {
        self.fixmenu = Some((slot, 0));
        self.fix_from = self.screen;
    }

    fn set_fix(&mut self, slot: usize, value: Option<u16>) {
        if slot == CHARM_ROW {
            self.fixed_charm = value.map(|v| v as usize);
        } else {
            let v = value.unwrap_or(0);
            self.excluded.retain(|&x| x != v);
            self.fixed[slot] = v;
        }
        self.persist();
        if self.fix_from == Screen::Results {
            self.research_from_results();
            self.run_search();
        } else {
            self.screen = Screen::Main;
        }
    }

    fn on_fixmenu(&mut self, b: Button) {
        let (slot, idx) = self.fixmenu.unwrap();
        let items = self.fix_items(slot);
        match b {
            Up | Down => self.fixmenu = Some((slot, step(idx, items.len(), b))),
            B => self.fixmenu = None,
            A => {
                self.fixmenu = None;
                match items[idx] {
                    FixItem::Release => self.set_fix(slot, None),
                    FixItem::Box => {
                        self.screen = Screen::Box;
                        self.box_slot = slot;
                        self.bidx = 0;
                        self.btop = 0;
                    }
                    FixItem::Equipped => {
                        let worn = if slot == CHARM_ROW { self.db.inv().equipped_charm.map(|c| c as u16) } else { Some(self.db.inv().equipped_armor[slot]).filter(|&v| v != 0) };
                        match worn {
                            Some(v) => self.set_fix(slot, Some(v)),
                            None => {
                                self.message = k::MSG_NOTHING_WORN;
                                self.message_slot = slot;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Choices from the equipment box: armor ids, or owned charm indices for the charm row
    pub fn box_items(&self) -> Vec<u16> {
        if self.box_slot == CHARM_ROW {
            return (0..self.db.inv().charms.len() as u16).collect();
        }
        let db = self.db;
        let mut ids: Vec<u16> = db
            .inv()
            .owned
            .iter()
            .copied()
            .filter(|&i| {
                let sex = if db.inv().sex == 2 { 2 } else { 1 };
                db.armor(i).is_some_and(|a| a.part as usize == self.box_slot && (a.kind == 0 || a.kind == self.kind) && (a.sex == 0 || a.sex == sex))
            })
            .collect();
        ids.sort_by_key(|&i| core::cmp::Reverse(db.armor(i).map_or(0, |a| a.def)));
        ids
    }

    fn on_box(&mut self, b: Button) {
        let items = self.box_items();
        match b {
            Up | Down | Left | Right => (self.bidx, self.btop) = move_cursor(self.bidx, items.len(), MANAGE_ROWS, b),
            B => self.screen = self.fix_from,
            A if !items.is_empty() => self.set_fix(self.box_slot, Some(items[self.bidx])),
            _ => {}
        }
    }

    // ---- Condition screen ----
    pub fn armor_use(&self, i: usize) -> bool {
        match i {
            0..=2 => self.ranks[i],
            3 => self.deviant,
            4 => self.arena,
            5 => self.reward,
            _ => self.event,
        }
    }

    pub fn opt_value(&self, opt: Opt) -> usize {
        match opt {
            Opt::Kind => self.kind as usize - 1,
            Opt::Weapon => self.weapon,
            // Dropdown index 0 free slots, 1 lowest rarity (engine priority 0 / 2; defense priority was removed)
            Opt::Priority => (self.priority == 2) as usize,
            Opt::MinDef => self.mindef,
            _ => 0,
        }
    }

    fn on_main(&mut self, b: Button) {
        if let Some((elem, idx)) = self.subdrop {
            match b {
                Up | Down => self.subdrop = Some((elem, step(idx, RES_STEPS.len(), b))),
                A => {
                    self.res[elem] = idx;
                    self.subdrop = None;
                    self.persist();
                }
                B => self.subdrop = None,
                _ => {}
            }
            return;
        }
        if let Some((name, idx)) = self.sub {
            let n = if name == Opt::Armor { ARMOR_USE } else { RES };
            match b {
                Up | Down => self.sub = Some((name, step(idx, n, b))),
                B | Y => self.sub = None,
                A if name == Opt::Resist => self.subdrop = Some((idx, self.res[idx])),
                A => {
                    match idx {
                        0..=2 => self.ranks[idx] = !self.ranks[idx],
                        3 => self.deviant = !self.deviant,
                        4 => self.arena = !self.arena,
                        5 => self.reward = !self.reward,
                        _ => self.event = !self.event,
                    }
                    self.persist();
                }
                _ => {}
            }
            return;
        }
        if let Some((opt, idx)) = self.drop {
            match b {
                Up | Down => self.drop = Some((opt, step(idx, opt.choices(), b))),
                A => {
                    match opt {
                        Opt::Kind => self.kind = idx as u8 + 1,
                        Opt::Weapon => self.weapon = idx,
                        Opt::Priority => self.priority = if idx == 1 { 2 } else { 0 },
                        Opt::MinDef => self.mindef = idx,
                        _ => {}
                    }
                    self.drop = None;
                    self.persist();
                }
                B => self.drop = None,
                _ => {}
            }
            return;
        }
        if let Some(m) = self.menu {
            match b {
                Up | Down => self.menu = Some(step(m, MENU.len(), b)),
                B | Y => self.menu = None,
                A => {
                    self.menu = None;
                    match MENU[m] {
                        MenuItem::ResetTargets => self.ask(Confirm::ResetTargets),
                        MenuItem::ResetFixed => self.ask(Confirm::ResetFixed),
                        MenuItem::Excluded => {
                            self.screen = Screen::Manage;
                            self.midx = 0;
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        let counts = [self.targets.len() + 1, OPTIONS.len(), FIX_ROWS];
        match b {
            Up | Down => self.row = step(self.row, counts[self.col], b),
            Left | Right => {
                self.col = if b == Right { (self.col + 1) % 3 } else { (self.col + 2) % 3 };
                self.row = self.row.min(counts[self.col] - 1);
            }
            Plus => self.run_search(),
            Minus => self.open_saved_from_main(),
            // Y: a whole-panel action per column. Clearing asks first (cursor on 'no'); excluded armor opens its screen.
            Y if self.col == 0 => {
                if !self.targets.is_empty() {
                    self.ask(Confirm::ResetTargets);
                }
            }
            Y if self.col == 1 => {
                self.screen = Screen::Manage;
                self.midx = 0;
            }
            Y => {
                if (0..FIX_ROWS).any(|r| self.is_fixed(r)) {
                    self.ask(Confirm::ResetFixed);
                }
            }
            // X on a fixed row: release just that one, no dialog
            X if self.col == 2 => {
                if self.is_fixed(self.row) {
                    self.fix_from = Screen::Main;
                    self.set_fix(self.row, None);
                }
            }
            _ if self.col == 0 => {
                if b == A && self.row < self.targets.len() {
                    self.open_skills(Some(self.targets[self.row]));
                } else if b == A && self.targets.len() < MAX_TARGETS {
                    self.open_skills(None);
                } else if b == X && self.row < self.targets.len() {
                    self.targets.remove(self.row);
                    self.row = self.row.min(self.targets.len());
                    self.persist();
                }
            }
            A if self.col == 2 => self.open_fixmenu(self.row),
            A => {
                let opt = OPTIONS[self.row];
                if opt.choices() > 0 {
                    self.drop = Some((opt, self.opt_value(opt)));
                } else if matches!(opt, Opt::Armor | Opt::Resist) {
                    self.sub = Some((opt, 0));
                } else if opt == Opt::Forbid {
                    self.forbid = !self.forbid;
                } else if opt == Opt::Smithy {
                    self.smithy = !self.smithy;
                    if self.smithy && self.db.inv().smithy.is_none() {
                        self.message = k::MSG_SMITHY_UNREAD;
                    }
                } else if opt == Opt::Search {
                    self.run_search();
                }
                self.persist();
            }
            _ => {}
        }
    }

    // ---- Excluded armor ----
    /// Rows of the excluded gear screen: armor first, then decorations
    pub fn manage_items(&self) -> Vec<ExItem> {
        self.excluded.iter().map(|&a| ExItem::Armor(a)).chain(self.excluded_jewels.iter().map(|&j| ExItem::Jewel(j))).collect()
    }

    fn on_manage(&mut self, b: Button) {
        let items = self.manage_items();
        let n = items.len();
        match b {
            Up | Down if n > 0 => self.midx = step(self.midx, n, b),
            B => self.screen = Screen::Main,
            X if n > 0 => match items[self.midx.min(n - 1)] {
                ExItem::Armor(id) => self.ask(Confirm::Release(id)),
                ExItem::Jewel(id) => self.ask(Confirm::ReleaseJewel(id)),
            },
            Y if n > 0 => self.ask(Confirm::ReleaseAll),
            _ => {}
        }
    }

    // ---- Skill picker ----
    /// Tab 0 is recent, categories from 1
    pub fn tab_count(&self) -> usize {
        1 + self.db.categories.len()
    }
    pub fn skill_list(&self) -> Vec<u16> {
        if self.tab == 0 { self.recent.clone() } else { self.db.categories[self.tab - 1].clone() }
    }

    fn open_skills(&mut self, skill: Option<u16>) {
        self.screen = Screen::Skills;
        self.sidx = 0;
        self.stop = 0;
        if self.recent.is_empty() && self.tab == 0 {
            self.tab = 1;
        }
        let Some(id) = skill else { return };
        for (tab, ids) in self.db.categories.iter().enumerate() {
            if let Some(i) = ids.iter().position(|&x| x == id) {
                self.tab = tab + 1;
                self.sidx = i;
                self.stop = i / SKILL_ROWS * SKILL_ROWS;
                return;
            }
        }
    }

    /// Condition screen − : the saved sets
    fn open_saved_from_main(&mut self) {
        self.screen = Screen::Saved;
        self.ridx = 0;
        self.rtop = 0;
        self.focus = Focus::List;
        self.popup = None;
        self.saved_from = Screen::Main;
    }

    fn on_skills(&mut self, b: Button) {
        let items = self.skill_list();
        match b {
            L | R => {
                let n = self.tab_count();
                self.tab = if b == R { (self.tab + 1) % n } else { (self.tab + n - 1) % n };
                self.sidx = 0;
                self.stop = 0;
            }
            Up | Down | Left | Right => (self.sidx, self.stop) = move_cursor(self.sidx, items.len(), SKILL_ROWS, b),
            B => {
                self.screen = Screen::Main;
                if self.col == 0 {
                    self.row = self.row.min(self.targets.len());
                }
            }
            A if !items.is_empty() => {
                let pick = items[self.sidx];
                let tree = self.db.skill(pick).map_or(0, |s| s.tree);
                let same = self.targets.iter().position(|&id| self.db.skill(id).is_some_and(|s| s.tree == tree));
                match same {
                    Some(i) if self.targets[i] == pick => {
                        self.targets.remove(i);
                    }
                    Some(i) => self.targets[i] = pick, // Only one level per tree
                    None if self.targets.len() < MAX_TARGETS => self.targets.push(pick),
                    None => self.message = k::MSG_FULL,
                }
                self.persist();
            }
            _ => {}
        }
    }

    // ---- Results / saved sets ----
    fn replace_current(&mut self, new: Set) {
        let Some(i) = self.current_index() else { return };
        if self.screen == Screen::Saved {
            self.saved[i] = new;
        } else {
            self.results[i] = new;
        }
    }

    fn on_results(&mut self, b: Button) {
        if self.extra.is_some() {
            let extra = self.extra_list();
            match b {
                // L/R: category tabs (same as the skill picker)
                L | R => {
                    let n = self.extra_tabs().len().max(1);
                    self.etab = if b == R { (self.etab + 1) % n } else { (self.etab + n - 1) % n };
                    self.eidx = 0;
                    self.etop = 0;
                }
                Up | Down | Left | Right => (self.eidx, self.etop) = move_cursor(self.eidx, extra.len(), EXTRA_ROWS, b),
                B => {
                    self.extra = None;
                    self.extra_any = false;
                }
                A if self.eidx < extra.len() => {
                    let pick = extra[self.eidx];
                    let tree = self.db.skill(pick).map_or(0, |s| s.tree);
                    match self.targets.iter().position(|&id| self.db.skill(id).is_some_and(|s| s.tree == tree)) {
                        Some(i) => self.targets[i] = pick,
                        None if self.targets.len() < MAX_TARGETS => self.targets.push(pick),
                        None => {
                            self.message = k::MSG_FULL;
                            return;
                        }
                    }
                    self.extra = None;
                    // A skill from the unowned-charm list: the user already chose unowned charms, so if owned charms find
                    // nothing the search goes on with unowned charms without asking again
                    if core::mem::take(&mut self.extra_any) {
                        self.auto_needed = true;
                    }
                    self.run_search();
                }
                _ => {}
            }
            return;
        }
        if self.extra_busy() {
            // B: stop looking for extra skills
            if b == B {
                if self.extra_run.take().is_none() {
                    self.extra_cancel = true; // the compute thread has it: dropped when it comes back
                }
            }
            return;
        }
        if let Some(popup) = self.popup.clone() {
            match b {
                Up | Down => self.aidx = step(self.aidx, popup.len(), b),
                // ZL/ZR: detail pages (info / materials) of the armor under the popup cursor
                ZL | ZR => self.dpage = 1 - self.dpage,
                // Left/right: previous/next page (the list can be long)
                Left | Right if popup.len() > ALT_ROWS => {
                    let pages = popup.len().div_ceil(ALT_ROWS);
                    let page = (self.aidx / ALT_ROWS + if b == Right { 1 } else { pages - 1 }) % pages;
                    self.aidx = (page * ALT_ROWS + self.aidx % ALT_ROWS).min(popup.len() - 1);
                }
                A => {
                    let (aid, _, res) = popup[self.aidx].clone();
                    let p = self.pidx;
                    match res {
                        None => {
                            // Same group: move it to the front for display
                            if let Some(i) = self.current_index() {
                                let store = if self.screen == Screen::Saved { &mut self.saved } else { &mut self.results };
                                let ids = &mut store[i].parts[p];
                                if let Some(at) = ids.iter().position(|&x| x == aid) {
                                    let v = ids.remove(at);
                                    ids.insert(0, v);
                                }
                            }
                        }
                        Some(idx) => {
                            // Another armor: switch to the set that uses it (decorations and free slots change too)
                            let Some(mut set) = self.alt_cache.as_ref().and_then(|(_, l)| l.get(p)).and_then(|l| l.sets.get(idx as usize)).cloned() else {
                                self.popup = None;
                                return;
                            };
                            if let Some(at) = set.parts[p].iter().position(|&x| x == aid) {
                                let v = set.parts[p].remove(at);
                                set.parts[p].insert(0, v);
                            }
                            set.tags = self.current().map_or(0, |c| c.tags);
                            self.replace_current(set);
                            // Only this set changed: drop only this set's alternative memory
                            if let Some(i) = self.current_index() {
                                if let Some(c) = self.alt_all.get_mut(i) {
                                    *c = None;
                                }
                            }
                            self.alt_cache = None;
                            if self.alt_run.as_ref().is_some_and(|r| Some(r.key) == self.current_index()) {
                                self.alt_run = None;
                            }
                        }
                    }
                    self.popup = None;
                }
                B => self.popup = None,
                _ => {}
            }
            return;
        }
        if self.focus == Focus::List {
            let rows = self.rows();
            let per = if self.screen == Screen::Saved { SAVED_ROWS } else { LIST_ROWS };
            let cur = self.current().cloned();
            match b {
                Up | Down | Left | Right => (self.ridx, self.rtop) = move_cursor(self.ridx, rows.len(), per, b),
                // Compare screen: B or + returns to the result list. Part selection (A) is blocked.
                B | Plus if self.compare && self.screen == Screen::Results => self.compare = false,
                A if self.compare && self.screen == Screen::Results => {}
                A if rows.get(self.ridx) == Some(&None) => self.show_all = !self.show_all,
                A if cur.is_some() => {
                    self.focus = Focus::Parts;
                    self.pidx = 0;
                    self.compare = false;
                }
                B if self.screen == Screen::Saved && self.saved_from == Screen::Results => {
                    self.screen = Screen::Results;
                    self.focus = Focus::List;
                    (self.ridx, self.rtop) = self.results_pos;
                }
                // Closing the results means searching again, so confirm
                B if self.screen == Screen::Results => self.ask(Confirm::LeaveResults),
                B => self.screen = Screen::Main,
                Minus if self.screen == Screen::Results => {
                    self.results_pos = (self.ridx, self.rtop);
                    self.saved_from = Screen::Results;
                    self.screen = Screen::Saved;
                    self.ridx = 0;
                    self.rtop = 0;
                    self.compare = false;
                }
                Plus if cur.is_some() && self.screen == Screen::Results => self.compare = true,
                X if cur.is_some() && self.screen == Screen::Results => {
                    self.eidx = 0;
                    self.etop = 0;
                    self.start_extra(false);
                }
                Y if cur.is_some() && self.screen == Screen::Results => {
                    let cur = cur.unwrap();
                    if self.is_saved(&cur) {
                        // Already saved: the hint is dimmed as 'Saved'
                    } else if self.saved.len() >= MAX_SAVED {
                        self.message = k::MSG_SAVED_FULL;
                    } else {
                        self.ask(Confirm::Save);
                    }
                }
                X if cur.is_some() && self.screen == Screen::Saved => {
                    if let Some(i) = self.current_index() {
                        self.ask(Confirm::DeleteSaved(i));
                    }
                }
                _ => {}
            }
            return;
        }
        // Part selection
        let Some(cur) = self.current().cloned() else {
            self.focus = Focus::List;
            return;
        };
        match b {
            Up | Down => self.pidx = step(self.pidx, PART_ROWS, b),
            ZL | ZR if self.pidx < PARTS => self.dpage = 1 - self.dpage,
            B => self.focus = Focus::List,
            A if self.pidx < PARTS => match self.part_alts(self.pidx) {
                Some(alts) => self.open_alt_popup(alts),
                // Counted but the popup list is not ready: open it as soon as the compute thread builds it
                None => self.alt_popup_wait = Some(self.pidx),
            },
            _ if self.screen != Screen::Results => {}
            X => match self.exclude_items().as_slice() {
                [] => {}
                // Only the armor (no decorations in this part): straight to the question, as before
                [ExItem::Armor(id)] => self.ask(Confirm::Exclude(*id)),
                _ => self.exmenu = Some(0),
            },
            Y if self.pidx < PARTS || (self.pidx == CHARM_ROW && cur.charm >= 0) => {
                if self.is_fixed(self.pidx) {
                    self.open_fixmenu(self.pidx);
                } else {
                    self.fix_from = Screen::Results;
                    let v = if self.pidx == CHARM_ROW { cur.charm as u16 } else { cur.head(self.pidx) };
                    self.set_fix(self.pidx, Some(v));
                }
            }
            _ => {}
        }
    }

    // ---- Saving ----
    pub fn encode(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        let list = |v: &[u16]| v.iter().map(|x| alloc::format!("{}", x)).collect::<Vec<_>>().join(",");
        let _ = writeln!(s, "version=1");
        let _ = writeln!(s, "targets={}", list(&self.targets));
        let _ = writeln!(s, "kind={}", self.kind);
        let _ = writeln!(s, "weapon={}", self.weapon);
        let _ = writeln!(s, "ranks={},{},{}", self.ranks[0] as u8, self.ranks[1] as u8, self.ranks[2] as u8);
        let _ = writeln!(s, "forbid={}", self.forbid as u8);
        let _ = writeln!(s, "priority={}", self.priority);
        let _ = writeln!(s, "arena={}", self.arena as u8);
        let _ = writeln!(s, "event={}", self.event as u8);
        let _ = writeln!(s, "reward={}", self.reward as u8);
        let _ = writeln!(s, "deviant={}", self.deviant as u8);
        let _ = writeln!(s, "smithy={}", self.smithy as u8);
        let _ = writeln!(s, "mindef={}", self.mindef);
        let _ = writeln!(s, "res={}", self.res.iter().map(|x| alloc::format!("{}", x)).collect::<Vec<_>>().join(","));
        let _ = writeln!(s, "fixed={}", list(&self.fixed));
        let _ = writeln!(s, "fixed_charm={}", self.fixed_charm.map_or(-1, |c| c as i32));
        let _ = writeln!(s, "excluded={}", list(&self.excluded));
        let _ = writeln!(s, "excluded_jewels={}", list(&self.excluded_jewels));
        let _ = writeln!(s, "recent={}", list(&self.recent));
        for set in &self.saved {
            let _ = writeln!(s, "saved={}", set.encode(self.db));
        }
        s
    }

    pub fn decode(&mut self, text: &str) {
        self.touch();
        let nums = |v: &str| v.split(',').filter_map(|x| x.parse::<u16>().ok()).collect::<Vec<u16>>();
        self.saved.clear();
        // Keys newer than some saves: start from the default so another character's value does not carry over
        self.excluded_jewels.clear();
        self.deviant = false;
        self.smithy = false;
        for line in text.lines() {
            let Some((key, v)) = line.split_once('=') else { continue };
            match key {
                "targets" => self.targets = nums(v).into_iter().filter(|&id| self.db.skill(id).is_some()).collect(),
                "kind" => self.kind = v.parse().unwrap_or(1).clamp(1, 2),
                "weapon" => self.weapon = v.parse().unwrap_or(0).min(3),
                "ranks" => {
                    let r = nums(v);
                    if r.len() == 3 {
                        self.ranks = [r[0] == 1, r[1] == 1, r[2] == 1];
                    }
                }
                "forbid" => self.forbid = v == "1",
                // 1 (defense priority) no longer exists: falls back to free slots
                "priority" => self.priority = if v == "2" { 2 } else { 0 },
                "thorough" => {} // older saves: the search mode choice was removed (always thorough)
                "arena" => self.arena = v == "1",
                "event" => self.event = v == "1",
                "reward" => self.reward = v == "1",
                "deviant" => self.deviant = v == "1",
                "smithy" => self.smithy = v == "1",
                "mindef" => self.mindef = v.parse().unwrap_or(0).min(MIN_DEF.len() - 1),
                "res" => {
                    let r = nums(v);
                    if r.len() == RES {
                        self.res = core::array::from_fn(|e| (r[e] as usize).min(RES_STEPS.len() - 1));
                    }
                }
                "fixed" => {
                    let r = nums(v);
                    if r.len() == PARTS {
                        self.fixed = core::array::from_fn(|p| r[p]);
                    }
                }
                "fixed_charm" => self.fixed_charm = v.parse::<i32>().ok().filter(|&c| c >= 0).map(|c| c as usize),
                "excluded" => self.excluded = nums(v),
                "excluded_jewels" => self.excluded_jewels = nums(v),
                "recent" => self.recent = nums(v).into_iter().filter(|&id| self.db.skill(id).is_some()).collect(),
                "saved" => {
                    if let Some(mut set) = Set::decode(v) {
                        if set.needed {
                            if let Some(c) = set.saved_charm {
                                set.chance = skillsim_engine::charm::chance(&self.db.gd.charm_kinds, &c);
                            }
                        }
                        if self.saved.len() < MAX_SAVED {
                            self.saved.push(set);
                        }
                    }
                }
                _ => {}
            }
        }
        self.tab = if self.recent.is_empty() { 1 } else { 0 };
    }
}
