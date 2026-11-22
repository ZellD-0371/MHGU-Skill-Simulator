//! MHGU Skill Simulator Forge plugin. The search uses the engine (skillsim-engine) as is.
//! Screens and controls match the preview (preview/). Opened with the 'Open' button in the Forge settings menu.
#![no_std]
extern crate alloc;

/// 'Enable logging' in the Forge menu (off by default): our info and warning lines are written only while it is on.
/// Errors are always written.
static LOGGING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub fn logging() -> bool {
    LOGGING.load(core::sync::atomic::Ordering::Relaxed)
}
// Defined before the modules so every module uses these instead of Forge's
macro_rules! info {
    ($($t:tt)*) => {
        if crate::logging() {
            forge::log::prelude::info!($($t)*)
        }
    };
}
macro_rules! warn {
    ($($t:tt)*) => {
        if crate::logging() {
            forge::log::prelude::warn!($($t)*)
        }
    };
}

mod app;
mod clock;
mod db;
mod generated;
mod input;
mod inventory;
mod job;
mod names;
mod game_menu;
mod res;
mod session;
mod prompts;
mod set;
mod sound;
mod storage;
mod ui;
mod window_view;
mod ext_block;
mod picture;
mod purplelib;

use core::{
    cell::UnsafeCell,
    ffi::c_void,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use forge::log::prelude::error;

static SIMDATA: &[u8] = include_bytes!("../../data/simdata.bin");

struct Shared {
    lock: AtomicBool,
    data: UnsafeCell<Option<Plugin>>,
}
unsafe impl Sync for Shared {}
impl Shared {
    /// Wait at most ms (for drawing). Waiting while the compute thread briefly holds the lock keeps the window from flickering,
    /// and if the compute thread never releases it the game still does not stop (only that frame is skipped).
    fn wait_with<R>(&self, ms: u64, f: impl FnOnce(&mut Plugin) -> R) -> Option<R> {
        let start = clock::ticks();
        let mut spins = 0;
        loop {
            if self.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                job::YIELD.store(false, Ordering::Relaxed);
                let r = unsafe { (*self.data.get()).as_mut().map(f) };
                self.lock.store(false, Ordering::Release);
                return r;
            }
            // Ask the compute thread to hand over the lock (the computation returns within 0.5 ms)
            job::YIELD.store(true, Ordering::Relaxed);
            if clock::ms_since(start) >= ms {
                job::YIELD.store(false, Ordering::Relaxed);
                return None;
            }
            backoff(&mut spins);
        }
    }
    fn with<R>(&self, f: impl FnOnce(&mut Plugin) -> R) -> Option<R> {
        let mut spins = 0;
        while self.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            backoff(&mut spins);
        }
        let r = unsafe { (*self.data.get()).as_mut().map(f) };
        self.lock.store(false, Ordering::Release);
        r
    }
}
/// Waiting for the lock: spin briefly, then sleep. The compute thread has a lower priority (24) than the game and draw
/// threads. On Ryujinx a higher-priority thread spinning on the same core never lets it run, so it could not release the
/// lock until the draw gave up after 100 ms: the window and the game stalled about once a second (2026-11-19 flicker).
/// A short sleep hands the core over; yield_now would not (it only yields to threads of the same priority).
fn backoff(spins: &mut u32) {
    if *spins < 64 {
        *spins += 1;
        core::hint::spin_loop();
    } else {
        forge::os::this_thread::sleep_for(core::time::Duration::from_micros(200));
    }
}
/// Highest heap peak seen since the plugin started (forge's peak is reset at each search start)
static HEAP_MAX: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// Log the Rust heap of this plugin (requested bytes; the vendored forge allocator counts them).
/// what: when it is logged. Then the peak starts again from the live size.
fn log_heap(what: &str) {
    let (live, peak, blocks) = forge::heap_stats();
    let max = HEAP_MAX.fetch_max(peak, Ordering::Relaxed).max(peak);
    info!("[skill-sim] heap ({}): peak {} KB, live {} KB in {} blocks, highest since start {} KB", what, peak / 1024, live / 1024, blocks, max / 1024);
    forge::heap_reset_peak();
}
static STATE: Shared = Shared { lock: AtomicBool::new(false), data: UnsafeCell::new(None) };
static BASE: AtomicU32 = AtomicU32::new(0);
/// The game's main module base (0 before start)
#[allow(dead_code)]
pub fn base() -> u32 {
    BASE.load(Ordering::Acquire)
}

