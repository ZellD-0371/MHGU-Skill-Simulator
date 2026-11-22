//! PC tests of the plugin logic. Takes the plugin sources as is (#[path]), compares results with the preview engine (query.exe),
//! and runs the screen flows (search -> results, continue, no-result cause analysis, extra skills, alternatives, save format).
//!   cargo run --release            (first run cargo build --release --examples in engine)
extern crate alloc;

#[path = "../../plugin/src/app.rs"]
mod app;
#[path = "../../plugin/src/db.rs"]
mod db;
#[path = "../../plugin/src/job.rs"]
mod job;
#[path = "../../plugin/src/set.rs"]
mod set;
#[path = "../../plugin/src/prompts.rs"]
#[allow(dead_code)]
mod prompts;
mod generated {
    #[path = "../../../plugin/src/generated/data.rs"]
    pub mod data;
    #[path = "../../../plugin/src/generated/strings.rs"]
    pub mod strings;
    #[path = "../../../plugin/src/generated/unlock.rs"]
    pub mod unlock;
}
use generated::strings as k;
/// PC clock (the plugin uses the system tick)
mod clock {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    pub fn ticks() -> u64 {
        START.get_or_init(Instant::now).elapsed().as_micros() as u64 * 192 / 10
    }
    pub const PER_MS: u64 = 19_200;
    pub fn ms_since(start: u64) -> u64 {
        ticks().saturating_sub(start) / PER_MS
    }
}

static SIMDATA: &[u8] = include_bytes!("../../data/simdata.bin");

use app::{App, Button, Dialog, Screen};
use skillsim_engine::types::*;
use std::process::Command;

fn query(args: &[String]) -> Vec<(u8, u16, Vec<u16>)> {
    let exe = concat!(env!("CARGO_MANIFEST_DIR"), "/../engine/target/release/examples/query.exe");
    let out = Command::new(exe).args(args).output().expect("query.exe (engine 에서 cargo build --release --examples)");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with("SET "))
        .map(|l| {
            let f: Vec<&str> = l.split(' ').collect();
            let heads = f[5].split(',').map(|p| p.split(';').next().unwrap().parse().unwrap()).collect();
            (f[1].parse().unwrap(), f[3].parse().unwrap(), heads)
        })
        .collect()
}

fn skill_id(db: &db::Db, tree: u16, pt: i8) -> u16 {
    db.gd.skills.iter().find(|s| s.tree == tree && s.threshold == pt).unwrap().id
}

fn run_to_end(a: &mut App) {
    let mut guard = 0;
    while a.job.is_some() || a.building.is_some() || a.diag_run.is_some() || a.extra_busy() {
        // Same path as the plugin's compute thread: the set search is taken out (outside the lock), run and put back
        if let Some(mut j) = a.take_job() {
            let was = j.state;
            j.step(1000);
            let found = (j.state == job::State::Done).then(|| j.result(a.db));
            a.put_job(j, was, found);
        } else {
            a.tick(1000);
        }
        // Emulate the helper threads (they really run separately)
        for i in 0..job::MAX_HELPERS {
            job::helper_run(i, 5);
        }
        if a.job.as_ref().is_some_and(|j| j.state == job::State::AskNeeded && !j.ask_ready()) {
            std::thread::sleep(std::time::Duration::from_millis(5)); // Show the bar at 100% briefly, then the question
        }
        if a.job.as_ref().is_some_and(|j| j.ask_ready()) {
            a.press(Button::A); // Search with unowned charms: yes (default cursor)
        }
        guard += 1;
        if guard > 100_000 {
            panic!("검색이 끝나지 않는다");
        }
        if let Some(j) = &a.job {
            if j.state == job::State::Paused {
                a.press(Button::B); // Show results so far (cursor 0 = continue, B = show results)
            }
        }
    }
}

/// Reproduction: with SKILLSIM_REPRO=<file with the query arguments from the game log joined>, run the plugin search (Job) with that condition
/// and print the node count (to compare with query.exe on the same condition).
fn repro(db: &'static db::Db, path: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    let get = |k: &str| text.split(' ').find_map(|a| a.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
    let charms: Vec<Charm> = get("charms")
        .split(';')
        .filter_map(|c| {
            let v: Vec<i32> = c.split(':').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 5).then(|| Charm { skills: [Pt { tree: v[0] as u16, pt: v[1] as i8 }, Pt { tree: v[2] as u16, pt: v[3] as i8 }], slots: v[4] as u8 })
        })
        .collect();
    let owned = get("owned").split(',').filter_map(|x| x.parse().ok()).collect();
    db.set_inv(db::Inv { owned, charms, loaded: true, ..Default::default() });
    let mut a = App::new(db);
    a.inline_build = true;
    a.targets = get("targets").split(',').filter_map(|t| t.split_once(':')).map(|(t, p)| skill_id(db, t.parse().unwrap(), p.parse().unwrap())).collect();
    a.kind = get("kind").parse().unwrap();
    a.weapon = get("weapon").parse().unwrap();
    a.forbid = get("forbid") == "1";
    a.priority = get("priority").parse().unwrap_or(0);
    let t0 = std::time::Instant::now();
    a.run_search();
    // Emulate two threads alternating every 5 ms (really simultaneous)
    while a.job.is_some() || a.building.is_some() {
        a.tick(5);
        for i in 0..job::MAX_HELPERS {
            job::helper_run(i, 5);
        }
        if a.job.as_ref().is_some_and(|j| j.state == job::State::AskNeeded && !j.ask_ready()) {
            std::thread::sleep(std::time::Duration::from_millis(5)); // Show the bar at 100% briefly, then the question
        }
        if a.job.as_ref().is_some_and(|j| j.ask_ready()) {
            a.press(Button::A);
        }
    }
    let s = a.last_search.take().unwrap();
    println!("plugin Job: {} nodes, {} sets, needed={}, charms {}, {} ms", s.nodes, s.found, s.needed, s.charms, t0.elapsed().as_millis());
}

