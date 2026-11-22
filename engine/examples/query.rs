//! Command-line searcher called by the preview program. Arguments are key=value.
//!   query kind=1 sex=1 weapon=2 forbid=1 rare=65535 targets=72:10,71:20
//!         jewelrank=3 priority=0 exflags=1 mindef=0 minres=-,-,5,-,0 needed=1
//!         fixed=0,0,0,0,0 excluded=12,55 charms=72:5:71:3:2;36:4:0:0:1 max=100 budget=50000000
//!   query extra=1 fixed=<5 ids> charms=<1 or -> targets=... weapon=.. forbid=.. jewelrank=.. [needed=1]
//! Output
//!   STAT <done|paused> <nodes> <ms> <owned|needed>
//!   SET <free> <bonus> <def> <charm|-1> <armor ids per part (; separated), parts , separated> <decorations item:torso;...|->
//!       <needed charm t1:p1:t2:p2:slots|-> <chance of getting that charm (estimate)> <rarity of that charm|0>
//!   VAR ... (same format as SET) a similar set grouped under the SET just above
//!   BEST ... (same format as SET) non-dominated set (when owned= is given). May be outside the result limit
//!   EXTRA <skill ids that can additionally be activated>
//!   ALT <part> ... (when alts=0,1,..) a set with only that part changed, same format as SET
use skillsim_engine::data::{self, GameData};
use skillsim_engine::search::{Query, Searcher, Status, Target, MAXT};
use skillsim_engine::types::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

fn nums<T: std::str::FromStr>(s: &str, sep: char) -> Vec<T> {
    s.split(sep).filter(|x| !x.is_empty() && *x != "-").filter_map(|x| x.parse().ok()).collect()
}

/// For this set (armor and charm all fixed), find skills that can additionally be activated by changing only decorations.
fn extra_skills(gd: &GameData, base: &Query, targets: &[Target]) {
    let mut found: Vec<u16> = Vec::new();
    let mut skills: Vec<&Skill> = gd.skills.iter().filter(|s| s.threshold >= 2 && s.tree != TREE_TORSO_UP).collect();
    skills.sort_by_key(|s| (s.tree, s.threshold));
    let mut failed_tree = 0;
    for s in skills {
        if s.tree == failed_tree {
            continue; // If a lower level fails, higher levels fail too
        }
        let mut list = targets.to_vec();
        match list.iter().position(|t| t.tree == s.tree) {
            Some(i) if list[i].need >= s.threshold as i16 => continue, // Level already in the targets
            Some(i) => list[i].need = s.threshold as i16,
            None if list.len() < MAXT => list.push(Target { tree: s.tree, need: s.threshold as i16 }),
            None => continue,
        }
        let q = Query { targets: &list, max_results: 1, dominance: false, ..*base };
        let ok = Searcher::new(&gd.armor, &gd.jewels, &q).is_ok_and(|mut se| {
            se.run(200_000);
            !se.results().is_empty()
        });
        if ok {
            found.push(s.id);
        } else {
            failed_tree = s.tree;
        }
    }
    println!("EXTRA {}", found.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","));
}

fn print_set(gd: &GameData, s: &Searcher, mode: &str, min_def: u16, tag: &str, r: &skillsim_engine::search::SetResult) {
    println!("{}", set_line(gd, s, mode, min_def, tag, r));
}

