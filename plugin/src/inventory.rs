//! Reads the inventory from game memory (MHGU 1.4.0). Research 2026-11-18 (static analysis + save editor docs):
//!  - Equipment box sEquipBox = [base+0x1856FE8]. Slot i = box + 0x1C + i*0x2C (2000 slots; same formula as game function 0x343894).
//!    slot +4 u16 kind (bit0-4: 1-5 armor head..legs, 6 charm, 7-21 weapons), +6 u16 equipment number (series number for armor).
//!    Charm: +0x10 u8 tree1, +0x11 u8 tree2, +0x12 i8 points1, +0x13 i8 points2, +0x14 u8 slots.
//!  - Equipped: sPlayer = [base+0x1885B58], +0x84 u16 x7 box indices (weapon, head..legs, charm; 0xFFFF none).
//!  - Item box [base+0x1858438] 2300 slots, pouch [base+0x18584B8] 50 slots: slot +0x1A+i*4 u16 item, +0x1C u16 quantity
//!    (same location as Wishlist).
//! Game tree number + 1 = DB tree number (checked with 424 decorations; charm trees are in 1..179).
//! Armor (part, series) -> DB armor id comes from the table built by tools/build_armor_map.py (5793 material comparisons match).
use crate::db::Inv;
use crate::generated::armor_map::ARMOR_MAP;
use skillsim_engine::types::{Charm, Pt, PARTS, TORSO};

const EQUIP_BOX: u32 = 0x1856fe8;
const PLAYER: u32 = 0x1885b58;
const ITEM_BOX: u32 = 0x1858438;
const POUCH: u32 = 0x18584b8;
const BOX_SLOTS: u32 = 2000;

fn valid(p: u32) -> bool {
    (0x10000..0xeffff000).contains(&p)
}
unsafe fn rd(p: u32) -> u32 {
    unsafe { core::ptr::read_unaligned(p as *const u32) }
}
unsafe fn half(p: u32) -> u16 {
    unsafe { core::ptr::read_unaligned(p as *const u16) }
}
unsafe fn byte(p: u32) -> u8 {
    unsafe { core::ptr::read(p as *const u8) }
}

/// Game armor (part 1-5, series) -> DB armor id
pub fn armor_id(part: u8, series: u16) -> Option<u16> {
    ARMOR_MAP.binary_search_by_key(&(part, series), |r| (r.0, r.1)).ok().map(|i| ARMOR_MAP[i].2)
}

/// Decorations of one equipment box entry: 3 x u16 game item numbers at +0x0A (0 = empty slot).
/// The save editor docs (07-equipment.md) put them at +0x06 of the save entry; the in-memory entry is the same record
/// shifted by 4 bytes (kind +4, id +6 and charm data +0x10 all match the save offsets + 4).
unsafe fn decorations(e: u32) -> [u16; 3] {
    unsafe { [half(e + 0x0a), half(e + 0x0c), half(e + 0x0e)] }
}

/// DB armor id -> game (part 1-5, series) (reverse of armor_id)
pub fn armor_game(id: u16) -> Option<(u8, u16)> {
    ARMOR_MAP.iter().find(|r| r.2 == id).map(|r| (r.0, r.1))
}

/// Decoration id -> game item number (reverse of jewel_of)
#[allow(dead_code)]
pub fn jewel_game(id: u16) -> Option<u16> {
    crate::generated::data::JEWEL_GAME.iter().find(|r| r.1 == id).map(|r| r.0)
}

/// Game item number -> decoration id
fn jewel_of(game: u16) -> Option<u16> {
    let t = crate::generated::data::JEWEL_GAME;
    t.binary_search_by_key(&game, |r| r.0).ok().map(|i| t[i].1)
}

fn tree(game: u8) -> u16 {
    game as u16 + 1
}