/// Extra skills on the first result sets of a few conditions: time, skills found (owned charm, then any charm).
/// Prints one line per run so two versions can be compared.
/// Time between engine trace marks (where Searcher::new spends its time): SKILLSIM_TRACE=1 with the extra bench
static TRACE_LAST: std::sync::Mutex<(Option<std::time::Instant>, u32, Vec<(u32, u32, u128)>)> = std::sync::Mutex::new((None, 0, Vec::new()));
fn trace_mark(n: u32) {
    let mut t = TRACE_LAST.lock().unwrap();
    let now = std::time::Instant::now();
    if let Some(prev) = t.0 {
        let from = t.1;
        let d = now.duration_since(prev).as_nanos();
        match t.2.iter_mut().find(|e| e.0 == from && e.1 == n) {
            Some(e) => e.2 += d,
            None => t.2.push((from, n, d)),
        }
    }
    t.0 = if n == 12 { None } else { Some(now) };
    t.1 = n;
}

fn extra_bench(db: &'static db::Db) {
    if std::env::var("SKILLSIM_TRACE").is_ok() {
        skillsim_engine::trace::set(trace_mark);
    }
    let cases: &[(&[(u16, i8)], usize, [bool; 3])] = &[
        (&[(72, 10), (35, 10), (36, 10)], 0, [false, true, true]),
        (&[(19, 20), (71, 10)], 1, [true, true, true]),
        (&[(19, 20)], 3, [true, true, true]),
        (&[(72, 10), (6, 15), (12, 10)], 2, [false, true, true]),
    ];
    for (n, (targets, weapon, ranks)) in cases.iter().enumerate() {
        for pick in 0..3 {
            let mut a = App::new(db);
            // (each set gets a fresh App, so the session cache starts empty)
            a.inline_build = true;
            a.targets = targets.iter().map(|&(t, p)| skill_id(db, t, p)).collect();
            a.weapon = *weapon;
            a.ranks = *ranks;
            a.run_search();
            run_to_end(&mut a);
            if a.results.len() <= pick {
                continue;
            }
            a.ridx = pick;
            for any in [false, true] {
                a.extra = None;
                a.extra_summary = None;
                let t0 = std::time::Instant::now();
                if !any {
                    a.press(Button::X);
                    while a.extra_busy() {
                        a.tick(1000);
                    }
                } else {
                    // The unowned-charm pass (what the 'yes' of the question starts)
                    if a.confirm.is_none() {
                        a.confirm = None;
                        a.press(Button::X);
                        while a.extra_busy() {
                            a.tick(1000);
                        }
                        if a.confirm.is_none() {
                            continue; // owned charm already found skills: no unowned pass
                        }
                    }
                    a.press(Button::A);
                    while a.extra_busy() {
                        a.tick(1000);
                    }
                }
                let mut found: Vec<u16> = a.extra.clone().unwrap_or_default();
                found.sort_unstable();
                println!(
                    "case {} set {} any={} : {} ms, {} | found {:?}",
                    n,
                    pick,
                    any,
                    t0.elapsed().as_millis(),
                    a.extra_summary.clone().unwrap_or_default(),
                    found
                );
                a.confirm = None;
            }
            // Same set again in this session: from the session cache (no search, same list)
            let before = a.extra.clone();
            a.extra = None;
            a.confirm = None;
            let t0 = std::time::Instant::now();
            a.press(Button::X);
            let busy = a.extra_busy();
            while a.extra_busy() {
                a.tick(1000);
            }
            if a.confirm.is_some() {
                a.press(Button::A);
                while a.extra_busy() {
                    a.tick(1000);
                }
            }
            println!("case {} set {} again: {} us, searched={}, same={}", n, pick, t0.elapsed().as_micros(), busy, a.extra == before);
        }
    }
    let mut t = TRACE_LAST.lock().unwrap();
    t.2.sort_by(|a, b| b.2.cmp(&a.2));
    for (from, to, ns) in t.2.iter().take(12) {
        println!("trace {:>3} -> {:>3}: {} ms", from, to, ns / 1_000_000);
    }
}