fn set_line(gd: &GameData, s: &Searcher, mode: &str, min_def: u16, tag: &str, r: &skillsim_engine::search::SetResult) -> String {
    let parts: Vec<String> = (0..PARTS)
        .map(|p| {
            let members = s.armor_members(p, r.groups[p]);
            let head = gd.armor[members[0] as usize].def;
            // Only alternatives whose total defense still meets the condition (the first of a group has the highest defense)
            let ids: Vec<String> = members
                .iter()
                .map(|&i| &gd.armor[i as usize])
                .filter(|a| r.def - head + a.def >= min_def)
                .map(|a| a.id.to_string())
                .collect();
            ids.join(";")
        })
        .collect();
    let jewels: Vec<String> = r.jewels[..r.nj as usize]
        .iter()
        .map(|p| format!("{}:{}", s.jewel(p.jewel).id, if p.torso { 1 } else if p.charm { 2 } else { 0 }))
        .collect();
    let charm = s.charm_members(r.charm).first().map_or(-1, |&i| i as i32);
    // Rarity of the needed charm: the value when this minimum charm comes from the easiest charm kind
    let rare = if mode == "needed" {
        skillsim_engine::charm::best_kind(&gd.charm_kinds, &r.need)
            .map_or(0, |(i, _)| skillsim_engine::charm::rarity(i, &gd.charm_kinds[i], &r.need))
    } else {
        0
    };
    let need = if mode == "needed" {
        let k = r.need.skills;
        format!("{}:{}:{}:{}:{}", k[0].tree, k[0].pt, k[1].tree, k[1].pt, r.need.slots)
    } else {
        "-".to_string()
    };
    format!(
        "{} {} {} {} {} {} {} {} {:e} {}",
        tag,
        r.free,
        r.bonus,
        r.def,
        charm,
        parts.join(","),
        if jewels.is_empty() { "-".to_string() } else { jewels.join(";") },
        need,
        r.chance as f64 / 1e15,
        rare,
    )
}

/// Alternatives: from the given set (5 parts in fixed, its charm as the single entry in charms) free one part
/// and find every set that meets the conditions. Dominance pruning is off, so armor that would be dropped for "a better one exists" also appears.
/// Output: ALT <part> followed by the SET format
fn alternatives(gd: &GameData, base: &Query, parts: &[usize]) {
    for &p in parts.iter().filter(|&&p| p < PARTS) {
        let mut fixed = base.fixed;
        fixed[p] = 0;
        let q = Query { fixed, dominance: false, group_similar: false, pareto: false, max_results: 2000, ..*base };
        let Ok(mut s) = Searcher::new(&gd.armor, &gd.jewels, &q) else { continue };
        s.run(u64::MAX);
        for r in s.sorted_results() {
            print_set(gd, &s, "owned", base.min_def, &format!("ALT {p}"), &r);
        }
    }
}