/// Inventory of the current character. None while the box pointer is not set yet (title screen etc.).
pub fn read(base: u32) -> Option<Inv> {
    let mut inv = Inv::default();
    let boxp = unsafe { rd(base + EQUIP_BOX) };
    if !valid(boxp) {
        return None;
    }
    // Box index -> charm list index (to find the equipped charm)
    let mut charm_at = alloc::collections::BTreeMap::new();
    let entry = |i: u32| boxp + 0x1c + i * 0x2c;
    for i in 0..BOX_SLOTS {
        let e = entry(i);
        let kind = (unsafe { half(e + 4) } & 31) as u8;
        let id = unsafe { half(e + 6) };
        match kind {
            1..=5 => {
                if let Some(a) = armor_id(kind, id) {
                    inv.owned.insert(a);
                }
            }
            6 => {
                let (t1, t2) = unsafe { (byte(e + 0x10), byte(e + 0x11)) };
                let (p1, p2) = unsafe { (byte(e + 0x12) as i8, byte(e + 0x13) as i8) };
                let slots = unsafe { byte(e + 0x14) }.min(3);
                let pt = |t: u8, p: i8| if t != 0 && p != 0 { Pt { tree: tree(t), pt: p } } else { Pt::default() };
                charm_at.insert(i as u16, inv.charms.len());
                inv.charms.push(Charm { skills: [pt(t1, p1), pt(t2, p2)], slots });
            }
            _ => {}
        }
    }
    let player = unsafe { rd(base + PLAYER) };
    if valid(player) {
        for p in 0..PARTS {
            let idx = unsafe { half(player + 0x84 + 2 * (p as u32 + 1)) };
            if (idx as u32) < BOX_SLOTS {
                let e = entry(idx as u32);
                let kind = (unsafe { half(e + 4) } & 31) as u8;
                if kind as usize == p + 1 {
                    inv.equipped_armor[p] = armor_id(kind, unsafe { half(e + 6) }).unwrap_or(0);
                }
            }
        }
        let idx = unsafe { half(player + 0x84 + 12) };
        inv.equipped_charm = charm_at.get(&idx).copied();
        // Gender: byte +4 of the 12-byte block at +0x550 (0 male, 1 female; read by the Smithy and the Armory).
        // Save editor docs 11-save-map.md (loader offsets; the equipped indices at +0x84 above use the same base).
        inv.sex = match unsafe { byte(player + 0x554) } {
            0 => 1,
            1 => 2,
            _ => 0,
        };
        // Weapon: slot 0, kind 7-21
        let idx = unsafe { half(player + 0x84) };
        if (idx as u32) < BOX_SLOTS {
            let kind = (unsafe { half(entry(idx as u32) + 4) } & 31) as u8;
            if (7..=21).contains(&kind) {
                inv.equipped_weapon = kind;
            }
        }
        // Decorations of the equipped weapon (0), armor (1-5) and charm (6)
        for slot in 0..7u32 {
            let idx = unsafe { half(player + 0x84 + 2 * slot) };
            if (idx as u32) >= BOX_SLOTS {
                continue;
            }
            for g in unsafe { decorations(entry(idx as u32)) } {
                if g == 0 {
                    continue;
                }
                if let Some(j) = jewel_of(g) {
                    // Location as in Set::jewels: torso (slot 2) or charm (slot 6)
                    let loc = match slot as usize {
                        s if s == TORSO + 1 => crate::set::LOC_TORSO,
                        6 => crate::set::LOC_CHARM,
                        _ => crate::set::LOC_ANY,
                    };
                    inv.equipped_jewels.push((j, loc));
                }
            }
        }
    }
    for (global, slots) in [(ITEM_BOX, 2300u32), (POUCH, 50)] {
        let p = unsafe { rd(base + global) };
        if !valid(p) {
            continue;
        }
        for i in 0..slots {
            let (item, count) = unsafe { (half(p + 0x1a + i * 4), half(p + 0x1c + i * 4)) };
            // Same check as Wishlist: skip broken slots
            if item == 0 || item >= 2992 || count > 99 {
                continue;
            }
            *inv.items.entry(item).or_insert(0) += count as u32;
        }
    }
    inv.smithy = smithy_unlock(base);
    inv.loaded = true;
    Some(inv)
}