struct Plugin {
    app: app::App,
    slot: Option<usize>,
    playing: bool,
    input_ready: bool,
    save_age: u32,
    prefs_dirty: bool,
    /// Buttons received in the game frame. The compute thread takes and handles them.
    pending: alloc::vec::Vec<app::Button>,
    /// Was the window open last frame (open/close sounds)
    was_open: bool,
    /// Keyboard / mouse prompts shown last frame
    kbm: bool,
    /// The inventory summary was logged once
    inv_logged: bool,
    /// Last search whose arguments were logged (start time)
    logged_job: u64,
}

#[forge::entry]
fn main() {
    let base = forge::mem::text_addr();
    BASE.store(base, Ordering::Release);
    if !session::install(base) {
        error!("[skill-sim] MHGU 1.4.0 session signatures rejected; disabled");
        return;
    }
    let Some(db) = db::Db::load() else {
        error!("[skill-sim] simdata parse failed; disabled");
        return;
    };
    let db: &'static db::Db = alloc::boxed::Box::leak(alloc::boxed::Box::new(db));
    let mut app = app::App::new(db);
    if let Ok(text) = storage::read(storage::SETTINGS) {
        for line in text.lines() {
            match line.split_once('=') {
                Some(("language", v)) => app.language = v.parse().unwrap_or(0).min(8),
                Some(("layout", v)) => app.layout = v.parse().unwrap_or(0).min(2),
                // Old keys (ui_scale, ui_pos, search_threads, diag_log, se_*) are ignored: those settings were removed
                // (diag_log was on by default; 'logging' starts off for everyone)
                Some(("logging", v)) => app.logging = v == "1",
                _ => {}
            }
        }
    }
    LOGGING.store(app.logging, Ordering::Relaxed);
    game_menu::install(base);
    sound::install(base);
    let input_ready = input::retry(base);
    unsafe { *STATE.data.get() = Some(Plugin { app, slot: None, playing: false, input_ready, save_age: 0, prefs_dirty: false, pending: alloc::vec::Vec::new(), was_open: false, kbm: false, inv_logged: false, logged_job: 0 }) };
    // Search, cause analysis, extra skills and alternatives run on this thread. When the game calls our per-frame function only about
    // 24 KB of stack is left (confirmed by a Ryujinx crash), so engine work there would overflow. A large stack is given separately.
    // nn::os priority is 0 (high) to 31 (low). Out of range, CreateThread aborts the game (confirmed on Ryujinx).
    let worker = forge::os::thread::Builder::new().name("skill-sim").stack_size(512 * 1024).priority(24).spawn(worker);
    core::mem::forget(worker);
    // No helper compute threads: one search thread is fast enough since the engine speedups (2026-11-19), and three
    // more 512 KB stacks are saved. job::HELPER_THREAD stays false, so searches never split.
    info!("[skill-sim] v{} ready; input={}", env!("CARGO_PKG_VERSION"), input_ready);
    log_heap("start");
}

/// Time per computation round and the rest between rounds (ms). The draw takes the lock during the rest.
/// The old 4 ms compute / 8 ms rest ran only about 1/3 of the time and was especially slow on Recomp. This thread has lower priority
/// than the game (24, game main 16), so it does not hold up the game.
const BUSY_SLICE_MS: u64 = 5;
const BUSY_REST_MS: u64 = 1;
const IDLE_MS: u64 = 8;