fn main() {
    let args: HashMap<String, String> = std::env::args()
        .skip(1)
        .filter_map(|a| a.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    let get = |k: &str, d: &str| args.get(k).cloned().unwrap_or_else(|| d.to_string());
    let blob = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../data/simdata.bin")).expect("simdata.bin 없음");
    let gd = data::parse(&blob).expect("simdata.bin 형식 오류");

    let targets: Vec<Target> = get("targets", "")
        .split(',')
        .filter_map(|t| t.split_once(':'))
        .map(|(t, n)| Target { tree: t.parse().unwrap(), need: n.parse().unwrap() })
        .collect();
    let charms: Vec<Charm> = get("charms", "")
        .split(';')
        .map(|c| nums::<i32>(c, ':'))
        .filter(|v| v.len() == 5)
        .map(|v| Charm {
            skills: [Pt { tree: v[0] as u16, pt: v[1] as i8 }, Pt { tree: v[2] as u16, pt: v[3] as i8 }],
            slots: v[4] as u8,
        })
        .collect();
    let mut fixed = [0u16; PARTS];
    for (i, id) in nums::<u16>(&get("fixed", ""), ',').into_iter().take(PARTS).enumerate() {
        fixed[i] = id;
    }
    let excluded = nums::<u16>(&get("excluded", ""), ',');
    let owned = nums::<u16>(&get("owned", ""), ',');
    let priority: u8 = get("priority", "0").parse().unwrap();
    let min_def: u16 = get("mindef", "0").parse().unwrap();
    let mut min_res = [None; RES];
    for (e, v) in get("minres", "").split(',').take(RES).enumerate() {
        min_res[e] = v.parse::<i8>().ok();
    }
    // Skill trees with a negative skill (skills with negative points in the DB skill table)
    let mut negative_trees: Vec<u16> = gd.skills.iter().filter(|s| s.threshold < 0).map(|s| s.tree).collect();
    negative_trees.sort();
    negative_trees.dedup();
    // jewels=id,id,...: only these decorations (the plugin's 'smithy only' list); '-' or missing = no list
    let jewel_ids: Option<Vec<u16>> = match get("jewels", "-").as_str() {
        "-" => None,
        v => {
            let mut ids: Vec<u16> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            ids.sort();
            Some(ids)
        }
    };
    let q = Query {
        negative_trees: &negative_trees,
        targets: &targets,
        kind: get("kind", "1").parse().unwrap(),
        sex: get("sex", "1").parse().unwrap(),
        rare_mask: get("rare", "65535").parse().unwrap(),
        weapon_slots: get("weapon", "0").parse().unwrap(),
        exclude_flags: get("exflags", "0").parse().unwrap(),
        jewel_rank: get("jewelrank", "3").parse().unwrap(),
        jewel_ids: jewel_ids.as_deref(),
        priority,
        forbid_negative: get("forbid", "1") == "1",
        dominance: get("dominance", "1") == "1",
        // Searches that rank by defense must not drop armor with higher defense.
        // thorough=1 ('Thorough'): non-dominated sets exact down to defense (slow)
        // (with free-slot priority only, same as the plugin)
        keep_defense: priority == 1 || min_def > 0 || (priority == 0 && get("thorough", "0") == "1"),
        relaxed: true,
        fixed,
        excluded: &excluded,
        charms: &charms,
        max_results: get("max", "100").parse().unwrap(),
        min_def,
        min_res,
        group_similar: get("group", "1") == "1",
        charm_kinds: None,
        owned: &owned,
        // When owned armor is given, non-dominated sets are collected by default.
        pareto: get("pareto", if owned.is_empty() { "0" } else { "1" }) == "1",
    };
    if get("extra", "0") == "1" {
        // needed=1: use any obtainable charm (the needed charm) instead of the owned charms.
        let base = if get("needed", "0") == "1" { Query { charm_kinds: Some(&gd.charm_kinds), ..q } } else { q };
        extra_skills(&gd, &base, &targets);
        return;
    }
    if get("alts", "-") != "-" {
        alternatives(&gd, &q, &nums::<usize>(&get("alts", ""), ','));
        return;
    }

    let budget: u64 = get("budget", "50000000").parse().unwrap();
    let t0 = Instant::now();
    let mut mode = "owned";
    // Rarity priority with owned charms runs in stages of rising rarity cap (engine search.rs 'Rarity priority in
    // stages', same as the plugin): free-slot searches without unowned armor above the cap, keeping sets of exactly that rarity
    let mut owned_sorted = owned.clone();
    owned_sorted.sort();
    let staged = priority == 2 && get("first", "0") != "1" && get("neededonly", "0") != "1";
    let stage_max = if staged { skillsim_engine::search::stage_max_rarity(&gd.armor) } else { 0 };
    let stage_ex: Vec<Vec<u16>> = (0..=stage_max)
        .map(|cap| if staged { excluded.iter().copied().chain(skillsim_engine::search::stage_excluded(&gd.armor, &owned_sorted, cap)).collect() } else { Vec::new() })
        .collect();
    let stage_query = |cap: u8| Query { priority: 0, keep_defense: min_def > 0, excluded: &stage_ex[cap as usize], ..q };
    let mut stage = Stage { cap: 0, lines: Vec::new(), kept: 0, nodes: 0 };
    let mut s = match Searcher::new(&gd.armor, &gd.jewels, &if staged { stage_query(0) } else { Query { ..q } }) {
        Ok(s) => s,
        Err(e) => {
            println!("ERROR {e:?}");
            return;
        }
    };
    // interactive=1: runs in small steps printing PROG lines, and stops at whichever comes first:
    // the node limit (budget) or the time limit (timelimit ms) (counted across the owned -> unowned charm phases).
    // When stopped it prints the results so far and PAUSED, then waits for a command.
    // Commands (one stdin line): "c <node limit> <time limit ms>" = continue with that limit, "s" = stop now and give results (also while running),
    // anything else = end. At the end it prints the results and END.
    let interactive = get("interactive", "0") == "1";
    let first = get("first", "0") == "1";
    let mut time_limit = Duration::from_millis(get("timelimit", "10000").parse().unwrap());
    let mut node_limit = budget;
    let slice: u64 = get("slice", "200000").parse().unwrap();
    let slow = Duration::from_millis(get("slowms", "0").parse().unwrap()); // Emulate a slow device
    let want_needed = get("needed", "0") == "1";
    // If owned charms fail, look for the needed charm. Not done when the charm is fixed.
    // The defense keeping of 'Thorough' is for recommended sets, so it is not used in the unowned-charm phase (same as the plugin)
    let needed = Query { charm_kinds: Some(&gd.charm_kinds), keep_defense: priority == 1 || min_def > 0, ..q };
    // neededonly=1: skip the owned-charm phase and go straight to unowned (any) charms.
    // For when only 'does any charm work' matters (cause analysis), so the owned phase does not use up the limit first.
    if want_needed && get("neededonly", "0") == "1" {
        mode = "needed";
        s = Searcher::new(&gd.armor, &gd.jewels, &needed).unwrap();
    }
    let commands = if interactive {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { break };
                if tx.send(line.trim().to_string()).is_err() {
                    break;
                }
            }
        });
        Some(rx)
    } else {
        None
    };
    let mut status;
    let mut deadline = Instant::now() + time_limit;
    let mut nodes_left = node_limit;
    loop {
        let mut stop = false;
        status = match &commands {
            Some(rx) => step(&mut s, &mut nodes_left, deadline, slice, slow, mode, rx, &mut stop),
            None if first => run_first(&mut s, budget, deadline, slice, slow),
            None => s.run(budget),
        };
        if stop {
            break; // The user pressed "stop and show results"
        }
        if get("profile", "0") == "1" && status == Status::Done {
            // Measurement: per phase nodes, combinations reaching the end, decoration placement work, per level (groups, groups after dominance)
            println!("PROFILE {mode} nodes {} leaves {} deco {} ms {:.1} groups {:?}", s.nodes, s.leaves, s.deco_nodes, t0.elapsed().as_secs_f64() * 1e3, s.group_counts);
        }
        if staged && mode == "owned" && status == Status::Done {
            // Keep this stage's sets of exactly its rarity, then the next stage while the results are not full
            let max = q.max_results;
            for r in s.sorted_results() {
                if stage.kept >= max {
                    break;
                }
                if skillsim_engine::search::set_rarity(&s, &gd.armor, &owned_sorted, &r) == stage.cap {
                    stage.lines.push(set_line(&gd, &s, mode, min_def, "SET", &r));
                    stage.lines.extend(r.variants.iter().map(|v| set_line(&gd, &s, mode, min_def, "VAR", v)));
                    stage.kept += 1;
                }
            }
            if stage.kept < max && stage.cap < stage_max {
                stage.nodes += s.nodes;
                stage.cap += 1;
                s = Searcher::new(&gd.armor, &gd.jewels, &stage_query(stage.cap)).unwrap();
                continue;
            }
        }
        let staged_empty = staged && mode == "owned" && stage.kept == 0;
        if (if staged && mode == "owned" { staged_empty } else { s.results().is_empty() }) && status == Status::Done && want_needed && mode == "owned" {
            // ask=1 (window run): ask before moving to unowned charms. "n" = search, anything else = end with no results
            if let (Some(rx), true) = (&commands, get("ask", "0") == "1") {
                println!("ASKNEEDED");
                if rx.recv().unwrap_or_default() != "n" {
                    break;
                }
            }
            mode = "needed";
            s = Searcher::new(&gd.armor, &gd.jewels, &needed).unwrap();
            continue;
        }
        let Some(rx) = &commands else { break };
        if status == Status::Done {
            break;
        }
        print_out(&gd, &s, mode, min_def, status, t0, staged, &stage);
        println!("PAUSED");
        let cmd = rx.recv().unwrap_or_default();
        let f: Vec<&str> = cmd.split(' ').collect();
        if f[0] != "c" {
            println!("END");
            return;
        }
        if f.len() >= 3 {
            node_limit = f[1].parse().unwrap_or(node_limit);
            time_limit = Duration::from_millis(f[2].parse().unwrap_or(time_limit.as_millis() as u64));
        }
        deadline = Instant::now() + time_limit;
        nodes_left = node_limit;
    }
    print_out(&gd, &s, mode, min_def, status, t0, staged, &stage);
    if interactive {
        println!("END");
    }
}

