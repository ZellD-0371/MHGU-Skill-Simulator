/// Skill tree ids are 1..=205. 0 means "none".
pub const NT: usize = 206;
pub const TREE_TORSO_UP: u16 = 1;
/// Secret skill: +10 gives skill points +2.
pub const TREE_SECRET: u16 = 204;
/// Talisman boost: +10 doubles charm skills.
pub const TREE_TALISMAN: u16 = 205;

/// Armor that needs arena coins to craft
pub const FLAG_ARENA: u8 = 1;
/// Armor that needs event (collaboration) coins to craft
pub const FLAG_EVENT_COIN: u8 = 2;
/// Bits 2-3 of the armor flags: deviant armor start rank (1 low, 2 high, 3 G; 0 = not deviant).
/// Deviant armor is upgraded from one piece up to G rank, so it is usable from that rank on. Never put in exclude_flags.
pub const DEVIANT_SHIFT: u8 = 2;

pub const PARTS: usize = 5;
/// Number of elemental resistances (fire, water, thunder, ice, dragon)
pub const RES: usize = 5;
pub const TORSO: usize = 1;
/// Places for decorations: 5 armor + weapon + charm.
pub const CONTAINERS: usize = 7;
pub const WEAPON: usize = 5;
pub const CHARM: usize = 6;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pt {
    pub tree: u16,
    pub pt: i8,
}

#[derive(Clone, Copy, Debug)]
pub struct Armor {
    pub id: u16,
    /// 0 head, 1 torso, 2 arms, 3 waist, 4 legs
    pub part: u8,
    /// 0 both, 1 blademaster, 2 gunner
    pub kind: u8,
    /// 0 both, 1 male, 2 female
    pub sex: u8,
    pub rare: u8,
    pub slots: u8,
    /// FLAG_ARENA, FLAG_EVENT_COIN, deviant start rank (DEVIANT_SHIFT)
    pub flags: u8,
    /// Fully upgraded defense
    pub def: u16,
    /// tree 0 is an empty slot
    pub skills: [Pt; 4],
    /// Elemental resistance: fire, water, thunder, ice, dragon
    pub res: [i8; RES],
}

impl Armor {
    pub fn torso_up(&self) -> bool {
        self.skills.iter().any(|s| s.tree == TREE_TORSO_UP && s.pt > 0)
    }
    /// Torso-up parts are treated as having no slots (all 0 in the real data too).
    pub fn usable_slots(&self) -> u8 {
        if self.torso_up() { 0 } else { self.slots }
    }
    /// Deviant armor start rank (1 low, 2 high, 3 G), 0 for other armor
    pub fn deviant_rank(&self) -> u8 {
        self.flags >> DEVIANT_SHIFT & 3
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Jewel {
    pub id: u16,
    pub size: u8,
    /// Rank where it becomes craftable: 1 low, 2 high, 3 G
    pub rank: u8,
    pub pos: Pt,
    /// tree 0 means no side effect
    pub neg: Pt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Charm {
    pub skills: [Pt; 2],
    pub slots: u8,
}

/// Generation table of one charm kind.
#[derive(Clone, Debug)]
pub struct CharmKind {
    /// A second skill is added when the roll (0-99) is at or above this value.
    pub second_threshold: u8,
    /// First skill candidates. lo/hi = min/max points
    pub first: alloc::vec::Vec<CharmSkill>,
    /// Second skill candidates. lo = max magnitude when negative, hi = max positive
    pub second: alloc::vec::Vec<CharmSkill>,
    /// Slot roll thresholds for fill values 1-20
    pub slots: [[u8; 3]; 20],
}

#[derive(Clone, Copy, Debug)]
pub struct CharmSkill {
    pub tree: u16,
    pub lo: u8,
    pub hi: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Skill {
    pub id: u16,
    pub tree: u16,
    pub threshold: i8,
}