/// Compute thread: handle buttons and compute a little, then rest. Rest briefly when there is work.
fn worker() {
    let mut busy = false;
    loop {
        forge::os::this_thread::sleep_for(core::time::Duration::from_millis(if busy { BUSY_REST_MS } else { IDLE_MS }));
        // If the screen or game frame is waiting for the lock, let it go first (at most 3 ms)
        let t = clock::ticks();
        while job::should_yield() && clock::ms_since(t) < 3 {
            core::hint::spin_loop();
        }
        let held = clock::ticks();
        busy = STATE.with(|p| {
            WORKER_STAGE.store(1, Ordering::Relaxed);
            for b in core::mem::take(&mut p.pending) {
                // On the condition screen with no overlay open, B closes the window
                let closable = p.app.screen == app::Screen::Main
                    && p.app.dialog().is_none()
                    && p.app.drop.is_none()
                    && p.app.menu.is_none()
                    && p.app.sub.is_none()
                    && p.app.fixmenu.is_none();
                if b == app::Button::B && closable {
                    p.app.open = false;
                    break;
                }
                press(&mut p.app, b);
            }
            // When a new search starts, log the PC searcher arguments for the same conditions (to reproduce slow conditions)
            if let Some(job) = &p.app.job {
                if p.logged_job != job.started {
                    p.logged_job = job.started;
                    log_heap("before search");
                    if logging() {
                        log_long("[skill-sim] query", &job.args);
                    }
                }
            }
            WORKER_STAGE.store(2, Ordering::Relaxed);
            // The set search is taken out of the lock below: only cause analysis and extra skills here
            if p.app.job.is_none() {
                p.app.tick(if p.app.busy() { BUSY_SLICE_MS } else { 4 });
            }
            if core::mem::take(&mut p.app.cache_hit) {
                info!("[skill-sim] search: same condition as an earlier search, showing its results");
            }
            // Result sound: once when the search ends and the results screen appears (the search ends outside the lock, so use the flag finish_search leaves)
            if core::mem::take(&mut p.app.search_done) {
                sound::cue(if p.app.results.is_empty() { sound::Cue::NoResult } else { sound::Cue::Found });
            }
            if let Some(s) = p.app.last_search.take() {
                info!(
                    "[skill-sim] search {} ({} threads): {} nodes, {} ms ({} ms computing, {} k nodes/s), {} sets, needed={}, charms {}, targets {}, owned armor {}",
                    s.how,
                    job::HELPER_COUNT.load(Ordering::Relaxed) + 1,
                    s.nodes,
                    s.wall_ms,
                    s.busy_ms,
                    s.nodes / s.busy_ms.max(1),
                    s.found,
                    s.needed,
                    s.charms,
                    s.targets,
                    p.app.db.inv().owned.len()
                );
                log_heap("search");
                if s.owned_skipped || s.seeded > 0 {
                    info!("[skill-sim] search reuse: owned phase skipped={}, started with {} earlier sets", s.owned_skipped, s.seeded);
                }
                if s.needed {
                    // Per phase: owned-charm phase / time waited at the question / unowned-charm phase (minus the wait).
                    // Subtracting two u64 with saturating_sub makes the compiler fuse them into one NEON vqsub.u64,
                    // which the Recomp interpreter does not know, and the game stopped (2026-11-18 crash). Use plain subtraction.
                    let used = s.owned_ms + s.ask_wait_ms;
                    let unowned_ms = if s.wall_ms > used { core::hint::black_box(s.wall_ms - used) } else { 0 };
                    let unowned_nodes = if s.nodes > s.owned_nodes { core::hint::black_box(s.nodes - s.owned_nodes) } else { 0 };
                    info!(
                        "[skill-sim] search phases: owned {} ms ({} nodes), asked {} ms, unowned {} ms ({} nodes)",
                        s.owned_ms,
                        s.owned_nodes,
                        s.ask_wait_ms,
                        core::hint::black_box(unowned_ms),
                        unowned_nodes
                    );
                }
            }
            // Alternatives: computed just by resting the cursor on the result list (a little at a time, when nothing else runs)
            if p.app.open && p.app.job.is_none() && p.app.diag_run.is_none() && p.app.diag_out.is_none() && !p.app.extra_busy() {
                WORKER_STAGE.store(3, Ordering::Relaxed);
                p.app.ensure_alts(BUSY_SLICE_MS);
            }
            p.app.open && p.app.busy()
        })
        .unwrap_or(false);
        // Diagnostics: if the compute thread held the shared lock over 30 ms (the draw stalled that long), log which stage it was in
        let held_ms = clock::ms_since(held);
        if held_ms >= 30 {
            warn!("[skill-sim] worker held the lock {} ms (stage {})", held_ms, WORKER_STAGE.load(Ordering::Relaxed));
        }
        // The set search runs outside the lock (including exchanging sets with helpers and converting results at the end):
        // inside the lock, exchanges took hundreds of ms with many results and the window flickered (2026-11-18)
        WORKER_STAGE.store(7, Ordering::Relaxed);
        // While the window is closed (quest, loading, menu closed) searches pause and resume when it opens again
        let suspended = job::SUSPEND.load(Ordering::Relaxed);
        if let Some((db, mut job)) = (!suspended).then(|| STATE.with(|p| p.app.take_job().map(|j| (p.app.db, j))).flatten()).flatten() {
            let was = job.state;
            job.step(BUSY_SLICE_MS);
            let found = (job.state == job::State::Done).then(|| job.result(db));
            STATE.with(|p| p.app.put_job(job, was, found));
            busy = true;
        }
        // Cause analysis and extra skills also compute (including building searchers) outside the lock
        WORKER_STAGE.store(9, Ordering::Relaxed);
        if let Some((db, mut d)) = (!suspended).then(|| STATE.with(|p| p.app.take_diag().map(|d| (p.app.db, d))).flatten()).flatten() {
            let done = d.work(db, BUSY_SLICE_MS);
            STATE.with(|p| p.app.put_diag(d, done));
            busy = true;
        }
        WORKER_STAGE.store(10, Ordering::Relaxed);
        if let Some((db, mut e)) = (!suspended).then(|| STATE.with(|p| p.app.take_extra().map(|e| (p.app.db, e))).flatten()).flatten() {
            let done = e.work(db, BUSY_SLICE_MS);
            if done.is_some() {
                info!("[skill-sim] {}", e.summary());
            }
            STATE.with(|p| p.app.put_extra(e, done));
            busy = true;
        }
        // Alternatives also compute and convert results outside the lock
        WORKER_STAGE.store(8, Ordering::Relaxed);
        if let Some((db, mut run)) = (!suspended).then(|| STATE.with(|p| p.app.take_alt_work().map(|r| (p.app.db, r))).flatten()).flatten() {
            run.work(db, BUSY_SLICE_MS);
            STATE.with(|p| p.app.put_alt_work(run));
            busy = true;
        }
        // Building searchers (heavy) happens outside the lock: the draw is never blocked and the loading window stays visible
        WORKER_STAGE.store(5, Ordering::Relaxed);
        if let Some((generation, req)) = STATE.with(|p| p.app.take_build()).flatten() {
            let job = req.run();
            STATE.with(|p| p.app.finish_build(generation, job));
            busy = true;
        }
        // Alternative searchers are built outside the lock too
        if let Some((db, (key, part, cond))) = STATE.with(|p| p.app.take_alt_build().map(|b| (p.app.db, b))).flatten() {
            let job = app::App::alt_build(db, &cond);
            STATE.with(|p| p.app.put_alt_job(key, part, job));
            busy = true;
        }
        WORKER_STAGE.store(0, Ordering::Relaxed);
    }
}