/// Kept sets of finished stages (rarity priority in stages)
struct Stage {
    cap: u8,
    lines: Vec<String>,
    kept: usize,
    /// Nodes of finished stages
    nodes: u64,
}

/// Results: from the stages (owned-charm phase of rarity priority) or from the searcher
#[allow(clippy::too_many_arguments)]
fn print_out(gd: &GameData, s: &Searcher<'_>, mode: &str, min_def: u16, status: Status, t0: Instant, staged: bool, stage: &Stage) {
    if !(staged && mode == "owned") {
        print_block(gd, s, mode, min_def, status, t0);
        return;
    }
    println!(
        "STAT {} {} {:.1} {mode}",
        if status == Status::Done { "done" } else { "paused" },
        stage.nodes + s.nodes,
        t0.elapsed().as_secs_f64() * 1e3
    );
    for l in &stage.lines {
        println!("{l}");
    }
    for r in s.front() {
        print_set(gd, s, mode, min_def, "BEST", &r);
    }
}

/// Stop as soon as one set is found (that phase counts as Done). For when only 'is it possible' matters (cause analysis).
/// Stop when the node limit or the deadline (time limit) is reached.
fn run_first(s: &mut Searcher<'_>, budget: u64, deadline: Instant, slice: u64, slow: Duration) -> Status {
    let start = s.nodes;
    loop {
        let st = s.run(slice);
        if st == Status::Done || !s.results().is_empty() {
            return Status::Done;
        }
        if s.nodes - start >= budget || Instant::now() >= deadline {
            return Status::Paused;
        }
        if !slow.is_zero() {
            std::thread::sleep(slow);
        }
    }
}