/// 'Obtained at least once' item bitset: [base + OBTAINED] + 0x9C, bit = game item number (0x178 bytes).
/// Set by the item box add code (0x1943D4), read by the smithy list code (0x19443C).
const OBTAINED: u32 = 0x18583a8;
/// Made-before bitsets: [base + MADE] + 0x16C8 + (part - 1) * 0x360, bit = recipe index * 4 + variant (0x5249F4)
const MADE: u32 = 0x1897f78;
/// Game functions for an armor recipe's open condition (mDispFlag): does it pass, and does it ask for the key materials
const COND_OPEN: u32 = 0x5615bc;
const COND_NEEDS_KEYS: u32 = 0x561838;

/// The game state the smithy lists depend on
pub struct SmithyMem {
    obtained: u32,
    made: u32,
    open: extern "C" fn(u32) -> u32,
    needs_keys: extern "C" fn(u32) -> u32,
}

impl SmithyMem {
    /// None if the game objects are not there
    pub fn read(base: u32) -> Option<Self> {
        let obtained = unsafe { rd(base + OBTAINED) };
        let made = unsafe { rd(base + MADE) };
        if !valid(obtained) || !valid(made) {
            return None;
        }
        let open = unsafe { core::mem::transmute::<u32, extern "C" fn(u32) -> u32>(base + COND_OPEN) };
        let needs_keys = unsafe { core::mem::transmute::<u32, extern "C" fn(u32) -> u32>(base + COND_NEEDS_KEYS) };
        Some(Self { obtained, made, open, needs_keys })
    }
    /// Was this game item obtained at least once
    pub fn got(&self, item: u16) -> bool {
        item < 0x178 * 8 && unsafe { byte(self.obtained + 0x9c + item as u32 / 8) } >> (item % 8) & 1 == 1
    }
    /// Decoration recipe (its key materials) listed
    pub fn deco_shown(&self, keys: &[u16]) -> bool {
        keys.iter().all(|&k| self.got(k))
    }
    /// Armor recipe variant listed (game 0x6FA928): made before, or the open condition passes and, if that condition asks
    /// for it, every key material was obtained. `conds` remembers the condition answers (they only read game state).
    pub fn armor_shown(&self, part: u8, bit: u16, cond: u16, keys: &[u16], conds: &mut alloc::collections::BTreeMap<u16, (bool, bool)>) -> bool {
        let off = 0x16c8 + (part as u32 - 1) * 0x360 + bit as u32 / 8;
        let made = bit < 0x360 * 8 && unsafe { byte(self.made + off) } >> (bit % 8) & 1 == 1;
        made || {
            let (pass, ask) = *conds.entry(cond).or_insert_with(|| ((self.open)(cond as u32) & 0xff == 1, (self.needs_keys)(cond as u32) & 0xff == 1));
            pass && (!ask || keys.iter().all(|&k| self.got(k)))
        }
    }
}

/// What the smithy lists right now, computed the same way as the game's list code (docs/smithy-deco-unlock-analysis.md):
/// decorations whose key materials were all obtained once; armor made before, or whose open condition passes and
/// (when the condition asks for it) whose key materials were all obtained once.
/// Runs on the game thread when the window opens. None if the game objects are not there.
fn smithy_unlock(base: u32) -> Option<crate::db::Smithy> {
    use crate::generated::unlock::{ARMOR_KEYS, DECO_KEYS};
    let m = SmithyMem::read(base)?;
    let mut out = crate::db::Smithy::default();
    for &(jewel, keys) in DECO_KEYS {
        if m.deco_shown(keys) {
            out.jewels.insert(jewel);
            out.deco_recipes += 1;
        }
    }
    let mut conds = alloc::collections::BTreeMap::new();
    for &(armor, part, bit, cond, keys) in ARMOR_KEYS {
        if !out.armor.contains(&armor) && m.armor_shown(part, bit, cond, keys, &mut conds) {
            out.armor.insert(armor);
        }
    }
    // FNV-1a over both lists
    let mut h: u64 = 0xcbf29ce484222325;
    for x in out.armor.iter().chain([&0u16]).chain(out.jewels.iter()) {
        for b in x.to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    out.sig = h;
    Some(out)
}