/// A log line is cut at about 250 characters, so log in pieces (joined they give the original string)
fn log_long(prefix: &str, s: &str) {
    let chars: alloc::vec::Vec<char> = s.chars().collect();
    let parts = chars.len().div_ceil(180).max(1);
    for (i, c) in chars.chunks(180).enumerate() {
        let piece: alloc::string::String = c.iter().collect();
        info!("{} {}/{} {}", prefix, i + 1, parts, piece);
    }
}

/// Handle one button and pick the sound by comparing the state before and after
fn press(app: &mut app::App, b: app::Button) {
    use app::Button::*;
    use sound::Cue;
    let before = app.snapshot();
    app.press(b);
    let after = app.snapshot();
    let cue = if after.message != before.message && after.message != 0 {
        Cue::Error // Notice text (no target skill etc.)
    } else if after == before {
        if matches!(b, A | X | Y | Plus | Minus) { Cue::Error } else { return }
    } else if after.tab != before.tab {
        Cue::Tab
    } else if after.page != before.page {
        // The page of a paged list changed (a different sound from cursor movement)
        Cue::Page
    } else if b == B {
        Cue::Cancel
    } else if matches!(b, Up | Down | Left | Right) {
        Cue::Move
    } else {
        Cue::Confirm
    };
    sound::cue(cue);
}

