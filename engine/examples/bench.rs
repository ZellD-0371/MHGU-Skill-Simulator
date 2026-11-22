//! Measures search size and time on the real game data.
//!   cargo run --release --example bench
use skillsim_engine::data;
use skillsim_engine::search::{Query, Searcher, Status, Target, LEVELS};
use skillsim_engine::types::*;
use std::collections::HashMap;
use std::time::Instant;

const NODE_CAP: u64 = 300_000_000;

fn names(path: &str, col: usize) -> HashMap<u16, String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some((f.first()?.parse().ok()?, f.get(col)?.to_string()))
        })
        .collect()
}

/// Fake charms for speed measurement. Unrelated to the real generation rules.
fn fake_charms(jewels: &[Jewel], count: usize) -> Vec<Charm> {
    let trees: Vec<u16> = {
        let mut t: Vec<u16> = jewels.iter().map(|j| j.pos.tree).collect();
        t.dedup();
        t
    };
    let mut x = 0x2545f4914f6cdd1du64;
    let mut next = move |n: u64| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x % n
    };
    (0..count)
        .map(|_| Charm {
            skills: [
                Pt { tree: trees[next(trees.len() as u64) as usize], pt: 1 + next(6) as i8 },
                Pt { tree: trees[next(trees.len() as u64) as usize], pt: next(12) as i8 - 3 },
            ],
            slots: next(4) as u8,
        })
        .collect()
}

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../data/");
    let blob = std::fs::read(format!("{dir}simdata.bin")).expect("tools/build_data.py 를 먼저 실행");
    let gd = data::parse(&blob).expect("simdata.bin 형식 오류");
    let mut negative_trees: Vec<u16> = gd.skills.iter().filter(|s| s.threshold < 0).map(|s| s.tree).collect();
    negative_trees.dedup();
    let skill_names = names(&format!("{dir}skills.tsv"), 4);
    let armor_names = names(&format!("{dir}armor.tsv"), 2);
    let jewel_names = names(&format!("{dir}jewels.tsv"), 2);
    let target = |name: &str| -> Target {
        let id = skill_names.iter().find(|(_, n)| n.as_str() == name).unwrap_or_else(|| panic!("스킬 없음: {name}")).0;
        let s = gd.skills.iter().find(|s| s.id == *id).unwrap();
        Target { tree: s.tree, need: s.threshold as i16 }
    };
    let charms = fake_charms(&gd.jewels, 60);

    let queries: [(&str, u8, &[&str]); 5] = [
        ("쉬움", 1, &["귀마개"]),
        ("보통", 1, &["약점 특효", "간파+3", "슈퍼회심"]),
        ("어려움", 1, &["장인+2", "약점 특효", "슈퍼회심", "도전자+2", "명검"]),
        ("거너", 2, &["관통탄/관통화살 강화", "장전 수UP", "간파+3", "약점 특효"]),
        ("불가능", 1, &["장인+2", "공격력UP【대】", "간파+3", "약점 특효", "슈퍼회심", "도전자+2", "고급 귀마개", "회피 성능+2"]),
    ];
    // (name, forbid negatives, dominance pruning, relaxed, use charms, compare defense)
    let modes: [(&str, bool, bool, bool, bool, bool); 2] = [
        ("기본(금지+제거+완화)", true, true, true, true, false),
        ("기본 + 방어력 비교", true, true, true, true, true),
    ];

    for (label, kind, skills) in queries {
        let targets: Vec<Target> = skills.iter().map(|s| target(s)).collect();
        println!("\n=== {label}: {}", skills.join(", "));
        for (mode, forbid, dominance, relaxed, with_charms, keep_defense) in modes {
            let q = Query {
                targets: &targets,
                kind,
                sex: 1,
                rare_mask: 0xffff,
                weapon_slots: 2,
                exclude_flags: 0,
                jewel_rank: 3,
                jewel_ids: None,
                priority: 0,
                forbid_negative: forbid,
                negative_trees: &negative_trees,
                dominance,
                keep_defense,
                relaxed,
                fixed: [0; PARTS],
                excluded: &[],
                charms: if with_charms { &charms } else { &[] },
                max_results: 100,
                min_def: 0,
                min_res: [None; RES],
                group_similar: false,
                charm_kinds: None,
                owned: &[],
                pareto: false,
            };
            let t0 = Instant::now();
            let mut s = Searcher::new(&gd.armor, &gd.jewels, &q).unwrap();
            let prep = t0.elapsed();
            let status = s.run(NODE_CAP);
            let total = t0.elapsed();
            let before: Vec<u16> = (0..LEVELS).map(|l| s.group_counts[l].0).collect();
            let after: Vec<u16> = (0..LEVELS).map(|l| s.group_counts[l].1).collect();
            println!(
                "  [{mode}] 그룹 {before:?} -> {after:?} | 준비 {:.1}ms 전체 {:.1}ms | 노드 {} (장식품 {}) 잎 {} | 결과 {}{}",
                prep.as_secs_f64() * 1e3,
                total.as_secs_f64() * 1e3,
                s.nodes,
                s.deco_nodes,
                s.leaves,
                s.results().len(),
                if status == Status::Paused { " (상한에서 중단)" } else { "" },
            );
            if true {
                if let Some(r) = s.sorted_results().first() {
                    let parts: Vec<String> = (0..PARTS)
                        .map(|p| {
                            let m = s.armor_members(p, r.groups[p]);
                            format!("{}(+{})", armor_names[&gd.armor[m[0] as usize].id], m.len() - 1)
                        })
                        .collect();
                    let jewels: Vec<String> = r.jewels[..r.nj as usize]
                        .iter()
                        .map(|p| format!("{}{}", jewel_names[&s.jewel(p.jewel).id], if p.torso { "[몸통]" } else if p.charm { "[호석x2]" } else { "" }))
                        .collect();
                    let charm = s.charm_members(r.charm).first().map(|&i| charms[i as usize]);
                    println!("    1위: 남는 슬롯 {} 덤 스킬 {} 방어 {}", r.free, r.bonus, r.def);
                    println!("      방어구 {}", parts.join(" / "));
                    println!("      호석 {charm:?}");
                    println!("      장식품 {}", jewels.join(", "));
                }
            }
        }
    }
}