fn main() {
    let db: &'static db::Db = Box::leak(Box::new(db::Db::load().expect("simdata")));
    // Test the two-thread parallel search path (alone with SKILLSIM_SINGLE=1)
    job::HELPER_THREAD.store(std::env::var("SKILLSIM_SINGLE").is_err(), std::sync::atomic::Ordering::Release);
    // Search thread count (SKILLSIM_THREADS, default 2): helpers = count - 1
    let threads: u32 = std::env::var("SKILLSIM_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
    job::HELPER_COUNT.store(threads.clamp(1, 4) - 1, std::sync::atomic::Ordering::Release);
    // Measure alternative computation time: SKILLSIM_ALTBENCH=1
    if std::env::var("SKILLSIM_ALTBENCH").is_ok() {
        alt_bench(db);
        return;
    }
    // Measure the extra-skill search: SKILLSIM_EXTRABENCH=1
    if std::env::var("SKILLSIM_EXTRABENCH").is_ok() {
        extra_bench(db);
        return;
    }
    if let Ok(path) = std::env::var("SKILLSIM_REPRO") {
        return repro(db, &path);
    }
    let mut fails = 0;

    // 1. Plugin search results == preview engine results (same conditions)
    let cases: &[(&[(u16, i8)], u8, u8, [bool; 3], bool, u8, usize)] = &[
        (&[(19, 20)], 1, 0, [true, true, true], true, 0, 0),
        (&[(19, 20), (71, 10)], 1, 1, [true, true, false], true, 0, 0),
        (&[(35, 10), (36, 10)], 1, 1, [true, false, false], true, 0, 0),
        (&[(19, 20), (71, 20), (35, 10)], 1, 1, [true, true, false], true, 0, 3),
        // Priority 1 (defense) was removed from the plugin: rarity priority here
        (&[(46, 10), (19, 15)], 2, 2, [true, true, true], false, 2, 0),
        (&[(19, 20), (71, 10)], 1, 3, [true, true, true], true, 2, 0),
    ];
    for (n, (targets, kind, weapon, ranks, forbid, priority, mindef)) in cases.iter().enumerate() {
        let mut a = App::new(db);
    a.inline_build = true;
        a.targets = targets.iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        a.kind = *kind;
        a.weapon = *weapon as usize;
        a.ranks = *ranks;
        a.forbid = *forbid;
        a.priority = *priority as usize;
        a.mindef = *mindef;
        let t0 = std::time::Instant::now();
        a.run_search();
        run_to_end(&mut a);
        let ms = t0.elapsed().as_millis();
        let mut mask = 0u32;
        for (on, (lo, hi)) in ranks.iter().zip(job::RANK_RARES) {
            if *on {
                for r in lo..=hi {
                    mask |= 1 << r;
                }
            }
        }
        mask |= 1 << 11; // rarity 11 = deviant armor: let through, filtered below
        let jewelrank = ranks.iter().rposition(|&r| r).unwrap() + 1;
        // Deviant armor is off by default and nothing is owned here, so the plugin leaves every deviant piece out
        let deviant = db.gd.armor.iter().filter(|a| a.deviant_rank() > 0).map(|a| a.id);
        let excluded: Vec<String> = db.reward_armor.iter().copied().chain([db::PLACEHOLDER_ARMOR]).chain(deviant).map(|x| x.to_string()).collect();
        let args: Vec<String> = vec![
            "thorough=1".into(), // the plugin always searches thoroughly now
            format!("kind={kind}"),
            "sex=1".into(),
            format!("weapon={weapon}"),
            format!("forbid={}", *forbid as u8),
            format!("rare={mask}"),
            format!("jewelrank={jewelrank}"),
            format!("priority={priority}"),
            "exflags=3".into(),
            format!("mindef={}", app::MIN_DEF[*mindef]),
            "minres=-,-,-,-,-".into(),
            "needed=1".into(),
            format!("targets={}", targets.iter().map(|(t, p)| format!("{t}:{p}")).collect::<Vec<_>>().join(",")),
            "fixed=0,0,0,0,0".into(),
            // Reward armor (no smithy recipe) and deviant armor are left out by the plugin unless owned or allowed: same for the engine here
            format!("excluded={}", excluded.join(",")),
            "charms=-".into(),
            "max=30".into(),
            "owned=-".into(),
        ];
        let want = query(&args);
        let got: Vec<(u8, u16, Vec<u16>)> = a.results.iter().map(|r| (r.free, r.def, (0..PARTS).map(|p| r.head(p)).collect())).collect();
        let same = want == got;
        println!(
            "검색 {n}: 플러그인 {}개 (최선 {:?}) / 미리보기 {}개 (최선 {:?}) {} [{} ms, 화면 {}]",
            got.len(),
            got.first().map(|g| g.0),
            want.len(),
            want.first().map(|g| g.0),
            if same { "일치" } else { "불일치" },
            ms,
            match a.screen {
                Screen::Results => "결과",
                _ => "기타",
            }
        );
        if !same {
            fails += 1;
            for (i, (g, w)) in got.iter().zip(want.iter()).enumerate().take(5) {
                if g != w {
                    println!("   {i}: 플러그인 {:?} / 미리보기 {:?}", g, w);
                }
            }
        }
    }

    // 2. No results -> cause-analysis suggestions appear, and picking one searches again with results
    {
        let mut a = App::new(db);
    a.inline_build = true;
        a.targets = [(19, 20), (71, 20), (35, 10), (83, 15)].iter().filter_map(|&(t, p)| db.gd.skills.iter().find(|s| s.tree == t && s.threshold == p).map(|s| s.id)).collect();
        a.ranks = [true, true, false];
        a.mindef = 3;
        a.run_search();
        run_to_end(&mut a);
        let d = a.dialog().map(|d| d.0);
        println!("원인 안내: 결과 {}개, 창 {:?}, 제안 {}개 {:?}", a.results.len(), d.map(|d| d == Dialog::Diag), a.diag.len(),
            a.diag.iter().map(|(l, _)| match l { app::DiagLabel::Lower(x, y) => format!("낮추기 {x}->{y}"), app::DiagLabel::Remove(x) => format!("빼기 {x}"), app::DiagLabel::Forbid => "금지끄기".into(), app::DiagLabel::MinDef => "방어력".into(), _ => "기타".into() }).collect::<Vec<_>>());
        if a.diag.is_empty() {
            fails += 1;
        }
        if a.results.is_empty() && !a.diag.is_empty() {
            a.press(Button::A);
            run_to_end(&mut a);
            println!("   제안 적용 후 결과 {}개", a.results.len());
            if a.results.is_empty() {
                fails += 1;
            }
        }
    }

    // 3. Extra skills, alternatives, save/load
    {
        let mut a = App::new(db);
    a.inline_build = true;
        a.targets = vec![skill_id(db, 19, 20)];
        a.weapon = 3;
        a.run_search();
        run_to_end(&mut a);
        a.press(Button::X); // Extra skills
        run_to_end(&mut a);
        let extra = a.extra.as_ref().map_or(0, |e| e.len());
        println!("추가 스킬: {}개 (확인 창 {:?})", extra, a.confirm.is_some());
        if extra == 0 && a.confirm.is_none() {
            fails += 1;
        }
        a.extra = None;
        a.press(Button::A); // View parts
        let alts = a.part_alts(0).map_or(0, |v| v.len());
        println!("대체 부위(머리): {}개", alts);
        // Prefetched counts (other sets, without lists) must equal the counts from building the lists
        {
            let saved = a.ridx;
            for _ in 0..200 {
                a.ensure_alts(u64::MAX); // One set per call: counts are prefetched from near the cursor
            }
            let mut same = true;
            for i in 1..6.min(a.rows().len()) {
                a.ridx = i;
                let counted: Vec<usize> = (0..5).map(|p| a.alts_len(p)).collect();
                let listed: Vec<usize> = (0..5).map(|p| a.part_alts(p).map_or(0, |v| v.len())).collect();
                // Worthwhile counts (tags) must equal the non-dimmed popup entries other than the current armor
                let useful: Vec<usize> = (0..5).map(|p| a.alts_useful(p)).collect();
                let shown: Vec<usize> = (0..5)
                    .map(|p| {
                        let head = a.current().unwrap().head(p);
                        let v = a.part_alts(p).unwrap_or_default();
                        v[..a.alt_worse_from.get().min(v.len())].iter().filter(|x| x.0 != head).count()
                    })
                    .collect();
                if useful != shown {
                    println!("  쓸 만한 대체 {:?} / 팝업 {:?} (세트 {})", useful, shown, i);
                    same = false;
                }
                if counted != listed {
                    println!("  미리 센 개수 {:?} / 목록 {:?} (세트 {})", counted, listed, i);
                    same = false;
                }
            }
            println!("대체 부위 미리 세기: 개수 일치 {}", same);
            if !same {
                fails += 1;
            }
            a.ridx = saved;
        }
        a.press(Button::B);
        a.press(Button::Y); // Save confirmation
        a.press(Button::A); // Yes
        let text = a.encode();
        let mut b = App::new(db);
        b.decode(&text);
        let same = b.saved.len() == 1 && b.saved[0].same_gear(&a.saved[0], db) && b.targets == a.targets;
        println!("저장 형식: 저장한 세트 {}개, 다시 읽기 {}", a.saved.len(), if same { "일치" } else { "불일치" });
        if !same {
            fails += 1;
        }
    }

    // 7. Reusing earlier searches: (1) 'nothing with owned charms' memory -> stricter conditions skip the owned-charm phase (2) start from earlier results
    {
        let run = |a: &mut App| {
            a.run_search();
            run_to_end(a);
            let mut v: Vec<(u8, u16)> = a.results.iter().map(|r| (r.free, r.def)).collect();
            v.sort();
            (v, a.last_search.take().unwrap())
        };
        let six: Vec<u16> = [(19, 20), (72, 10), (35, 10), (36, 10), (6, 10), (71, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        let seven: Vec<u16> = six.iter().copied().chain(core::iter::once(skill_id(db, 46, 10))).collect();
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = six.clone();
        let (_, s1) = run(&mut a);
        a.targets = seven.clone();
        let (got, s2) = run(&mut a);
        let mut fresh = App::new(db);
        fresh.inline_build = true;
        fresh.targets = seven.clone();
        let (want, _) = run(&mut fresh);
        println!("이전 검색 활용(1): 6스킬 보유 단계 건너뜀={} → 7스킬 건너뜀={}, 결과 {}개(새로 찾기 {}개)", s1.owned_skipped, s2.owned_skipped, got.len(), want.len());
        if !s2.owned_skipped || got != want || s1.owned_skipped {
            println!("  실패: 보유 호석 안 됨 기억");
            fails += 1;
        }
        // (2) A condition with results -> turn off forbid negatives (looser): starting from earlier results must give the same ranking
        let two: Vec<u16> = [(19, 20), (71, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = two.clone();
        let _ = run(&mut a);
        a.forbid = false;
        let (got, s) = run(&mut a);
        let mut fresh = App::new(db);
        fresh.inline_build = true;
        fresh.targets = two.clone();
        fresh.forbid = false;
        let (want, f) = run(&mut fresh);
        println!(
            "이전 검색 활용(2): 이전 결과 {}개로 시작, 노드 {} (새로 찾기 {}), 대표 {}개, 묶인 세트까지 {}개(새로 찾기 {}개) 같음={}",
            s.seeded, s.nodes, f.nodes, got.len(), s.flat.len(), f.flat.len(), s.flat == f.flat
        );
        if got != want {
            let top = |v: &Vec<(u8, u16)>| v.iter().rev().take(12).copied().collect::<Vec<_>>();
            println!("  다름: 이전 결과로 시작 상위 {:?}", top(&got));
            println!("        새로 찾기          상위 {:?}", top(&want));
            println!("        최고 남는 슬롯 {:?} / {:?}, 남는 슬롯별 개수 같음={}", got.iter().map(|x| x.0).max(), want.iter().map(|x| x.0).max(),
                (0..20u8).all(|f| got.iter().filter(|x| x.0 == f).count() == want.iter().filter(|x| x.0 == f).count()));
        }
        {
            let missing: Vec<_> = f.flat.iter().filter(|x| !s.flat.contains(x)).copied().collect();
            let extra: Vec<_> = s.flat.iter().filter(|x| !f.flat.contains(x)).copied().collect();
            println!("  새로 찾기에만 있음(상위) {:?}", missing.iter().rev().take(10).collect::<Vec<_>>());
            println!("  이전 결과 시작에만 있음(상위) {:?}", extra.iter().rev().take(10).collect::<Vec<_>>());
        }
        // The set shown as representative can depend on grouping order: everything including grouped sets must match
        let _ = (&got, &want);
        // Within the result limit (100 representatives) the lower sets depend on search order (same as split work).
        // The best free slots and every set (including grouped) with those free slots must match.
        let best = |v: &Vec<(u8, u16)>| v.iter().map(|x| x.0).max();
        let tier = |v: &Vec<(u8, u16)>| v.iter().filter(|x| Some(x.0) == best(v)).copied().collect::<Vec<_>>();
        println!("  최고 남는 슬롯 {:?}/{:?}, 그 세트 {}개/{}개 같음={}", best(&s.flat), best(&f.flat), tier(&s.flat).len(), tier(&f.flat).len(), tier(&s.flat) == tier(&f.flat));
        if s.seeded == 0 || best(&s.flat) != best(&f.flat) || tier(&s.flat) != tier(&f.flat) {
            println!("  실패: 이전 결과로 시작");
            fails += 1;
        }
    }
    // 8. Exactly the same conditions show remembered results at once (including unowned-charm results)
    {
        let six: Vec<u16> = [(19, 20), (72, 10), (35, 10), (36, 10), (6, 10), (71, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = six.clone();
        a.run_search();
        run_to_end(&mut a);
        let first: Vec<(u8, u16)> = a.results.iter().map(|r| (r.free, r.def)).collect();
        let needed = a.needed_mode;
        // Saved sets keep no charm chance: reloading works it out from the needed charm and must match the search
        // (the search result is chance x 1e15 truncated to an integer, hence the small tolerance)
        for r in a.results.iter().filter(|r| r.needed) {
            let mut saved = r.clone();
            saved.saved_charm = saved.charm_data(db);
            let mut b = App::new(db);
            b.saved.push(saved);
            let text = b.encode();
            b.decode(&text);
            let got = b.saved.first().map_or(-1.0, |x| x.chance);
            if (got - r.chance).abs() > r.chance * 1e-5 + 2e-15 {
                println!("  실패: 저장 세트 확률 {} != {}", got, r.chance);
                fails += 1;
            }
        }
        a.run_search();
        let hit = core::mem::take(&mut a.cache_hit);
        let again: Vec<(u8, u16)> = a.results.iter().map(|r| (r.free, r.def)).collect();
        println!("같은 조건 다시: 기억 사용={hit}, 미보유 호석 결과={needed}/{}, 결과 {}개 같음={}", a.needed_mode, again.len(), first == again);
        if !hit || first != again || needed != a.needed_mode || a.job.is_some() || a.building.is_some() {
            println!("  실패: 같은 조건 결과 기억");
            fails += 1;
        }
        // Not used if any condition differs
        a.forbid = !a.forbid;
        a.run_search();
        let hit = core::mem::take(&mut a.cache_hit);
        run_to_end(&mut a);
        if hit {
            println!("  실패: 다른 조건인데 기억한 결과를 썼다");
            fails += 1;
        }
    }
    // 9. A new charm: if it is worse than every existing charm (same charm signature) use remembered results, otherwise search again
    {
        let ch = |a: (u16, i8), b: (u16, i8), slots: u8| Charm { skills: [Pt { tree: a.0, pt: a.1 }, Pt { tree: b.0, pt: b.1 }], slots };
        let base = vec![ch((19, 4), (0, 0), 1), ch((71, 3), (0, 0), 2)];
        let set_charms = |charms: Vec<Charm>| db.set_inv(db::Inv { charms, loaded: true, ..Default::default() });
        set_charms(base.clone());
        let two: Vec<u16> = [(19, 20), (71, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = two.clone();
        a.run_search();
        run_to_end(&mut a);
        let first: Vec<(u8, u16)> = a.results.iter().map(|r| (r.free, r.def)).collect();
        // Got a useless charm (+1 in a tree unrelated to the targets, 0 slots)
        let mut more = base.clone();
        more.push(ch((2, 1), (0, 0), 0));
        set_charms(more);
        a.run_search();
        run_to_end(&mut a);
        let hit1 = core::mem::take(&mut a.cache_hit);
        let again: Vec<(u8, u16)> = a.results.iter().map(|r| (r.free, r.def)).collect();
        // Got a good charm (targets +7/+5, 3 slots)
        let mut better = base.clone();
        better.push(ch((19, 7), (71, 5), 3));
        set_charms(better);
        a.run_search();
        run_to_end(&mut a);
        let hit2 = core::mem::take(&mut a.cache_hit);
        let best_after = a.results.iter().map(|r| r.free).max();
        println!(
            "호석 얻음: 쓸모없는 호석 → 기억 사용={hit1}(결과 같음={}), 좋은 호석 → 기억 사용={hit2}, 최고 남는 슬롯 {:?} → {:?}",
            first == again,
            first.iter().map(|x| x.0).max(),
            best_after
        );
        if !hit1 || first != again || hit2 || best_after <= first.iter().map(|x| x.0).max() {
            println!("  실패: 호석 서명으로 기억 판정");
            fails += 1;
        }
        db.set_inv(db::Inv::default());
    }
    // 10. Search memory: the same condition again is shown from memory; another character (clear_cache) searches anew
    {
        let ch = |a: (u16, i8), b: (u16, i8), slots: u8| Charm { skills: [Pt { tree: a.0, pt: a.1 }, Pt { tree: b.0, pt: b.1 }], slots };
        db.set_inv(db::Inv { charms: vec![ch((19, 4), (0, 0), 1), ch((71, 3), (0, 0), 2)], loaded: true, ..Default::default() });
        let two: Vec<u16> = [(19, 20), (71, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = two.clone();
        a.run_search();
        run_to_end(&mut a);
        let want = a.results.iter().map(|r| (r.free, r.def, r.charm, r.needed)).collect::<Vec<_>>();
        a.cache_hit = false;
        a.run_search();
        let hit = core::mem::take(&mut a.cache_hit);
        run_to_end(&mut a);
        let same = a.results.iter().map(|r| (r.free, r.def, r.charm, r.needed)).collect::<Vec<_>>() == want;
        a.clear_cache();
        a.run_search();
        let stale = core::mem::take(&mut a.cache_hit);
        run_to_end(&mut a);
        println!("검색 기억: 같은 조건 기억 사용={hit}, 결과 같음={same}, 캐릭터 바뀐 뒤 기억 안 씀={}", !stale);
        if !hit || !same || stale {
            println!("  실패: 검색 기억");
            fails += 1;
        }
        db.set_inv(db::Inv::default());
    }
    // 11. Key pictures: SDL3 joined names and older spaced names find the same picture; side buttons and wheel have one
    {
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/ui");
        let mut ok = true;
        for (key, want) in [
            ("PageUp", "kbm-page-up"),
            ("Page Up", "kbm-page-up"),
            ("PageDown", "kbm-page-down"),
            ("CapsLock", "kbm-caps-lock"),
            ("Left Shift", "kbm-left-shift"),
            ("Mouse 4", "kbm-mouse-4"),
            ("Mouse 5", "kbm-mouse-5"),
            ("Wheel Up", "kbm-wheel-up"),
            ("Wheel Down", "kbm-wheel-down"),
            ("Keypad +", "kbm-keypad-plus"),
            ("-", "kbm-minus"),
            ("F12", "kbm-f12"),
        ] {
            let got = prompts::key_sprite(key);
            let exists = ui.join(format!("{got}.png")).exists();
            if got != want || !exists {
                println!("  실패: 키 그림 {key:?} -> {got} (그림 있음={exists})");
                ok = false;
            }
        }
        for (key, want) in [("PageDown", "PGD"), ("Mouse 5", "M5"), ("Wheel Up", "W↑"), ("PrintScreen", "PRI")] {
            let got = prompts::key_label(key);
            if got != want {
                println!("  실패: 키 글자 {key:?} -> {got}");
                ok = false;
            }
        }
        println!("키 그림 이름: {}", if ok { "맞음" } else { "틀림" });
        if !ok {
            fails += 1;
        }
    }
    // 9. Deviant armor: used only from its start rank on (any enabled rank at or above it) and when allowed or owned.
    //    Targets a skill tree that only that deviant series has (Redhelm low, Drilltusk high, Elderfrost G).
    {
        let mut ok = true;
        for (tree, start) in [(145u16, 1usize), (148, 2), (161, 3)] {
            let pt = db.gd.skills.iter().filter(|s| s.tree == tree && s.threshold > 0).map(|s| s.threshold).min().unwrap();
            let series: Vec<u16> = db.gd.armor.iter().filter(|a| a.skills.iter().any(|s| s.tree == tree && s.pt > 0)).map(|a| a.id).collect();
            for rank in 0..3usize {
                for (allow, own) in [(false, false), (true, false), (false, true)] {
                    let owned = if own { series.iter().copied().collect() } else { Default::default() };
                    db.set_inv(db::Inv { owned, loaded: true, ..Default::default() });
                    let mut a = App::new(db);
                    a.inline_build = true;
                    a.targets = vec![skill_id(db, tree, pt)];
                    a.ranks = core::array::from_fn(|i| i == rank);
                    a.deviant = allow;
                    a.run_search();
                    run_to_end(&mut a);
                    let want = (allow || own) && rank + 1 >= start;
                    if a.results.is_empty() == want {
                        println!("  실패: 이명 트리 {tree} 등급 {} 허용 {allow} 보유 {own} -> 결과 {}개", rank + 1, a.results.len());
                        ok = false;
                    }
                }
            }
        }
        db.set_inv(db::Inv::default());
        println!("이명 방어구 등급·옵션: {}", if ok { "맞음" } else { "틀림" });
        if !ok {
            fails += 1;
        }
    }
    // 10. 'Only gear the smithy can make' with a fake smithy list (the real one is read from game memory)
    {
        let mut ok = true;
        let all_armor: std::collections::BTreeSet<u16> = db.gd.armor.iter().map(|a| a.id).collect();
        let all_jewels: std::collections::BTreeSet<u16> = db.gd.jewels.iter().map(|j| j.id).collect();
        let smithy = |armor: std::collections::BTreeSet<u16>, jewels: std::collections::BTreeSet<u16>, sig: u64| Some(db::Smithy { armor, jewels, deco_recipes: 0, sig });
        let run = |inv: db::Inv, targets: Vec<u16>, deviant: bool| {
            db.set_inv(inv);
            let mut a = App::new(db);
            a.inline_build = true;
            a.targets = targets;
            a.deviant = deviant;
            a.smithy = true;
            a.run_search();
            run_to_end(&mut a);
            a
        };
        // (a) List unreadable: the search is refused with a message
        let a = run(db::Inv { loaded: true, ..Default::default() }, vec![skill_id(db, 19, 20)], false);
        if a.message != k::MSG_SMITHY_UNREAD || !a.results.is_empty() {
            println!("  실패: 대장간 목록 없음인데 검색함 (메시지 {})", a.message);
            ok = false;
        }
        // (b) Armor: Redhelm's skill tree is only on Redhelm armor. Not on the list -> no set (and the cause analysis names
        //     the smithy option); on the list -> sets; owned but not on the list -> sets
        let target = vec![skill_id(db, 145, db.gd.skills.iter().filter(|s| s.tree == 145 && s.threshold > 0).map(|s| s.threshold).min().unwrap())];
        let series: std::collections::BTreeSet<u16> = db.gd.armor.iter().filter(|a| a.skills.iter().any(|s| s.tree == 145 && s.pt > 0)).map(|a| a.id).collect();
        let without: std::collections::BTreeSet<u16> = all_armor.difference(&series).copied().collect();
        let a = run(db::Inv { loaded: true, smithy: smithy(without.clone(), all_jewels.clone(), 1), ..Default::default() }, target.clone(), true);
        let named = a.diag.iter().any(|(l, _)| matches!(l, app::DiagLabel::Smithy));
        if !a.results.is_empty() || !named {
            println!("  실패: 대장간에 없는 홍두 방어구 결과 {}개, 원인 분석 대장간 제안 {named}", a.results.len());
            ok = false;
        }
        let a = run(db::Inv { loaded: true, smithy: smithy(all_armor.clone(), all_jewels.clone(), 2), ..Default::default() }, target.clone(), true);
        if a.results.is_empty() {
            println!("  실패: 대장간에 있는 홍두 방어구인데 결과 없음");
            ok = false;
        }
        let a = run(db::Inv { loaded: true, owned: series.clone(), smithy: smithy(without, all_jewels.clone(), 3), ..Default::default() }, target, true);
        if a.results.is_empty() {
            println!("  실패: 가진 홍두 방어구인데 대장간 목록 밖이라 빠짐");
            ok = false;
        }
        // (c) Decorations: with an empty smithy decoration list no set may use a decoration
        let three: Vec<u16> = [(19, 20), (72, 10), (35, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        let a = run(db::Inv { loaded: true, smithy: smithy(all_armor.clone(), Default::default(), 4), ..Default::default() }, three.clone(), false);
        let used = a.results.iter().filter(|s| !s.jewels.is_empty()).count();
        let b = run(db::Inv { loaded: true, smithy: smithy(all_armor, all_jewels, 5), ..Default::default() }, three, false);
        let used_all = b.results.iter().filter(|s| !s.jewels.is_empty()).count();
        if used != 0 || used_all == 0 {
            println!("  실패: 장식품 목록 비움 -> 장식품 쓴 세트 {used}개 (목록 전체일 때 {used_all}개)");
            ok = false;
        }
        db.set_inv(db::Inv::default());
        println!("대장간에서 만들 수 있는 장비만: {}", if ok { "맞음" } else { "틀림" });
        if !ok {
            fails += 1;
        }
    }
    // 11. 'Nothing with owned charms' memory: newly owned armor (e.g. a quest reward) must not reuse it
    {
        let six: Vec<u16> = [(19, 20), (72, 10), (35, 10), (36, 10), (6, 10), (71, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        let seven: Vec<u16> = six.iter().copied().chain(core::iter::once(skill_id(db, 46, 10))).collect();
        db.set_inv(db::Inv { loaded: true, ..Default::default() });
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = six;
        a.run_search();
        run_to_end(&mut a);
        // Same charms, one more owned armor piece
        let reward = db.reward_armor[0];
        db.set_inv(db::Inv { loaded: true, owned: [reward].into_iter().collect(), ..Default::default() });
        a.targets = seven;
        a.run_search();
        run_to_end(&mut a);
        let skipped = a.last_search.take().is_some_and(|s| s.owned_skipped);
        db.set_inv(db::Inv::default());
        let ok = !skipped;
        println!("보유 방어구가 바뀐 뒤 '결과 없음' 기억 안 씀: {}", if ok { "맞음" } else { "틀림" });
        if !ok {
            fails += 1;
        }
    }
    // 12. Excluding a decoration from the results: X on a part row with decorations opens its exclude list (armor first,
    //     then the decorations); picking a decoration and confirming searches again without it, and it is saved
    {
        let mut ok = true;
        db.set_inv(db::Inv { loaded: true, ..Default::default() });
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = [(19, 20), (72, 10), (35, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        a.run_search();
        run_to_end(&mut a);
        // First result row with a decoration in an armor part
        let pick = a.results.iter().enumerate().find_map(|(i, s)| {
            let (placed, _) = s.placement(db);
            (0..PARTS).find(|&p| !placed[p].is_empty()).map(|p| (i, p, placed[p][0]))
        });
        match pick {
            None => {
                println!("  실패: 장식품을 쓰는 결과가 없어 검사할 수 없음");
                ok = false;
            }
            Some((i, p, jewel)) => {
                a.ridx = a.rows().iter().position(|r| *r == Some(i)).unwrap_or(0);
                a.focus = app::Focus::Parts;
                a.pidx = p;
                a.press(Button::X);
                let items = a.exclude_items();
                let at = items.iter().position(|x| *x == app::ExItem::Jewel(jewel));
                if a.exmenu.is_none() || items.first() != Some(&app::ExItem::Armor(a.current().unwrap().head(p))) || at.is_none() {
                    println!("  실패: 제외 목록이 안 열리거나 내용이 다름 (열림 {})", a.exmenu.is_some());
                    ok = false;
                } else {
                    for _ in 0..at.unwrap() {
                        a.press(Button::Down);
                    }
                    a.press(Button::A); // pick the decoration
                    a.press(Button::A); // confirm (cursor on 'yes')
                    run_to_end(&mut a);
                    let used = a.results.iter().filter(|s| s.jewels.iter().any(|j| j.0 == jewel)).count();
                    let saved = a.encode().contains(&format!("excluded_jewels={jewel}"));
                    if a.excluded_jewels != vec![jewel] || used != 0 || !saved {
                        println!("  실패: 제외 목록 {:?}, 그 장식품을 쓰는 결과 {used}개, 저장 {saved}", a.excluded_jewels);
                        ok = false;
                    }
                }
            }
        }
        db.set_inv(db::Inv::default());
        println!("장식품 제외: {}", if ok { "맞음" } else { "틀림" });
        if !ok {
            fails += 1;
        }
    }
    // 13. Unowned-charm results: fixing a part (Y) or excluding armor (X) searches again without asking about unowned
    //     charms again (the narrower condition skips the owned-charm phase from memory and goes on with unowned charms)
    {
        let mut ok = true;
        db.set_inv(db::Inv { loaded: true, ..Default::default() });
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = [(19, 20), (72, 10), (35, 10), (36, 10), (6, 10), (71, 10)].iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        a.run_search();
        run_to_end(&mut a);
        // Run to the end, counting the unowned-charm questions (answered yes so the run can finish)
        let asked_run = |a: &mut App| -> usize {
            let mut asked = 0;
            let mut guard = 0;
            while a.job.is_some() || a.building.is_some() {
                a.tick(1000);
                for i in 0..job::MAX_HELPERS {
                    job::helper_run(i, 5);
                }
                if a.job.as_ref().is_some_and(|j| j.state == job::State::AskNeeded) {
                    asked += 1;
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    if a.job.as_ref().is_some_and(|j| j.ask_ready()) {
                        a.press(Button::A);
                    }
                }
                guard += 1;
                assert!(guard < 100_000, "검색이 끝나지 않는다");
            }
            asked
        };
        if !a.needed_mode || a.results.is_empty() {
            println!("  실패: 미보유 호석 결과가 아님 (needed {}, 결과 {}개)", a.needed_mode, a.results.len());
            ok = false;
        } else {
            if a.notice.is_some() {
                a.press(Button::A); // close the 'searched with unowned charms' notice
            }
            a.ridx = a.rows().iter().position(|r| r.is_some()).unwrap_or(0);
            a.focus = app::Focus::Parts;
            a.pidx = 1;
            a.press(Button::Y); // fix this part from the results
            let asked = asked_run(&mut a);
            let skipped = a.last_search.take().is_some_and(|s| s.owned_skipped);
            if asked != 0 || !skipped || !a.needed_mode || a.results.is_empty() || a.fixed[1] == 0 || a.notice.is_some() {
                println!(
                    "  실패: 고정 후 질문 {asked}번, 보유 단계 건너뜀 {skipped}, needed {}, 결과 {}개, 알림 {}",
                    a.needed_mode,
                    a.results.len(),
                    a.notice.is_some()
                );
                ok = false;
            }
            // Exclude another part's armor (X, then 'yes')
            if a.notice.is_some() {
                a.press(Button::A);
            }
            a.ridx = a.rows().iter().position(|r| r.is_some()).unwrap_or(0);
            a.focus = app::Focus::Parts;
            a.pidx = 2;
            a.press(Button::X);
            if a.exmenu.is_some() {
                a.press(Button::A); // the armor (first item)
            }
            a.press(Button::A); // confirm
            let asked = asked_run(&mut a);
            if asked != 0 || a.excluded.is_empty() || !a.needed_mode || a.notice.is_some() {
                println!("  실패: 빼기 후 질문 {asked}번, 뺀 방어구 {}개, needed {}, 알림 {}", a.excluded.len(), a.needed_mode, a.notice.is_some());
                ok = false;
            }
            // A new search from the main screen still asks
            a.press(Button::B);
            a.screen = app::Screen::Main;
            a.fixed = [0; PARTS];
            a.excluded.clear();
            a.targets.push(skill_id(db, 46, 10));
            a.run_search();
            let asked = asked_run(&mut a);
            if asked == 0 || (a.needed_mode && a.notice.is_none()) {
                println!("  실패: 메인 화면의 새 검색에서 질문 {asked}번, 미보유 결과 알림 {}", a.notice.is_some());
                ok = false;
            }
        }
        db.set_inv(db::Inv::default());
        println!("미보유 호석 결과에서 고정·빼기 후 다시 묻거나 알리지 않음: {}", if ok { "맞음" } else { "틀림" });
        if !ok {
            fails += 1;
        }
    }
    println!("{}", if fails == 0 { "모두 통과" } else { "실패 있음" });
    std::process::exit(if fails == 0 { 0 } else { 1 });
}

/// Per-step time of the alternative computation (build searcher / search / convert results / build lists)
fn alt_bench(db: &'static db::Db) {
    use std::time::Instant;
    let conds: [&[(u16, i8)]; 3] = [&[(19, 20)], &[(19, 20), (72, 10), (35, 10)], &[(19, 20), (72, 10), (36, 10), (71, 10)]];
    for tg in conds {
        let mut a = App::new(db);
        a.inline_build = true;
        a.targets = tg.iter().map(|&(t, p)| skill_id(db, t, p)).collect();
        a.weapon = 3;
        a.run_search();
        run_to_end(&mut a);
        println!("조건 {:?}: 결과 {}개", tg, a.results.len());
        let mut tot = [0f64; 4];
        let n = a.results.len().min(5);
        for i in 0..n {
            a.ridx = i;
            let cur = a.current().cloned().unwrap();
            let mut line = String::new();
            for p in 0..5 {
                let t0 = Instant::now();
                let c = a.alt_cond(&cur, p);
                let Some(mut job) = App::alt_build(db, &c) else { continue };
                let t1 = Instant::now();
                while job.state == job::State::Running {
                    job.step(1000);
                }
                let nodes = job.total_nodes();
                let t2 = Instant::now();
                let found = job.result(db);
                let sets = found.sets.len();
                let t3 = Instant::now();
                let list = App::alt_collect(&cur, p, found).items.len();
                let t4 = Instant::now();
                let ms = |x: Instant, y: Instant| (y - x).as_secs_f64() * 1000.;
                tot[0] += ms(t0, t1);
                tot[1] += ms(t1, t2);
                tot[2] += ms(t2, t3);
                tot[3] += ms(t3, t4);
                line += &format!(" [부위{p} 노드{nodes} 세트{sets} 목록{list} {:.1}/{:.1}/{:.1}/{:.1}ms]", ms(t0, t1), ms(t1, t2), ms(t2, t3), ms(t3, t4));
            }
            println!("  세트{i}:{line}");
        }
        println!("  세트당 평균: 만들기 {:.1}ms, 검색 {:.1}ms, 결과 바꾸기 {:.1}ms, 목록 {:.1}ms", tot[0] / n as f64, tot[1] / n as f64, tot[2] / n as f64, tot[3] / n as f64);
    }
}