#[unsafe(no_mangle)]
pub extern "C" fn forge_onUpdate(_dt: f32) {
    let base = BASE.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    game_menu::update();
    let input_ready = input::retry(base);
    let (slot, playing) = session::update();
    let overlay = forge::graphics::Graphics::is_menu_visible();
    // Even while computing, wait briefly (at most 2 ms) so input and opening the window are not delayed: the compute thread hands over soon
    // All save file writes happen on this thread (the game thread), after releasing the shared lock
    let mut writes: alloc::vec::Vec<(&'static core::ffi::CStr, alloc::string::String)> = alloc::vec::Vec::new();
    STATE.wait_with(2, |p| {
        p.input_ready = input_ready;
        if p.slot != slot {
            // When the character changes, read that character's conditions and saved sets
            p.slot = slot;
            p.app.open = false;
            p.app.clear_cache(); // the search memory belongs to the previous character's equipment
            if let Some(s) = slot {
                let text = storage::read(storage::character(s)).unwrap_or_default();
                p.app.decode(&text);
            }
            p.app.dirty = false;
        }
        p.playing = playing;
        if !playing || !input_ready {
            p.app.open = false;
        }
        // Open from the game menu's 'Skill Simulator' entry (or the mod menu host's panel)
        if crate::game_menu::OPEN_REQUEST.swap(false, Ordering::AcqRel) && playing && input_ready {
            p.app.open = true;
        }
        // Inventory (equipment box, charms, equipped, items): re-read every time the window opens (it cannot change in game while the window is open)
        if p.app.open && !p.was_open && playing {
            match inventory::read(base) {
                Some(inv) => {
                    if !p.inv_logged {
                        info!(
                            "[skill-sim] inventory: armor {}, charms {}, equipped {:?} charm {:?} decorations {:?}, items {}",
                            inv.owned.len(),
                            inv.charms.len(),
                            inv.equipped_armor,
                            inv.equipped_charm,
                            inv.equipped_jewels,
                            inv.items.len()
                        );
                        p.inv_logged = true;
                    }
                    // Every open: compare these counts with the smithy's lists in game
                    match &inv.smithy {
                        Some(s) => info!(
                            "[skill-sim] smithy list: armor {} / {}, decorations {} / {} (recipes {} / {})",
                            s.armor.len(),
                            p.app.db.recipe_armor_count(),
                            s.jewels.len(),
                            p.app.db.gd.jewels.len(),
                            s.deco_recipes,
                            crate::generated::unlock::DECO_KEYS.len()
                        ),
                        None => error!("[skill-sim] smithy list unavailable: 'smithy only' searches are refused"),
                    }
                    p.app.apply_inventory(inv);
                }
                None => error!("[skill-sim] inventory unavailable"),
            }
        }
        if p.app.open != p.was_open {
            p.app.touch();
            if playing {
                sound::cue(if p.app.open { sound::Cue::Open } else { sound::Cue::Close });
            }
            if !p.app.open {
                log_heap("window open"); // everything while the window was open (alternatives, extra skills, ...)
            }
            p.was_open = p.app.open;
        }
        game_menu::set_plugin_language(p.app.language);
        LOGGING.store(p.app.logging, Ordering::Relaxed);
        input::OWNED.store(p.app.open, Ordering::Release);
        // Recomp: our own free mouse pointer while the window is open (clicks are ours only)
        input::set_mouse_free(p.app.open && !overlay);
        job::SUSPEND.store(!p.app.open, Ordering::Relaxed);
        if p.app.open && !overlay && !ext_block::guest_modal() {
            p.pending.extend(input::take());
        } else {
            input::take();
        }
        // Button prompts switch between controller and keyboard / mouse pictures: redraw on a change
        let kbm = prompts::kbm();
        if kbm != p.kbm {
            p.kbm = kbm;
            p.app.touch();
        }
        // Changes are gathered briefly and then saved (postponed during a search)
        p.save_age = p.save_age.saturating_add(1);
        // Save when the window closes, or after 2 s while open (save writes are expensive, so not often)
        if p.app.dirty && p.app.job.is_none() && p.app.job_out.is_none() && (!p.app.open || p.save_age > 120) {
            if let Some(s) = p.slot {
                writes.push((storage::character(s), p.app.encode()));
            }
            p.app.dirty = false;
            p.save_age = 0;
        }

        if p.prefs_dirty {
            let text = alloc::format!("language={}\nlayout={}\nlogging={}\n", p.app.language, p.app.layout, p.app.logging as u8);
            writes.push((storage::SETTINGS, text));
            p.prefs_dirty = false;
        }
    });
    for (path, text) in writes {
        if storage::write(path, &text).is_err() {
            error!("[skill-sim] save failed: {:?} ({} bytes)", path, text.len());
        }
    }
    // Sounds are played outside the lock (calls a game function)
    sound::update();
}

#[unsafe(no_mangle)]
pub extern "C" fn forge_onImGuiInit(_ctx: *mut c_void, _alloc: *mut c_void, _free: *mut c_void) {
    ui::init();
}

/// What the compute thread is doing now (for the skipped-frame log)
static WORKER_STAGE: AtomicU32 = AtomicU32::new(0);

/// Measurement (log): every 5 s while the window is open, average lock wait and draw time
struct FrameStat {
    frames: core::sync::atomic::AtomicU32,
    wait: core::sync::atomic::AtomicU64,
    draw: core::sync::atomic::AtomicU64,
    start: core::sync::atomic::AtomicU64,
}
impl FrameStat {
    fn with(&self, wait: u64, draw: u64, searching: bool) {
        use core::sync::atomic::Ordering::Relaxed;
        if self.frames.fetch_add(1, Relaxed) == 0 {
            self.start.store(clock::ticks(), Relaxed);
        }
        self.wait.fetch_add(wait, Relaxed);
        self.draw.fetch_add(draw, Relaxed);
        let start = self.start.load(Relaxed);
        if clock::ms_since(start) >= 5000 {
            let n = self.frames.swap(0, Relaxed).max(1) as u64;
            let us = |t: u64| t * 1000 / clock::PER_MS / n;
            info!(
                "[skill-sim] frame: {} frames in 5s, lock wait avg {} us, draw avg {} us, searching={}",
                n,
                us(self.wait.swap(0, Relaxed)),
                us(self.draw.swap(0, Relaxed)),
                searching
            );
        }
    }
}
static FRAME_STAT: FrameStat = FrameStat {
    frames: core::sync::atomic::AtomicU32::new(0),
    wait: core::sync::atomic::AtomicU64::new(0),
    draw: core::sync::atomic::AtomicU64::new(0),
    start: core::sync::atomic::AtomicU64::new(0),
};

#[unsafe(no_mangle)]
pub extern "C" fn forge_onImGuiFreeRender() {
    if !ui::ready() {
        return;
    }
    prompts::refresh();
    let t0 = clock::ticks();
    let mut drawn = 0u64;
    let mut window_drawn = false;
    let r = STATE.wait_with(100, |p| {
        if p.app.open {
            window_drawn = true;
            let t1 = clock::ticks();
            if !forge::graphics::Graphics::is_menu_visible() {
                ui::mouse_shield();
            }
            // Mouse clicks / wheel on our window become button presses (handled like the controller)
            let presses = ui::render(&mut p.app);
            // A guest mod's dialog (Wishlist) over our window takes the input
            if !ext_block::guest_modal() {
                p.pending.extend(presses);
            }
            // Tell other mods (Better-Skillinfo, Wishlist) where our window is this frame
            window_view::publish(ui::shared_view());
            ext_block::publish(ui::shared_ext());
            drawn = clock::ticks() - t1;
            if logging() {
                FRAME_STAT.with(t1 - t0, drawn, p.app.job.is_some() || p.app.job_out.is_some());
            }
        }
    });
    if r.is_some() && !window_drawn {
        window_view::publish(None); // window closed this frame
        ext_block::publish(None);
    }
    if r.is_none() {
        // The lock was not obtained within 100 ms, so this frame did not draw the window (flicker): what was the compute thread doing
        // 0 idle, 1 buttons, 2 cause analysis/extra skills, 3 alternatives prep, 5 around building searchers outside the lock, 6 saving search memory,
        // 7 take/put of the set search (computed outside the lock), 8 take/put of alternatives, 9 cause analysis, 10 extra skills
        warn!("[skill-sim] frame skipped: lock busy over 100 ms, worker stage {}", WORKER_STAGE.load(Ordering::Relaxed));
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn forge_onImGuiRender() {
    // Our section in the Forge settings window: skipping it on frames where the lock was not free made it vanish and reappear, changing the content height,
    // and ImGui pulled the scroll back up (2026-11-19). The compute thread sees YIELD and releases soon, so wait briefly.
    STATE.wait_with(5, |p| {
        let ready = p.playing && p.input_ready;
        if ui::menu(&mut p.app, ready) {
            p.prefs_dirty = true;
            p.app.touch();
        }
    });
}