/// Run in small steps within the limit (remaining nodes or deadline) and report progress. Stop at once on an "s" command.
#[allow(clippy::too_many_arguments)]
fn step(
    s: &mut Searcher<'_>,
    nodes_left: &mut u64,
    deadline: Instant,
    slice: u64,
    slow: Duration,
    mode: &str,
    rx: &std::sync::mpsc::Receiver<String>,
    stop: &mut bool,
) -> Status {
    loop {
        let before = s.nodes;
        let st = s.run(slice.min((*nodes_left).max(1)));
        *nodes_left = nodes_left.saturating_sub(s.nodes - before);
        println!("PROG {mode} {:.4} {} {}", s.progress(), s.nodes, s.results().len());
        if st == Status::Done {
            return st;
        }
        if rx.try_recv().is_ok_and(|c| c == "s") {
            *stop = true;
            return Status::Paused;
        }
        if *nodes_left == 0 || Instant::now() >= deadline {
            return Status::Paused;
        }
        if !slow.is_zero() {
            std::thread::sleep(slow);
        }
    }
}

fn print_block(gd: &GameData, s: &Searcher<'_>, mode: &str, min_def: u16, status: Status, t0: Instant) {
    println!(
        "STAT {} {} {:.1} {mode}",
        if status == Status::Done { "done" } else { "paused" },
        s.nodes,
        t0.elapsed().as_secs_f64() * 1e3
    );
    for r in s.sorted_results() {
        print_set(gd, s, mode, min_def, "SET", &r);
        for v in &r.variants {
            print_set(gd, s, mode, min_def, "VAR", v);
        }
    }
    for r in s.front() {
        print_set(gd, s, mode, min_def, "BEST", &r);
    }
}
