//! Screen drawing. The preview preview/render.py 'native' style ported with the same coordinates (1280x720 base).
use crate::{
    app::*,
    generated::{atlas, strings as k},
    names::{self, Names},
    set::{CHARM, Set, WEAPON},
};
use alloc::{format, string::String, vec::Vec};
use forge::{
    imgui::{self, DrawList, ImFont},
    sys::imgui::{self as sys, ImDrawFlags, ImTextureRef, ImVec2},
};
use skillsim_engine::types::*;

// ---- Colors (native style, 0xRRGGBB) ----
const BG: u32 = 0x161a1c;
const PANEL: u32 = 0x1b2226;
const LINE: u32 = 0x36464c;
const TEXT: u32 = 0xebece9;
const MUTED: u32 = 0x98a3a6;
const GOLD: u32 = 0xf8e66c;
/// Game window_sub corner picture (top-left): rows of (x, y, length, white). Same pixels as the Better-Skillinfo skin.
const CORNER: &[(u8, u8, u8, bool)] = &[(2, 0, 10, false), (2, 1, 10, false), (0, 2, 2, false), (2, 2, 10, true), (12, 2, 2, false), (0, 3, 2, false), (2, 3, 10, true), (12, 3, 2, false), (0, 4, 2, false), (2, 4, 2, true), (4, 4, 10, false), (0, 5, 2, false), (2, 5, 2, true), (4, 5, 10, false), (0, 6, 2, false), (2, 6, 2, true), (4, 6, 2, false), (6, 6, 6, true), (12, 6, 2, false), (0, 7, 2, false), (2, 7, 2, true), (4, 7, 2, false), (6, 7, 6, true), (12, 7, 2, false), (0, 8, 2, false), (2, 8, 2, true), (4, 8, 6, false), (10, 8, 2, true), (12, 8, 2, false), (0, 9, 2, false), (2, 9, 2, true), (4, 9, 6, false), (10, 9, 2, true), (12, 9, 2, false), (0, 10, 2, false), (2, 10, 10, true), (12, 10, 2, false), (0, 11, 2, false), (2, 11, 10, true), (12, 11, 2, false), (2, 12, 10, false), (2, 13, 10, false)];
const GREEN: u32 = 0x9fd49a;
/// 'Craftable' tag: a different color from 'owned' (green)
const CRAFT: u32 = 0x86c5e8;
const RED: u32 = 0xf08a7a;
const SELECT: u32 = 0x2c3a40;
const POPUP: u32 = 0x1f282d;
const TEAL: u32 = 0x2b7f86;
const RED_TONES: [u32; 4] = [0x8a3635, 0xa92927, 0xb62220, 0xa52e1c];
const RED_UNDER: [u32; 3] = [0x857421, 0x648914, 0x598a25];
const RARE_COLORS: [u32; 11] = [0xe6e6e6, 0xb092f2, 0xded55e, 0xe88e9e, 0x6fc773, 0x708ef8, 0xda565a, 0x4c9eac, 0xee8c5d, 0xff45bd, 0x7d00ff];
const RES_COLORS: [u32; 5] = [0xe8746a, 0x6aa8e8, 0xe8d24a, 0x8fd8e8, 0xb48ae8];
const LAYOUTS: [&str; 3] = ["Nintendo", "Xbox", "PlayStation"];

fn abgr(rgb: u32, alpha: u8) -> u32 {
    ((alpha as u32) << 24) | ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff)
}
fn rare_color(rare: u8) -> u32 {
    RARE_COLORS[(rare.clamp(1, 11) - 1) as usize]
}

// ---- Resources: font, icon atlas, names ----
static mut FONT: Option<ImFont> = None;
/// The window around everything (x0, y0, x1, y1)
const WIN: (f32, f32, f32, f32) = (30., 6., 1250., 714.);
/// Every text is this much larger than the preview size (user request: it looked a bit small on the game screen)
const TEXT_BUMP: f32 = 1.5;
static mut ATLAS: Option<ImTextureRef> = None;
static mut NAMES: Option<Names> = None;

pub fn init() {
    unsafe {
        FONT = ImFont::load(&crate::res::path("font.otf"));
        if (*core::ptr::addr_of!(ATLAS)).is_none() {
            ATLAS = load_atlas();
        }
    }
}

/// The icon atlas is read straight into the ImGui texture's pixels: reading it into a buffer first and copying
/// raised our heap peak by 3.4 MB while the window first opened (measured 2026-11-19).
fn load_atlas() -> Option<ImTextureRef> {
    let len = (atlas::WIDTH * atlas::HEIGHT * 4) as usize;
    let mut tex: *mut sys::ImTextureData = core::ptr::null_mut();
    // The texture is made only after the file size matched
    let ok = crate::res::read_into("atlas.rgba", len, || unsafe {
        tex = sys::ImTextureData_ImTextureData();
        if tex.is_null() {
            return None;
        }
        sys::ImTextureData_Create(tex, sys::ImTextureFormat::RGBA32, atlas::WIDTH as i32, atlas::HEIGHT as i32);
        let pixels = sys::ImTextureData_GetPixels(tex);
        (!pixels.is_null()).then_some(pixels as *mut u8)
    });
    unsafe {
        if !ok {
            if !tex.is_null() {
                sys::ImTextureData_destroy(tex);
            }
            return None;
        }
        sys::igRegisterUserTexture(tex);
        Some(sys::ImTextureData_GetTexRef(tex))
    }
}

/// Reload the names when the language changes.
fn names(lang: usize) -> Option<&'static Names> {
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(NAMES);
        if slot.as_ref().is_none_or(|n| n.language != lang) {
            *slot = Names::load(lang).or_else(|| Names::load(0));
        }
        slot.as_ref()
    }
}

pub fn ready() -> bool {
    unsafe { (*core::ptr::addr_of!(FONT)).is_some() }
}

// ---- Strings ----
fn t(app: &App, key: u16) -> &'static str {
    k::TABLES[app.language.min(8)].get(key as usize).copied().unwrap_or("?")
}
fn tl(app: &App, key: u16, i: usize) -> &'static str {
    t(app, key).split('\u{1f}').nth(i).unwrap_or("?")
}
fn fmt1(s: &str, a: impl core::fmt::Display) -> String {
    s.replacen("{}", &format!("{}", a), 1)
}
fn fmt2(s: &str, a: impl core::fmt::Display, b: impl core::fmt::Display) -> String {
    fmt1(&fmt1(s, a), b)
}

// ---- Draw cache ----
/// One draw command in screen coordinates. On frames where the screen did not change, last frame's commands are re-issued as they are
/// (Recomp interprets the plugin, and drawing the results screen from scratch took about 1.9 ms per frame: 2026-11-19).
enum Op {
    Fill(ImVec2, ImVec2, u32, f32),
    Stroke(ImVec2, ImVec2, u32, f32, f32),
    Line(ImVec2, ImVec2, u32, f32),
    Disc(ImVec2, f32, u32),
    Ring(ImVec2, f32, u32, f32),
    /// Text: size (our font, 0 for the default font), position, color, contents
    Text(f32, ImVec2, u32, String),
    Image(ImVec2, ImVec2, ImVec2, ImVec2, u32),
}

fn exec(draw: &DrawList, op: &Op) {
    match op {
        Op::Fill(a, b, c, r) => draw.add_rect_filled(*a, *b, *c, *r),
        Op::Stroke(a, b, c, r, w) => draw.add_rect(*a, *b, *c, *r, *w, ImDrawFlags::None),
        Op::Line(a, b, c, w) => draw.add_line(*a, *b, *c, *w),
        Op::Disc(p, r, c) => draw.add_circle_filled(*p, *r, *c, 16),
        Op::Ring(p, r, c, w) => draw.add_circle(*p, *r, *c, 16, *w),
        Op::Text(size, pos, c, s) => match unsafe { FONT } {
            Some(f) if *size > 0. => draw.add_text_with_font(&f, *size, *pos, *c, s),
            _ => draw.add_text(*pos, *c, s),
        },
        Op::Image(a, b, uv0, uv1, c) => {
            if let Some(tex) = unsafe { ATLAS } {
                draw.add_image(tex, *a, *b, *uv0, *uv1, *c);
            }
        }
    }
}

/// Last frame's draw commands with the screen state of that time (App::rev, size/position) and when they were made
struct DrawCache {
    key: (u64, u32, u32, u32),
    at: u64,
    ops: Vec<Op>,
    hits: Vec<([f32; 4], Target)>,
}
/// Last mouse position (screen pixels): hovering moves the cursor only when the mouse moves
static mut LAST_MOUSE: (f32, f32) = (f32::MIN, f32::MIN);
/// Drawing happens only on the game thread
static mut DRAW_CACHE: Option<DrawCache> = None;
/// Even if a change missed the state number, redraw from scratch at least this often
const DRAW_REFRESH_MS: u64 = 250;

// ---- Canvas: fits 1280x720 coordinates to the screen ----
struct Cv {
    draw: DrawList,
    s: f32,
    ox: f32,
    oy: f32,
    /// The game's picture in the window (the canvas sits in it; Recomp's mouse is relative to it)
    pic: crate::picture::Area,
    font: Option<ImFont>,
    /// Draw commands issued this frame (so the next frame can re-issue them)
    rec: core::cell::RefCell<Vec<Op>>,
    /// Mouse targets drawn this frame: (canvas rectangle, target)
    hits: core::cell::RefCell<Vec<([f32; 4], Target)>>,
    /// Clickable control under the mouse (App::hover)
    hover: Option<Target>,
}

#[derive(Clone, Copy, PartialEq)]
enum Anchor {
    LA,
    LM,
    MM,
    MA,
    RA,
    RM,
}
use Anchor::*;

impl Cv {
    fn new(app: &App) -> Option<Cv> {
        // Always draw on the background layer: the Forge menu and other mods' ImGui windows (Better Skillinfo panel etc.) show above ours
        let draw = DrawList::background();
        if draw.raw().is_null() {
            return None;
        }
        let io = imgui::get_io();
        let (w, h) = (io.DisplaySize.x, io.DisplaySize.y);
        if w < 64. || h < 64. {
            return None;
        }
        // Fit into the game's picture, not the whole window (black bars when their shapes differ: picture.rs)
        let pic = crate::picture::area(w, h);
        // Fixed size and place: on the right, leaving the left strip for Better-Skillinfo's panel
        let s = (pic.w / 1280.).min(pic.h / 720.) * UI_SCALE;
        let ox = pic.left + pic.w - 1280. * s - pic.w * 0.02;
        let oy = pic.top + (pic.h - 720. * s) * 0.5;
        Some(Cv { draw, s, ox, oy, pic, font: unsafe { FONT }, rec: core::cell::RefCell::new(Vec::with_capacity(512)), hits: Default::default(), hover: app.hover })
    }
    fn put(&self, op: Op) {
        exec(&self.draw, &op);
        self.rec.borrow_mut().push(op);
    }
    fn p(&self, x: f32, y: f32) -> ImVec2 {
        ImVec2 { x: self.ox + x * self.s, y: self.oy + y * self.s }
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, col: u32) {
        self.put(Op::Fill(self.p(x0, y0), self.p(x1, y1), abgr(col, 255), 0.));
    }
    fn rect_a(&self, x0: f32, y0: f32, x1: f32, y1: f32, col: u32, a: u8) {
        self.put(Op::Fill(self.p(x0, y0), self.p(x1, y1), abgr(col, a), 0.));
    }
    fn round(&self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, col: u32) {
        self.put(Op::Fill(self.p(x0, y0), self.p(x1, y1), abgr(col, 255), r * self.s));
    }
    fn outline(&self, x0: f32, y0: f32, x1: f32, y1: f32, col: u32) {
        self.put(Op::Stroke(self.p(x0, y0), self.p(x1, y1), abgr(col, 255), 0., self.s.max(1.)));
    }
    fn round_outline(&self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32, col: u32) {
        self.put(Op::Stroke(self.p(x0, y0), self.p(x1, y1), abgr(col, 255), r * self.s, self.s.max(1.)));
    }
    fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, col: u32, w: f32) {
        self.put(Op::Line(self.p(x0, y0), self.p(x1, y1), abgr(col, 255), (w * self.s).max(1.)));
    }
    fn circle(&self, x: f32, y: f32, r: f32, col: u32, filled: bool) {
        if filled {
            self.put(Op::Disc(self.p(x, y), r * self.s, abgr(col, 255)));
        } else {
            self.put(Op::Ring(self.p(x, y), r * self.s, abgr(col, 255), self.s.max(1.)));
        }
    }

    fn measure(&self, size: f32, s: &str) -> f32 {
        let size = size + TEXT_BUMP;
        let Some(font) = self.font else { return s.len() as f32 * size * 0.5 };
        let px = size * self.s;
        let v = unsafe { sys::ImFont_CalcTextSizeA(font.raw(), px, f32::MAX, 0., s.as_ptr().cast(), s.as_ptr().add(s.len()).cast(), core::ptr::null_mut()) };
        v.x / self.s
    }

    fn fit(&self, s: &str, size: f32, width: f32) -> String {
        if self.measure(size, s) <= width {
            return s.into();
        }
        let mut out = String::from(s);
        while !out.is_empty() && self.measure(size, &format!("{}…", out)) > width {
            out.pop();
        }
        out.push('…');
        out
    }

    /// Preview text(): anchor follows PIL (la left-top, mm middle, ra right-top ...)
    fn text(&self, x: f32, y: f32, s: &str, size: f32, col: u32, width: Option<f32>, anchor: Anchor) {
        let s = match width {
            Some(w) => self.fit(s, size, w),
            None => s.into(),
        };
        let w = self.measure(size, &s);
        let size = size + TEXT_BUMP;
        let x = match anchor {
            LA | LM => x,
            MM | MA => x - w * 0.5,
            RA | RM => x - w,
        };
        // Raise slightly so the glyph body is centered in the font's line height
        // Match the preview (PIL) baseline: for top anchors, lower slightly so the top of the glyph body sits at y
        let y = match anchor {
            LM | MM | RM => y - size * 0.60,
            _ => y + size * 0.10,
        };
        let pos = self.p(x, y);
        let px = if self.font.is_some() { size * self.s } else { 0. };
        self.put(Op::Text(px, pos, abgr(col, 255), s));
    }
    fn tx(&self, x: f32, y: f32, s: &str, size: f32, col: u32) {
        self.text(x, y, s, size, col, None, LA);
    }

    fn sprite(&self, name: &str, x: f32, y: f32, size: f32, tint: u32, flip: bool) -> bool {
        if unsafe { ATLAS }.is_none() {
            return false;
        }
        let Some(i) = atlas::NAMES.iter().position(|&n| n == name) else { return false };
        let (cx, cy) = ((i as u32 % atlas::COLS) * atlas::CELL, (i as u32 / atlas::COLS) * atlas::CELL);
        let (w, h) = (atlas::WIDTH as f32, atlas::HEIGHT as f32);
        let (mut u0, v0, mut u1, v1) = (cx as f32 / w, cy as f32 / h, (cx + atlas::CELL) as f32 / w, (cy + atlas::CELL) as f32 / h);
        if flip {
            core::mem::swap(&mut u0, &mut u1);
        }
        self.put(Op::Image(self.p(x, y), self.p(x + size, y + size), ImVec2 { x: u0, y: v0 }, ImVec2 { x: u1, y: v1 }, abgr(tint, 255)));
        true
    }

    // ---- Game window look ----
    fn frame(&self, x0: f32, y0: f32, x1: f32, y1: f32, tones: [u32; 4], under: &[u32]) {
        self.rect(x0, y0 + 8., x0 + 3., y1 - 8., 0x000000);
        self.rect(x1 - 3., y0 + 8., x1, y1 - 8., 0x000000);
        // Double lines top and bottom: colored lines over a black band
        for (i, &c) in tones.iter().enumerate() {
            let yy = 2. + i as f32;
            self.line(x0 + 8., y0 + yy, x1 - 8., y0 + yy, c, 1.);
            self.line(x0 + 8., y1 - yy - 1., x1 - 8., y1 - yy - 1., c, 1.);
        }
        self.line(x0 + 8., y0 + 1., x1 - 8., y0 + 1., 0x000000, 1.);
        self.line(x0 + 8., y1 - 2., x1 - 8., y1 - 2., 0x000000, 1.);
        for (i, &c) in under.iter().enumerate() {
            let yy = 6. + i as f32;
            self.line(x0 + 4., y0 + yy, x1 - 4., y0 + yy, c, 1.);
            self.line(x0 + 4., y1 - yy - 1., x1 - 4., y1 - yy - 1., c, 1.);
        }
        // Four corner ornaments: the game's window_sub corner picture, mirrored per corner (white parts gold)
        for flip_y in [false, true] {
            for flip_x in [false, true] {
                for &(rx, ry, rw, white) in CORNER {
                    let (rx, ry, rw) = (rx as f32, ry as f32, rw as f32);
                    let px = if flip_x { x1 - rx - rw } else { x0 + rx };
                    let py = if flip_y { y1 - ry - 1. } else { y0 + ry };
                    self.rect(px, py, px + rw, py + 1., if white { GOLD } else { 0x000000 });
                }
            }
        }
    }
    fn panel(&self, x0: f32, y0: f32, x1: f32, y1: f32, title: Option<&str>, red: bool) {
        self.rect_a(x0, y0, x1, y1, PANEL, 245);
        if red {
            self.frame(x0, y0, x1, y1, RED_TONES, &RED_UNDER);
        } else {
            self.frame(x0, y0, x1, y1, [TEAL; 4], &[]);
        }
        if let Some(title) = title {
            self.tx(x0 + 20., y0 + 12., title, 18., GOLD);
            self.line(x0 + 14., y0 + 40., x1 - 14., y0 + 40., LINE, 1.);
        }
    }
    fn popup(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.rect(x0, y0, x1, y1, POPUP);
        self.frame(x0, y0, x1, y1, [TEAL; 4], &[]);
    }
    /// A selectable row: the cursor when selected, and a mouse target either way
    fn row(&self, x0: f32, y0: f32, x1: f32, y1: f32, selected: bool, t: Target) {
        if selected {
            self.cursor(x0, y0, x1, y1);
        }
        self.hit(x0, y0, x1, y1, t);
    }
    /// A mouse target without a cursor. A tab / pager arrow / button prompt under the mouse gets a gold outline.
    fn hit(&self, x0: f32, y0: f32, x1: f32, y1: f32, t: Target) {
        if self.hover == Some(t) {
            self.round_outline(x0, y0, x1, y1, 6., GOLD);
        }
        self.hits.borrow_mut().push(([x0, y0, x1, y1], t));
    }
    fn cursor(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.rect(x0, y0, x1, y1, SELECT);
        for (cx, sx) in [(x0, 1.), (x1, -1.)] {
            for (cy, sy) in [(y0, 1.), (y1, -1.)] {
                self.line(cx, cy, cx + 9. * sx, cy, GOLD, 2.);
                self.line(cx, cy, cx, cy + 9. * sy, GOLD, 2.);
            }
        }
    }
    fn checkbox(&self, x: f32, y: f32, on: bool) {
        self.outline(x, y, x + 18., y + 18., TEXT);
        if on {
            self.rect(x + 4., y + 4., x + 14., y + 14., GOLD);
        }
    }
    fn slots(&self, x: f32, y: f32, total: u8, used: u8) {
        for i in 0..total {
            let cx = x + i as f32 * 18. + 6.;
            if i < used {
                self.circle(cx, y + 6., 6., GOLD, true);
            }
            self.circle(cx, y + 6., 6., TEXT, false);
        }
    }
    fn button(&self, app: &App, b: Button, x: f32, y: f32, size: f32, dim: bool) {
        // Follow the Recomp button layout (input arrives with that layout too)
        let b = match crate::prompts::get(b) {
            crate::prompts::Prompt::Original => b,
            crate::prompts::Prompt::Button(to) => to,
            crate::prompts::Prompt::Text(label) => {
                self.round_outline(x + 2., y + 4., x + size - 2., y + size - 4., 8., if dim { MUTED } else { TEXT });
                self.text(x + size / 2., y + size / 2. - 1., &label, 15., if dim { MUTED } else { TEXT }, Some(size - 6.), MM);
                return;
            }
            // Keyboard / mouse: the key's picture (Kenney prompts), or its short name when there is none
            crate::prompts::Prompt::Key(key) => {
                if !self.sprite(&crate::prompts::key_sprite(&key), x, y, size, if dim { 0x6a6f73 } else { 0xffffff }, false) {
                    let label = crate::prompts::key_label(&key);
                    self.round_outline(x + 2., y + 4., x + size - 2., y + size - 4., 6., if dim { MUTED } else { TEXT });
                    self.text(x + size / 2., y + size / 2. - 1., &label, 13., if dim { MUTED } else { TEXT }, Some(size - 6.), MM);
                }
                return;
            }
        };
        let key = match b {
            Button::A => "a",
            Button::B => "b",
            Button::X => "x",
            Button::Y => "y",
            Button::L => "l",
            Button::R => "r",
            Button::ZL => "zl",
            Button::ZR => "zr",
            Button::Plus => "plus",
            Button::Minus => "minus",
            _ => "dpad",
        };
        let name = format!("{}-{}", LAYOUTS[app.layout.min(2)], key);
        if !self.sprite(&name, x, y, size, if dim { 0x6a6f73 } else { 0xffffff }, false) {
            self.round_outline(x + 2., y + 4., x + size - 2., y + size - 4., 8., TEXT);
            let label = match b {
                Button::Plus => "+",
                Button::Minus => "−",
                _ => key,
            };
            self.text(x + size / 2., y + size / 2. - 1., label, 17., TEXT, None, MM);
        }
    }
    fn armor_icon(&self, a: &Armor, x: f32, y: f32, size: f32) {
        self.sprite(&format!("part-{}", a.part), x, y, size, rare_color(a.rare), false);
    }
    fn charm_icon(&self, rare: u8, x: f32, y: f32, size: f32) {
        self.sprite("charm", x, y, size, if rare > 0 { rare_color(rare) } else { 0xffffff }, false);
    }
    /// Page number with left/right arrows; the arrows are mouse targets of the given layer (0 list, 1 popup)
    fn pager(&self, app: &App, cx: f32, y: f32, total: usize, rows: usize, top: usize, layer: u8) {
        let pages = total.div_ceil(rows).max(1);
        let label = format!("{} / {}", top / rows + 1, pages);
        let w = self.measure(18., &label);
        self.text(cx, y + 13., &label, 18., if pages > 1 { TEXT } else { MUTED }, None, MM);
        if pages > 1 {
            let (lx, rx) = (cx - w / 2. - 40., cx + w / 2. + 12.);
            if crate::prompts::kbm() {
                // Keyboard / mouse: the keys bound to the d-pad (clicking the arrow works too)
                self.button(app, Button::Left, lx, y, 28., false);
                self.button(app, Button::Right, rx, y, 28., false);
            } else {
                let key = format!("{}-dpadleft", LAYOUTS[app.layout.min(2)]);
                self.sprite(&key, lx, y, 28., 0xffffff, false);
                self.sprite(&key, rx, y, 28., 0xffffff, true);
            }
            // Generous click areas (the label in between stays out)
            self.hit(lx - 10., y - 6., lx + 32., y + 34., Target::Page(false, layer));
            self.hit(rx - 4., y - 6., rx + 38., y + 34., Target::Page(true, layer));
        }
    }
}

// ---- Names ----
fn nm(app: &App, table: usize, id: u16) -> &'static str {
    names(app.language).map_or("?", |n| n.get(table, id))
}
fn skill_name(app: &App, id: u16) -> &'static str {
    nm(app, names::SKILL, id)
}
fn tree_name(app: &App, tree: u16) -> &'static str {
    nm(app, names::TREE, tree)
}
fn charm_text(app: &App, c: &Charm) -> String {
    let mut s = String::new();
    for p in c.skills.iter().filter(|p| p.tree != 0) {
        if !s.is_empty() {
            s.push_str("  ");
        }
        s.push_str(&format!("{} {:+}", tree_name(app, p.tree), p.pt));
    }
    s
}
fn odds(app: &App, chance: f64) -> String {
    if chance <= 0. {
        return "—".into();
    }
    let n = 1. / chance;
    // East Asian languages count in 10^4 / 10^8 units; the others write the whole number
    let units = match app.language {
        1 => ("亿", "万"),
        2 => ("億", "萬"),
        3 => ("億", "万"),
        4 => ("억", "만"),
        lang => {
            let word = ["in", "", "", "", "", "zu", "sur", "su", "entre"][lang.min(8)];
            return format!("1 {} {}", word, n as u64);
        }
    };
    if n >= 1e8 {
        format!("1/{:.1}{}", n / 1e8, units.0)
    } else if n >= 1e4 {
        format!("1/{:.0}{}", n / 1e4, units.1)
    } else {
        format!("1/{:.0}", n)
    }
}

// ---- Draw entry point ----
/// An invisible ImGui window covering the whole screen while our window is open: ImGui takes the mouse so
/// Recomp's mouse menu input (MenuMouse) does not reach the game menu behind.
pub fn mouse_shield() {
    use forge::sys::imgui::{ImGuiCond, ImGuiWindowFlags};
    let io = imgui::get_io();
    imgui::set_next_window_pos(ImVec2 { x: 0., y: 0. }, ImGuiCond::Always, ImVec2 { x: 0., y: 0. });
    imgui::set_next_window_size(ImVec2 { x: io.DisplaySize.x, y: io.DisplaySize.y }, ImGuiCond::Always);
    let flags = ImGuiWindowFlags::NoTitleBar
        | ImGuiWindowFlags::NoResize
        | ImGuiWindowFlags::NoMove
        | ImGuiWindowFlags::NoSavedSettings
        | ImGuiWindowFlags::NoScrollbar
        | ImGuiWindowFlags::NoScrollWithMouse
        | ImGuiWindowFlags::NoBackground
        | ImGuiWindowFlags::NoNav
        | ImGuiWindowFlags::NoFocusOnAppearing
        | ImGuiWindowFlags::NoBringToFrontOnFocus;
    imgui::begin(c"##skill-sim-mouse-shield", None, flags);
    imgui::end();
}

/// Our window's frame and cursor view as last drawn (screen pixels), None if not drawn: for the shared block (window_view)
static mut SHARE: Option<([f32; 4], crate::window_view::View)> = None;
pub fn shared_view() -> Option<([f32; 4], crate::window_view::View)> {
    unsafe { (*core::ptr::addr_of_mut!(SHARE)).take() }
}
/// Same for the guest-mod block (ext_block)
static mut EXT: Option<crate::ext_block::Ext> = None;
pub fn shared_ext() -> Option<crate::ext_block::Ext> {
    unsafe { (*core::ptr::addr_of_mut!(EXT)).take() }
}

pub fn render(app: &mut App) -> Vec<Button> {
    let cv = Cv::new(app);
    let share = cv.as_ref().map(|cv| {
        let (a, b) = (cv.p(WIN.0, WIN.1), cv.p(WIN.2, WIN.3));
        ([a.x, a.y, b.x, b.y], share_view(cv, app))
    });
    unsafe { *core::ptr::addr_of_mut!(SHARE) = share };
    let ext = cv.as_ref().map(|cv| ext_view(cv, app));
    unsafe { *core::ptr::addr_of_mut!(EXT) = ext };
    let Some(cv) = cv else { return Vec::new() };
    let hits = draw_cached(&cv, app);
    mouse(&cv, app, &hits)
}

/// What a guest mod (Wishlist) may act on: the set or the part under the cursor on the results / saved-set screens
fn ext_view(cv: &Cv, app: &App) -> crate::ext_block::Ext {
    use crate::ext_block::*;
    let db = app.db;
    let left = cv.p(hints_left(), 658.);
    let bottom = cv.p(0., 698.);
    let mut e = Ext { hints_left: left.x, hint_top: left.y, hint_bottom: bottom.y, gap: 26. * cv.s, scale: cv.s, ..Ext::default() };
    e.screen = match app.screen {
        Screen::Main => 1,
        Screen::Skills => 2,
        Screen::Results => 3,
        Screen::Saved => 4,
        Screen::Manage => 5,
        Screen::Box => 6,
    };
    let overlay = app.dialog().is_some()
        || app.extra.is_some()
        || app.extra_busy()
        || app.popup.is_some()
        || app.compare
        || app.fixmenu.is_some()
        || app.exmenu.is_some()
        || app.drop.is_some()
        || app.sub.is_some()
        || app.subdrop.is_some();
    e.overlay = overlay as u32;
    if !matches!(app.screen, Screen::Results | Screen::Saved) || overlay {
        return e;
    }
    let Some(cur) = app.current() else { return e };
    for p in 0..PARTS {
        let id = cur.head(p);
        if let Some((part, series)) = crate::inventory::armor_game(id) {
            let mut flags = 0;
            if db.inv().owned.contains(&id) {
                flags |= FLAG_OWNED;
            }
            if db.reward_armor.contains(&id) {
                flags |= FLAG_REWARD;
            }
            e.pieces[p] = Piece { series, part, flags };
        }
    }
    if app.focus == Focus::List {
        e.kind = KIND_SET;
    } else if app.pidx < PARTS && e.pieces[app.pidx].series != 0 {
        e.kind = KIND_PART;
        e.part = app.pidx as u32 + 1;
    }
    e
}

/// What the cursor points at, in the game's ids, for other mods (window_view, version 3). Facts only: the reader decides what to show.
fn share_view(cv: &Cv, app: &App) -> crate::window_view::View {
    use crate::window_view::*;
    let db = app.db;
    let rect = |area: u32, r: [f32; 4]| {
        let (a, b) = (cv.p(r[0], r[1]), cv.p(r[2], r[3]));
        (area, [a.x, a.y, b.x, b.y])
    };
    // The areas drawn on this screen (canvas coordinates as in the draw functions)
    let rects = match app.screen {
        Screen::Main => alloc::vec![
            rect(AREA_TARGETS, [60., 70., 436., 610.]),
            rect(AREA_CONDITIONS, [452., 70., 828., 610.]),
            rect(AREA_FIXED, [844., 70., 1220., 610.]),
        ],
        Screen::Results | Screen::Saved => alloc::vec![rect(AREA_SET_LIST, [60., 70., 440., 610.]), rect(AREA_SET_DETAIL, [460., 70., 1220., 610.])],
        Screen::Skills | Screen::Manage | Screen::Box => alloc::vec![rect(AREA_LIST, [60., 70., 1220., 610.])],
    };
    let mut v = View { rects, ..View::default() };
    // Something over the window: a dialog, menu, dropdown, the extra-skill list or a search in progress
    // Extra-skill list: the skill under its cursor (tree and that level's points)
    if app.extra.is_some() && !app.extra_busy() && app.dialog().is_none() {
        v.screen = SCREEN_MODAL;
        if let Some(s) = app.extra_list().get(app.eidx).and_then(|&id| db.skill(id)) {
            v.subject = SUBJECT_TREE;
            v.a = game_tree(s.tree) as u32;
            v.b = s.threshold as i32 as u32;
        }
        return v;
    }
    let modal = app.dialog().is_some()
        || app.extra.is_some()
        || app.extra_busy()
        || app.menu.is_some()
        || app.fixmenu.is_some()
        || app.exmenu.is_some()
        || app.drop.is_some()
        || app.sub.is_some()
        || app.subdrop.is_some();
    if modal {
        v.screen = SCREEN_MODAL;
        return v;
    }
    fn pts(list: &mut dyn Iterator<Item = (u16, i16)>) -> Vec<(u16, i16)> {
        let mut out: Vec<(u16, i16)> = Vec::new();
        for (tree, pt) in list {
            if tree == 0 || pt == 0 {
                continue;
            }
            let g = game_tree(tree);
            match out.iter_mut().find(|e| e.0 == g) {
                Some(e) => e.1 += pt,
                None => out.push((g, pt)),
            }
        }
        out.retain(|e| e.1 != 0);
        out.truncate(MAX_POINTS);
        out
    }
    // Decoration points in placement order
    let jewel_pts = |jewels: &[u16]| -> Vec<(u16, i16)> {
        jewels.iter().filter_map(|&j| db.jewel(j)).flat_map(|j| [(j.pos.tree, j.pos.pt as i16), (j.neg.tree, j.neg.pt as i16)]).collect()
    };
    // Armor: its own skills in the detail panel's 'Equipment skills' order, then the decorations placed in that part
    let armor = |v: &mut View, id: u16, jewels: &[u16]| {
        let Some((part, series)) = crate::inventory::armor_game(id) else { return };
        v.subject = SUBJECT_ARMOR;
        v.a = series as u32;
        v.b = part as u32;
        let own = db.armor(id).map(|a| a.skills.iter().map(|s| (s.tree, s.pt as i16)).collect::<Vec<_>>()).unwrap_or_default();
        v.points = pts(&mut own.into_iter().chain(jewel_pts(jewels)));
    };
    // Charm: its skills, then the decorations placed in it (a skill-less unowned charm still sends its decorations)
    let charm = |v: &mut View, c: Option<Charm>, jewels: &[u16]| {
        v.subject = SUBJECT_CHARM;
        v.a = c.map_or(0, |c| c.slots as u32);
        let own: Vec<(u16, i16)> = c.map(|c| c.skills.iter().map(|s| (s.tree, s.pt as i16)).collect()).unwrap_or_default();
        v.points = pts(&mut own.into_iter().chain(jewel_pts(jewels)));
    };
    // Set totals: the trees of the 'Active skills' list in its order, then the other trees
    let set_total = |v: &mut View, r: &crate::set::Set| {
        v.subject = SUBJECT_SET;
        let totals = r.totals(db);
        let mut order: Vec<u16> = db.active_skills(&totals).iter().map(|s| s.tree).collect();
        for &tree in totals.keys() {
            if !order.contains(&tree) {
                order.push(tree);
            }
        }
        v.points = pts(&mut order.into_iter().map(|tree| (tree, totals.get(&tree).copied().unwrap_or(0))));
    };
    // A skill: its tree and that level's points
    let tree_of = |v: &mut View, skill: u16| {
        if let Some(s) = db.skill(skill) {
            v.subject = SUBJECT_TREE;
            v.a = game_tree(s.tree) as u32;
            v.b = s.threshold as i32 as u32;
        }
    };
    match app.screen {
        Screen::Main => {
            v.screen = SCREEN_MAIN;
            v.area = [AREA_TARGETS, AREA_CONDITIONS, AREA_FIXED][app.col.min(2)];
            if app.col == 0 {
                if let Some(&id) = app.targets.get(app.row) {
                    tree_of(&mut v, id);
                }
            } else if app.col == 2 {
                if app.row < PARTS {
                    if app.fixed[app.row] != 0 {
                        armor(&mut v, app.fixed[app.row], &[]);
                    }
                } else if let Some(c) = app.fixed_charm.and_then(|i| db.inv().charms.get(i).copied()) {
                    charm(&mut v, Some(c), &[]);
                }
            }
        }
        Screen::Skills => {
            v.screen = SCREEN_SKILLS;
            v.area = AREA_LIST;
            if let Some(&id) = app.skill_list().get(app.sidx) {
                tree_of(&mut v, id);
            }
        }
        Screen::Results | Screen::Saved => {
            v.screen = if app.screen == Screen::Results { SCREEN_RESULTS } else { SCREEN_SAVED };
            let Some(cur) = app.current().cloned() else {
                v.area = AREA_SET_LIST;
                return v;
            };
            if app.focus == Focus::List {
                v.area = AREA_SET_LIST;
                set_total(&mut v, &cur);
            } else {
                v.area = AREA_SET_DETAIL;
                // The detail panel shows on the left: armor rows have two pages (ZL/ZR), charm and weapon one
                v.page = if app.pidx < PARTS { app.dpage as u32 + 1 } else { 1 };
                // With the alternatives popup open, the armor under the popup cursor (as the detail panel shows)
                let r = app.popup_preview().unwrap_or(cur);
                let (placed, _) = placement_cached(app, &r);
                if app.pidx < PARTS {
                    armor(&mut v, r.head(app.pidx), &placed[app.pidx]);
                } else if app.pidx == CHARM_ROW {
                    charm(&mut v, r.charm_data(db), &placed[CONTAINER[CHARM_ROW]]);
                } else {
                    // Weapon row: only the decorations in the weapon slots (count 0 without any)
                    v.subject = SUBJECT_WEAPON;
                    v.points = pts(&mut jewel_pts(&placed[CONTAINER[WEAPON_ROW]]).into_iter());
                }
            }
        }
        Screen::Manage => {
            v.screen = SCREEN_MANAGE;
            v.area = AREA_LIST;
            if let Some(&id) = app.excluded.get(app.midx) {
                armor(&mut v, id, &[]);
            }
        }
        Screen::Box => {
            v.screen = SCREEN_BOX;
            v.area = AREA_LIST;
            if let Some(&item) = app.box_items().get(app.bidx) {
                if app.box_slot == CHARM_ROW {
                    if let Some(c) = db.inv().charms.get(item as usize).copied() {
                        charm(&mut v, Some(c), &[]);
                    }
                } else {
                    armor(&mut v, item, &[]);
                }
            }
        }
    }
    v
}

/// Mouse on our window (Recomp only): pointing moves the cursor, left click = point + decide (A), right click = back (B),
/// clicking a tab / pager arrow / button prompt acts on it, wheel = page. Returns the buttons to press (handled like controller input, with sounds).
/// Recomp gives the mouse to ImGui only while its own Forge menu is open, so the pointer comes from Recomp's mod pointer
/// API instead (input::mouse): while our window is open the pointer is free and its clicks are ours only, so the
/// [Keybinds] mouse bindings (e.g. Mouse Left = X) do not reach the game or our window as buttons.
fn mouse(cv: &Cv, app: &mut App, hits: &[([f32; 4], Target)]) -> Vec<Button> {
    let mut out = Vec::new();
    if forge::graphics::Graphics::is_menu_visible() {
        return out;
    }
    let Some(m) = crate::input::mouse() else {
        if app.hover.take().is_some() {
            app.touch();
        }
        return out;
    };
    // Recomp's pointer is in console screen pixels (1280x720) over the game's picture, not the whole window
    let (mx, my) = cv.pic.mouse(m.x, m.y);
    let (x, y) = ((mx - cv.ox) / cv.s, (my - cv.oy) / cv.s);
    // Button prompts stay clickable under popups (they are the popup's own buttons too)
    let top = hits.iter().filter(|h| !matches!(h.1, Target::Hint(_))).map(|h| h.1.layer()).max();
    let hit = hits
        .iter()
        .rev()
        .find(|(r, t)| (matches!(t, Target::Hint(_)) || Some(t.layer()) == top) && x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
        .map(|h| h.1);
    // Highlight the clickable control (tab, pager arrow, button prompt) under the mouse
    let hover = hit.filter(|t| matches!(t, Target::Tab(_) | Target::ETab(_) | Target::Page(..) | Target::Hint(_)));
    if hover != app.hover {
        app.hover = hover;
        app.touch();
        if hover.is_some() {
            crate::sound::cue(crate::sound::Cue::Move); // entering a clickable control sounds like a cursor move
        }
    }
    let last = unsafe { core::ptr::addr_of_mut!(LAST_MOUSE).replace((mx, my)) };
    let moved = (last.0 - mx).abs() > 0.5 || (last.1 - my).abs() > 0.5;
    if let Some(t) = hit {
        if m.left {
            let before = app.rev;
            app.touch();
            let tab = (app.tab, app.etab);
            if let Some(b) = app.mouse_target(t, true) {
                out.push(b);
            } else if (app.tab, app.etab) != tab {
                crate::sound::cue(crate::sound::Cue::Tab);
            } else if app.rev != before && !matches!(t, Target::Tab(_) | Target::ETab(_)) {
                crate::sound::cue(crate::sound::Cue::Move);
            }
        } else if moved {
            let cursors = |a: &App| ((a.col, a.row, a.ridx, a.pidx, a.aidx, a.dialog_sel, a.sidx, a.eidx, a.midx, a.bidx), (a.drop, a.sub, a.subdrop, a.menu, a.fixmenu, a.exmenu));
            let snap = cursors(app);
            app.mouse_target(t, false);
            if snap != cursors(app) {
                app.touch();
                crate::sound::cue(crate::sound::Cue::Move);
            }
        }
    }
    if m.right {
        out.push(Button::B);
    }
    // Wheel turns the page of the topmost list that has pages (down = next); nothing elsewhere
    if m.wheel != 0 {
        if let Some(&(_, Target::Page(_, layer))) = hits.iter().rev().find(|h| matches!(h.1, Target::Page(..)) && Some(h.1.layer()) == top) {
            app.touch();
            if let Some(b) = app.mouse_target(Target::Page(m.wheel < 0, layer), true) {
                out.push(b);
            }
        }
    }
    out
}

/// Draw (or re-issue the cached draw) and return the mouse targets of this frame
fn draw_cached(cv: &Cv, app: &mut App) -> Vec<([f32; 4], Target)> {
    let cv = cv;
    // If the screen state (rev) and size/position match last frame, re-issue last frame's draw commands.
    // Not cached while a window that changes every frame (progress bar, elapsed time) is shown.
    let key = (app.rev, cv.s.to_bits(), cv.ox.to_bits(), cv.oy.to_bits());
    let animated = app.job.is_some() || app.job_out.is_some() || app.building.is_some() || app.diag_run.is_some() || app.diag_out.is_some() || app.extra_busy();
    let cache = unsafe { &mut *core::ptr::addr_of_mut!(DRAW_CACHE) };
    if !animated {
        if let Some(c) = cache.as_ref() {
            if c.key == key && crate::clock::ms_since(c.at) < DRAW_REFRESH_MS {
                for op in &c.ops {
                    exec(&cv.draw, op);
                }
                return c.hits.clone();
            }
        }
    }
    draw_all(cv, app);
    let hits = cv.hits.take();
    *cache = (!animated).then(|| DrawCache { key, at: crate::clock::ticks(), ops: cv.rec.take(), hits: hits.clone() });
    hits
}

fn draw_all(cv: &Cv, app: &mut App) {
    // One window around everything in the red XX style (like the right panel), over a light dim of the game
    cv.rect_a(WIN.0, WIN.1, WIN.2, WIN.3, BG, 240);
    cv.frame(WIN.0, WIN.1, WIN.2, WIN.3, RED_TONES, &RED_UNDER);
    cv.tx(60., 22., t(app, k::TITLE), 26., GOLD);
    match app.screen {
        Screen::Main => draw_main(cv, app),
        Screen::Skills => draw_skills(cv, app),
        Screen::Results | Screen::Saved => draw_results(cv, app),
        Screen::Manage => draw_manage(cv, app),
        Screen::Box => draw_box(cv, app),
    }
    if app.fixmenu.is_some() {
        draw_fixmenu(cv, app);
    }
    if app.exmenu.is_some() {
        draw_exmenu(cv, app);
    }
    draw_dialog(cv, app);
    // Help line: explains what is under the cursor (shared by every screen)
    if let Some(key) = help_key(app) {
        cv.line(60., 618., 1220., 618., LINE, 1.);
        cv.text(60., 626., t(app, key), 18., MUTED, Some(1160.), LA);
    }
    // Button hints: right-aligned at the bottom of the window (text only, not mouse targets)
    let hints = app.hints();
    let mut x = 1220.;
    for (b, key) in hints.into_iter().rev() {
        let dim = key == k::H_SAVE_DONE;
        let label = t(app, key);
        x -= cv.measure(19., label);
        cv.text(x, 678., label, 19., if dim { MUTED } else { TEXT }, None, LM);
        x -= 36.;
        cv.button(app, b, x, 662., 32., dim);
        x -= 26.;
    }
    // A cached frame re-issues the same hints, so the value stays right until the next real draw
    unsafe { *core::ptr::addr_of_mut!(HINTS_LEFT) = x + 26. };
}

/// Left end of the button hints (canvas units) as last drawn: the guest mod's slot goes left of it
static mut HINTS_LEFT: f32 = 1220.;
fn hints_left() -> f32 {
    unsafe { *core::ptr::addr_of!(HINTS_LEFT) }
}

fn option_label(opt: Opt) -> u16 {
    match opt {
        Opt::Kind => k::KIND,
        Opt::Weapon => k::WEAPON,
        Opt::Armor => k::ARMOR,
        Opt::Smithy => k::SMITHY,
        Opt::MinDef => k::MINDEF,
        Opt::Resist => k::RESIST,
        Opt::Priority => k::PRIORITY,
        Opt::Forbid => k::FORBID,
        Opt::Search => k::SEARCH,
    }
}
fn option_value(app: &App, opt: Opt, i: usize) -> String {
    match opt {
        Opt::Kind => tl(app, k::KIND_VALUES, i).into(),
        Opt::Weapon => tl(app, k::WEAPON_VALUES, i).into(),
        Opt::Priority => tl(app, k::PRIORITY_VALUES, i).into(),
        Opt::MinDef => {
            if i == 0 {
                t(app, k::MINDEF_NONE).into()
            } else {
                format!("{}", MIN_DEF[i])
            }
        }
        _ => String::new(),
    }
}
fn sub_summary(app: &App, opt: Opt) -> String {
    if opt == Opt::Armor {
        if (0..ARMOR_USE).all(|i| app.armor_use(i)) {
            return t(app, k::ARMOR_ALL).into();
        }
        let mut on: Vec<String> = (0..ARMOR_USE).filter(|&i| app.armor_use(i)).map(|i| tl(app, k::ARMOR_SHORT, i).into()).collect();
        if app.ranks.iter().all(|&r| r) {
            let joined = format!("{}~{}", tl(app, k::ARMOR_SHORT, 0), tl(app, k::ARMOR_SHORT, 2));
            on = core::iter::once(joined).chain(on.into_iter().skip(3)).collect();
        }
        if on.is_empty() { t(app, k::ARMOR_NONE).into() } else { on.join("·") }
    } else {
        let parts: Vec<String> = (0..RES)
            .filter_map(|e| RES_STEPS[app.res[e]].map(|v| format!("{} {}↑", tl(app, k::RES_SHORT, e), v)))
            .collect();
        if parts.is_empty() { t(app, k::RES_NONE).into() } else { parts.join(" · ") }
    }
}
fn fix_label(app: &App, slot: usize) -> Option<String> {
    if slot == CHARM_ROW {
        app.fixed_charm.and_then(|i| app.db.inv().charms.get(i)).map(|c| charm_text(app, c))
    } else if app.fixed[slot] != 0 {
        Some(nm(app, names::ARMOR, app.fixed[slot]).into())
    } else {
        None
    }
}
fn slot_icon(cv: &Cv, app: &App, slot: usize, x: f32, y: f32, size: f32) {
    if slot == CHARM_ROW {
        cv.charm_icon(0, x, y, size);
    } else if let Some(a) = app.db.armor(app.fixed[slot]) {
        cv.armor_icon(a, x, y, size);
    } else {
        cv.sprite(&format!("part-{}", slot), x, y, size, 0xffffff, false);
    }
}

fn draw_main(cv: &Cv, app: &App) {
    // Left: target skills
    cv.panel(60., 70., 436., 610., Some(&format!("{}  {} / {}", t(app, k::TARGETS), app.targets.len(), MAX_TARGETS)), false);
    for i in 0..=app.targets.len() {
        let y = 122. + i as f32 * 52.;
        let here = app.col == 0 && app.row == i;
        cv.row(72., y - 6., 424., y + 42., here, Target::Main(0, i));
        if let Some(&id) = app.targets.get(i) {
            let s = app.db.skill(id);
            cv.text(90., y - 4., skill_name(app, id), 21., TEXT, Some(320.), LA);
            if let Some(s) = s {
                cv.text(90., y + 22., &format!("{} {}", tree_name(app, s.tree), s.threshold), 14., MUTED, Some(320.), LA);
            }
            continue;
        }
        let full = app.targets.len() >= MAX_TARGETS;
        if !here {
            cv.outline(80., y - 2., 416., y + 36., LINE);
        }
        cv.text(248., y + 17., t(app, if full { k::ADD_SKILL_FULL } else { k::ADD_SKILL }), if full { 16. } else { 19. }, if full { MUTED } else { GOLD }, None, MM);
    }
    // Middle: search conditions
    cv.panel(452., 70., 828., 610., Some(t(app, k::OPTIONS)), false);
    // 48 spacing so 9 rows (search mode added) do not reach the panel's bottom line (556)
    let row_y = |i: usize| 120. + i as f32 * 48.;
    for (i, &opt) in OPTIONS.iter().enumerate() {
        let y = row_y(i);
        cv.row(464., y - 7., 816., y + 33., app.col == 1 && app.row == i, Target::Main(1, i));
        if opt == Opt::Search {
            cv.text(640., y + 13., &format!("▶ {}", t(app, k::SEARCH)), 21., GOLD, None, MM);
            continue;
        }
        if opt == Opt::Smithy {
            // Long label in some languages: smaller first, then cut with '…' (the help line has the full text)
            let label = t(app, k::SMITHY);
            let room = 770. - 480.;
            let size = if cv.measure(19., label) <= room { 19. } else { 17. };
            // Dimmed when the smithy list could not be read (searching with it on is refused)
            let readable = app.db.inv().smithy.is_some();
            cv.text(480., y + (19. - size) / 2., label, size, if readable { TEXT } else { MUTED }, Some(room), LA);
            cv.checkbox(782., y + 3., app.smithy);
            continue;
        }
        cv.tx(480., y, t(app, option_label(opt)), 19., TEXT);
        if opt.choices() > 0 {
            cv.text(778., y, &option_value(app, opt, app.opt_value(opt)), 19., GOLD, None, RA);
            cv.text(800., y + 3., "▼", 14., MUTED, None, RA);
        } else if opt == Opt::Forbid {
            cv.checkbox(782., y + 3., app.forbid);
        } else {
            // Same size as the other values when it fits beside the label; a long summary drops to 17, then ends with '…'
            let room = 778. - 492. - cv.measure(19., t(app, option_label(opt)));
            let summary = sub_summary(app, opt);
            let size = if cv.measure(19., &summary) <= room { 19. } else { 17. };
            cv.text(778., y + (19. - size) / 2., &summary, size, GOLD, Some(room), RA);
            cv.text(800., y + 3., "▶", 14., MUTED, None, RA);
        }
    }
    cv.line(466., 556., 814., 556., LINE, 1.);
    cv.text(640., 582., &fmt2(t(app, k::STATUS), app.excluded.len() + app.excluded_jewels.len(), app.saved.len()), 16., MUTED, None, MM);
    // Right: fixed equipment
    cv.panel(844., 70., 1220., 610., Some(t(app, k::FIX_TITLE)), true);
    for slot in 0..FIX_ROWS {
        let y = 122. + slot as f32 * 58.;
        cv.row(856., y - 6., 1208., y + 44., app.col == 2 && app.row == slot, Target::Main(2, slot));
        slot_icon(cv, app, slot, 870., y + 2., 30.);
        let label = fix_label(app, slot);
        cv.text(914., y + 5., label.as_deref().unwrap_or(t(app, k::FIX_NONE)), 19., if label.is_some() { TEXT } else { MUTED }, Some(284.), LA);
    }
    cv.text(1032., 490., t(app, k::FIX_HELP), 15., MUTED, None, MM);
    cv.text(1032., 512., t(app, k::FIX_HELP2), 15., MUTED, None, MM);

    if let Some((opt, idx)) = app.drop {
        let n = opt.choices();
        let top = (row_y(OPTIONS.iter().position(|&o| o == opt).unwrap()) + 36.).min(600. - 14. - n as f32 * 40.);
        cv.round(560., top, 816., top + 14. + n as f32 * 40., 8., PANEL);
        cv.round_outline(560., top, 816., top + 14. + n as f32 * 40., 8., GOLD);
        for i in 0..n {
            let y = top + 10. + i as f32 * 40.;
            cv.row(568., y - 3., 808., y + 33., i == idx, Target::Drop(i));
            cv.tx(584., y, &option_value(app, opt, i), 19., if i == app.opt_value(opt) { GOLD } else { TEXT });
            if opt == Opt::Weapon {
                cv.slots(744., y + 8., i as u8, 0);
            }
        }
    }
    if let Some((opt, idx)) = app.sub {
        let rows = if opt == Opt::Armor { ARMOR_USE } else { RES };
        let row_top = row_y(OPTIONS.iter().position(|&o| o == opt).unwrap());
        let top = (row_top + 40.).min(600. - 58. - rows as f32 * 42.);
        cv.popup(472., top, 816., top + 56. + rows as f32 * 42.);
        cv.tx(490., top + 12., t(app, if opt == Opt::Armor { k::ARMOR } else { k::RES_TITLE }), 16., MUTED);
        let armor_keys = [k::RANK0, k::RANK1, k::RANK2, k::DEVIANT, k::ARENA, k::REWARD, k::EVENT];
        for i in 0..rows {
            let y = top + 46. + i as f32 * 42.;
            cv.row(480., y - 5., 808., y + 33., i == idx, Target::Sub(i));
            if opt == Opt::Armor {
                cv.tx(500., y, t(app, armor_keys[i]), 19., TEXT);
                cv.checkbox(776., y + 4., app.armor_use(i));
            } else {
                cv.tx(500., y, tl(app, k::RES, i), 19., RES_COLORS[i]);
                let v = RES_STEPS[app.res[i]];
                let label = v.map_or_else(|| String::from(t(app, k::RES_NONE)), |v| format!("{} ↑", v));
                cv.text(770., y, &label, 19., if v.is_some() { GOLD } else { MUTED }, None, RA);
                cv.text(796., y + 3., "▼", 14., MUTED, None, RA);
            }
        }
        if let Some((elem, cur)) = app.subdrop {
            let h = 14. + RES_STEPS.len() as f32 * 36.;
            let dtop = (top + 46. + elem as f32 * 42. + 36.).min(600. - h);
            cv.popup(640., dtop, 808., dtop + h);
            for (i, v) in RES_STEPS.iter().enumerate() {
                let y = dtop + 9. + i as f32 * 36.;
                cv.row(648., y - 3., 800., y + 29., i == cur, Target::SubDrop(i));
                let label = v.map_or_else(|| String::from(t(app, k::RES_NONE)), |v| format!("{} ↑", v));
                cv.tx(664., y, &label, 19., if i == app.res[elem] { GOLD } else { TEXT });
            }
        }
    }
    if let Some(m) = app.menu {
        cv.round(420., 200., 860., 222. + 44. + MENU.len() as f32 * 48., 10., PANEL);
        cv.round_outline(420., 200., 860., 222. + 44. + MENU.len() as f32 * 48., 10., GOLD);
        cv.tx(440., 212., t(app, k::MENU), 18., MUTED);
        for (i, item) in MENU.iter().enumerate() {
            let y = 254. + i as f32 * 48.;
            cv.row(432., y - 6., 848., y + 36., i == m, Target::Menu(i));
            let (key, count) = match item {
                MenuItem::Excluded => (k::EXCLUDED, Some(app.excluded.len() + app.excluded_jewels.len())),
                MenuItem::ResetTargets => (k::RESET_TARGETS, None),
                MenuItem::ResetFixed => (k::RESET_FIXED, None),
            };
            cv.tx(452., y, t(app, key), 21., TEXT);
            if let Some(c) = count {
                cv.text(830., y, &format!("{}", c), 21., MUTED, None, RA);
            }
        }
    }
}

fn draw_fixmenu(cv: &Cv, app: &App) {
    let (slot, idx) = app.fixmenu.unwrap();
    let items = app.fix_items(slot);
    cv.round(420., 220., 860., 300. + items.len() as f32 * 48., 10., PANEL);
    cv.round_outline(420., 220., 860., 300. + items.len() as f32 * 48., 10., GOLD);
    slot_icon(cv, app, slot, 440., 232., 28.);
    cv.text(478., 234., fix_label(app, slot).as_deref().unwrap_or(t(app, k::FIX_NONE)), 18., MUTED, Some(360.), LA);
    for (i, item) in items.iter().enumerate() {
        let y = 284. + i as f32 * 48.;
        cv.row(432., y - 6., 848., y + 36., i == idx, Target::Fix(i));
        let key = match item {
            FixItem::Equipped => k::FIX_EQUIPPED,
            FixItem::Box => k::FIX_BOX,
            FixItem::Release => k::FIX_RELEASE,
        };
        cv.tx(452., y, t(app, key), 21., TEXT);
    }
}

/// The results' exclude list (X on a part row): the part's armor and its decorations
fn draw_exmenu(cv: &Cv, app: &App) {
    let idx = app.exmenu.unwrap_or(0);
    let items = app.exclude_items();
    let bottom = 300. + items.len() as f32 * 48.;
    cv.round(420., 220., 860., bottom, 10., PANEL);
    cv.round_outline(420., 220., 860., bottom, 10., GOLD);
    cv.text(640., 248., t(app, k::EXCLUDE_PICK), 18., MUTED, None, MM);
    for (i, item) in items.iter().enumerate() {
        let y = 284. + i as f32 * 48.;
        cv.row(432., y - 6., 848., y + 36., i == idx, Target::Ex(i));
        excluded_row(cv, app, *item, 452., y, 380.);
    }
}

/// Icon and name of an excluded (or to-be-excluded) armor piece or decoration
fn excluded_row(cv: &Cv, app: &App, item: ExItem, x: f32, y: f32, width: f32) {
    match item {
        ExItem::Armor(id) => {
            if let Some(a) = app.db.armor(id) {
                cv.armor_icon(a, x, y, 30.);
            }
            cv.text(x + 46., y, nm(app, names::ARMOR, id), 21., TEXT, Some(width), LA);
        }
        ExItem::Jewel(id) => {
            cv.sprite("jewel", x, y, 30., 0xffffff, false);
            cv.text(x + 46., y, nm(app, names::JEWEL, id), 21., TEXT, Some(width), LA);
        }
    }
}

fn draw_manage(cv: &Cv, app: &App) {
    let items = app.manage_items();
    cv.panel(60., 70., 1220., 610., Some(&format!("{}  {}", t(app, k::EXCLUDED), items.len())), false);
    if items.is_empty() {
        cv.text(640., 340., t(app, k::NONE), 21., MUTED, None, MM);
    }
    let top = app.midx / MANAGE_ROWS * MANAGE_ROWS;
    for (row, &item) in items.iter().enumerate().skip(top).take(MANAGE_ROWS) {
        let y = 120. + (row - top) as f32 * 41.;
        cv.row(74., y - 5., 1190., y + 35., row == app.midx, Target::Manage(row));
        excluded_row(cv, app, item, 90., y, 600.);
        if let ExItem::Armor(aid) = item {
            if let Some(a) = app.db.armor(aid) {
                cv.text(1170., y + 2., &format!("{} {}", t(app, k::COL_DEF), a.def), 19., TEXT, None, RA);
            }
        }
    }
    cv.pager(app, 640., 568., items.len(), MANAGE_ROWS, top, 0);
}

fn draw_box(cv: &Cv, app: &App) {
    let items = app.box_items();
    cv.panel(60., 70., 1220., 610., Some(&format!("{}  {}", t(app, k::BOX_TITLE), items.len())), true);
    if items.is_empty() {
        cv.text(640., 340., t(app, k::BOX_NONE), 21., MUTED, None, MM);
    }
    let target_trees: Vec<u16> = app.targets.iter().filter_map(|&id| app.db.skill(id)).map(|s| s.tree).collect();
    for (row, &item) in items.iter().enumerate().skip(app.btop).take(MANAGE_ROWS) {
        let y = 120. + (row - app.btop) as f32 * 41.;
        cv.row(74., y - 5., 1190., y + 35., row == app.bidx, Target::Box(row));
        if app.box_slot == CHARM_ROW {
            let c = app.db.inv().charms[item as usize];
            cv.charm_icon(0, 90., y, 30.);
            cv.text(136., y, &charm_text(app, &c), 21., TEXT, Some(700.), LA);
            cv.slots(1116., y + 9., c.slots, 0);
            continue;
        }
        let Some(a) = app.db.armor(item) else { continue };
        cv.armor_icon(a, 90., y, 30.);
        cv.text(136., y, nm(app, names::ARMOR, item), 21., TEXT, Some(300.), LA);
        let mut x = 460.;
        for s in a.skills.iter().filter(|s| s.tree != 0) {
            let label = format!("{} {:+}", tree_name(app, s.tree), s.pt);
            cv.tx(x, y + 3., &label, 16., if s.pt < 0 { RED } else if target_trees.contains(&s.tree) { GREEN } else { MUTED });
            x += cv.measure(16., &label) + 16.;
        }
        cv.slots(1050., y + 9., a.slots, 0);
        cv.text(1170., y + 2., &format!("{}", a.def), 19., TEXT, None, RA);
    }
    cv.pager(app, 640., 568., items.len(), MANAGE_ROWS, app.btop, 0);
}

fn tab_key(i: usize) -> u16 {
    [k::CAT_RECENT, k::CAT_ATTACK, k::CAT_BLADE, k::CAT_GUNNER, k::CAT_GUARD, k::CAT_MOVE, k::CAT_LIFE, k::CAT_SPECIAL].get(i).copied().unwrap_or(k::CAT_SPECIAL)
}

fn draw_skills(cv: &Cv, app: &App) {
    cv.panel(60., 70., 1220., 610., None, false);
    let n = app.tab_count();
    let widths: Vec<f32> = (0..n).map(|i| cv.measure(19., t(app, tab_key(i))) + 26.).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6. * (n as f32 - 1.);
    let mut x = 640. - total / 2.;
    cv.button(app, Button::L, x - 48., 82., 32., false);
    cv.button(app, Button::R, x + total + 16., 82., 32., false);
    cv.hit(x - 52., 80., x - 12., 116., Target::Hint(Button::L));
    cv.hit(x + total + 12., 80., x + total + 52., 116., Target::Hint(Button::R));
    for i in 0..n {
        let w = widths[i];
        cv.hit(x, 80., x + w, 116., Target::Tab(i));
        if i == app.tab {
            cv.round(x, 82., x + w, 114., 6., GOLD);
        }
        cv.text(x + w / 2., 98., t(app, tab_key(i)), 19., if i == app.tab { PANEL } else if app.hover == Some(Target::Tab(i)) { TEXT } else { MUTED }, None, MM);
        x += w + 6.;
    }
    cv.line(74., 124., 1206., 124., LINE, 1.);
    let items = app.skill_list();
    if items.is_empty() && app.tab == 0 {
        cv.text(640., 330., t(app, k::RECENT_NONE), 19., MUTED, None, MM);
    }
    for (row, &id) in items.iter().enumerate().skip(app.stop).take(SKILL_ROWS) {
        let y = 132. + (row - app.stop) as f32 * 36.;
        cv.row(74., y - 3., 1206., y + 31., row == app.sidx, Target::Skill(row));
        let chosen = app.targets.contains(&id);
        cv.text(100., y, skill_name(app, id), 21., if chosen { GREEN } else { TEXT }, Some(560.), LA);
        if let Some(s) = app.db.skill(id) {
            cv.text(1186., y + 2., &format!("{} {}", tree_name(app, s.tree), s.threshold), 18., MUTED, None, RA);
        }
    }
    cv.line(74., 532., 1206., 532., LINE, 1.);
    let label = fmt1(t(app, k::PICKED), app.targets.len());
    cv.text(90., 549., &label, 17., if app.targets.is_empty() { MUTED } else { GREEN }, None, LM);
    let picked: Vec<&str> = app.targets.iter().map(|&id| skill_name(app, id)).collect();
    let names = if picked.is_empty() { String::from(t(app, k::PICKED_NONE)) } else { picked.join(" · ") };
    let lw = cv.measure(17., &label);
    cv.text(110. + lw, 549., &names, 17., if app.targets.is_empty() { MUTED } else { TEXT }, Some(1090. - lw), LM);
    cv.pager(app, 640., 568., items.len(), SKILL_ROWS, app.stop, 0);
}

fn set_label(cv: &Cv, app: &App, r: &Set, size: f32, width: f32) -> String {
    let skills: Vec<&str> = r.targets.iter().map(|&id| skill_name(app, id)).collect();
    if skills.is_empty() {
        return "—".into();
    }
    let label = skills.join(" · ");
    let mut key = r.targets.clone();
    key.sort();
    let same: Vec<usize> = (0..app.saved.len())
        .filter(|&i| {
            let mut x = app.saved[i].targets.clone();
            x.sort();
            x == key
        })
        .collect();
    if same.len() < 2 {
        return cv.fit(&label, size, width);
    }
    let me = app.saved.iter().position(|s| core::ptr::eq(s, r)).and_then(|i| same.iter().position(|&x| x == i)).map_or(1, |p| p + 1);
    let num = format!("  ({})", me);
    format!("{}{}", cv.fit(&label, size, width - cv.measure(size, &num)), num)
}

fn draw_list(cv: &Cv, app: &App) {
    let saved = app.screen == Screen::Saved;
    let rows = app.rows();
    let per = if saved { SAVED_ROWS } else { LIST_ROWS };
    let count = if saved { app.saved.len() } else { app.results.len() };
    cv.panel(60., 70., 440., 610., Some(&format!("{}  {}", t(app, if saved { k::SAVED } else { k::RESULTS }), count)), false);
    let needed = !saved && app.needed_mode;
    if !saved {
        // How these results were found: incl. unowned charms · earlier result (same condition remembered)
        let cached = app.results_cached;
        let parts: Vec<&str> = [(needed, k::RES_NEEDED), (cached, k::RES_CACHED)].iter().filter(|x| x.0).map(|x| t(app, x.1)).collect();
        if !parts.is_empty() {
            cv.text(426., 86., &parts.join(" · "), 14., MUTED, Some(240.), RA);
        }
    }
    if saved {
        // Column order (user request): defense, parts to craft, free slots
        for (x, key) in [(282., k::COL_DEF), (342., k::COL_CRAFT), (398., k::COL_FREE)] {
            cv.text(x, 132., t(app, key), 15., MUTED, None, MM);
        }
        cv.text(86., 132., t(app, k::COL_SET), 15., MUTED, Some(150.), LM);
    } else {
        for (x, key) in [(112., k::COL_DEF), (184., k::COL_CRAFT), (256., k::COL_FREE), (356., if needed { k::COL_CHANCE } else { k::COL_NOTE })] {
            cv.text(x, 132., t(app, key), 15., MUTED, None, MM);
        }
    }
    let step = if saved { 52. } else { 42. };
    for (row, item) in rows.iter().enumerate().skip(app.rtop).take(per) {
        let y = 150. + (row - app.rtop) as f32 * step;
        cv.row(74., y - 5., 426., y + step - 9., row == app.ridx, Target::Row(row));
        let Some(i) = item else {
            cv.line(80., y - 6., 420., y - 6., LINE, 1.);
            let label = if app.show_all { String::from(t(app, k::FOLD_CLOSE)) } else { fmt1(t(app, k::FOLD_OPEN), app.results.len() - app.best_n) };
            cv.text(250., y + 14., &label, 17., MUTED, None, MM);
            continue;
        };
        let r = &app.store()[*i];
        let craft = r.craft(app.db);
        if saved {
            cv.text(86., y + 7., &set_label(cv, app, r, 18., 330.), 18., GREEN, None, LM);
            cv.text(398., y + 31., &fmt1(t(app, k::FREE_N), r.free), 17., TEXT, None, MM);
            cv.text(282., y + 31., &format!("{}", r.def), 17., TEXT, None, MM);
            cv.text(342., y + 31., &fmt1(t(app, k::CRAFT_N), craft), 17., if craft == 0 { GREEN } else { TEXT }, None, MM);
            continue;
        }
        if app.row_has_alts(*i) {
            cv.circle(84., y + 12., 3.5, GOLD, true); // Other armor can be swapped in (centered on the row's numbers as drawn in game, measured 2026-11-19)
        }
        // Free slots, with the container sizes when they are split (e.g. '3칸 1+1+1': size-3 decorations do not fit)
        let sizes = r.free_slots(app.db);
        if sizes.len() > 1 {
            let parts: Vec<String> = sizes.iter().map(|s| format!("{}", s)).collect();
            cv.text(256., y + 6., &fmt1(t(app, k::FREE_N), r.free), 19., TEXT, None, MM);
            cv.text(256., y + 25., &parts.join("+"), 13., MUTED, None, MM);
        } else {
            cv.text(256., y + 14., &fmt1(t(app, k::FREE_N), r.free), 20., TEXT, None, MM);
        }
        cv.text(112., y + 14., &format!("{}", r.def), 20., TEXT, None, MM);
        cv.text(184., y + 14., &fmt1(t(app, k::CRAFT_N), craft), 20., if craft == 0 { GREEN } else { TEXT }, None, MM);
        if needed {
            cv.text(356., y + 14., &odds(app, r.chance), 17., RED, None, MM);
        } else if r.tags != 0 {
            let tags: Vec<u16> = [(1u8, k::TAG_READY), (2, k::TAG_SLOTS), (4, k::TAG_DEF)].iter().filter(|(b, _)| r.tags & b != 0).map(|x| x.1).collect();
            // The reasons are explained in the box below the list (for the set under the cursor)
            let _ = tags;
            let tag = t(app, k::TAG_REC);
            let w = cv.measure(14., tag) + 14.;
            cv.round_outline(356. - w / 2., y + 2., 356. + w / 2., y + 26., 5., GREEN);
            cv.text(356., y + 14., tag, 14., GREEN, None, MM);
        } else if app.priority == 2 && r.craft_rare(app.db) > 0 {
            let rare = r.craft_rare(app.db);
            cv.text(356., y + 14., &fmt1(t(app, k::RARE_SHORT), rare), 16., rare_color(rare), None, MM);
        } else {
            cv.text(356., y + 14., "—", 16., MUTED, None, MM);
        }
    }
    // Why the set under the cursor is recommended (one line per reason)
    if let Some(r) = app.current().filter(|r| !saved && !needed && r.tags != 0) {
        let top = 150. + per as f32 * step + 2.;
        cv.line(80., top, 420., top, LINE, 1.);
        cv.tx(86., top + 8., t(app, k::REC_TITLE), 16., MUTED);
        let mut y = top + 38.;
        for (bit, name, desc) in [(1u8, k::TAG_READY, k::TAG_READY_DESC), (2, k::TAG_SLOTS, k::TAG_SLOTS_DESC), (4, k::TAG_DEF, k::TAG_DEF_DESC)] {
            if r.tags & bit == 0 {
                continue;
            }
            // Same green pill as the list's '추천' tag
            let label = t(app, name);
            let w = cv.measure(14., label) + 14.;
            cv.round_outline(84., y - 2., 84. + w, y + 22., 5., GREEN);
            cv.text(84. + w / 2., y + 10., label, 14., GREEN, None, MM);
            let x = 84. + w + 8.;
            cv.text(x, y + 10., t(app, desc), 15., TEXT, Some(424. - x), LM);
            y += 30.;
        }
    } else if let Some(i) = app.current_index().filter(|_| !saved) {
        // Not a pick: explain the set under the cursor instead of leaving the box empty
        let top = 150. + per as f32 * step + 2.;
        cv.line(80., top, 420., top, LINE, 1.);
        let r = &app.results[i];
        if needed {
            // Unowned-charm result: the charm it needs and its chance
            cv.tx(86., top + 8., t(app, k::NEED_TITLE), 16., MUTED);
            let c = r.charm_data(app.db).map(|c| {
                let mut s = charm_text(app, &c);
                if c.slots > 0 {
                    s.push_str(&format!("  {}", fmt1(t(app, k::SLOTS_N), c.slots)));
                }
                s
            });
            cv.text(86., top + 40., &c.unwrap_or_else(|| "—".into()), 17., TEXT, Some(334.), LA);
            cv.text(86., top + 68., &fmt1(t(app, k::ODDS_LINE), odds(app, r.chance)), 16., RED, Some(334.), LA);
        } else if app.best_n > 0 && i >= app.best_n {
            // Compared with the top pick: what this set gains or loses
            let best = &app.results[0];
            cv.tx(86., top + 8., t(app, k::VS_REC_TITLE), 16., MUTED);
            let rows = [
                (k::COL_DEF, r.def as i32, best.def as i32, true),
                (k::COL_CRAFT, r.craft(app.db) as i32, best.craft(app.db) as i32, false),
                (k::COL_FREE, r.free as i32, best.free as i32, true),
            ];
            for (n, (key, v, b, higher_better)) in rows.into_iter().enumerate() {
                let y = top + 38. + n as f32 * 26.;
                let d = v - b;
                let good = if higher_better { d > 0 } else { d < 0 };
                let color = if d == 0 { MUTED } else if good { GREEN } else { RED };
                cv.tx(86., y, t(app, key), 16., MUTED);
                let shown = if d == 0 { "±0".into() } else { format!("{:+}", d) };
                cv.text(420., y, &shown, 16., color, None, RA);
            }
        }
    } else if !saved && app.rows().get(app.ridx) == Some(&None) {
        // The fold row
        let top = 150. + per as f32 * step + 2.;
        cv.line(80., top, 420., top, LINE, 1.);
        cv.text(86., top + 10., t(app, k::FOLD_INFO), 16., MUTED, Some(334.), LA);
    }
    cv.pager(app, 250., 568., rows.len(), per, app.rtop, 0);
    if !saved && app.results.iter().enumerate().any(|(i, _)| app.row_has_alts(i)) {
        cv.circle(88., 581., 3.5, GOLD, true);
        cv.text(96., 581., t(app, k::ALT_MARK), 14., MUTED, None, LM);
    }
}

fn res_line(cv: &Cv, app: &App, x0: f32, x1: f32, y: f32, values: &[i16; RES], size: f32, cond: Option<[Option<i8>; RES]>, justify: bool) {
    let items: Vec<(&str, String, i16, f32)> = (0..RES)
        .map(|e| {
            let label = tl(app, k::RES_SHORT, e);
            let shown = if values[e] != 0 { format!("{:+}", values[e]) } else { "0".into() };
            let w = cv.measure(size, label) + 5. + cv.measure(size, &shown);
            (label, shown, values[e], w)
        })
        .collect();
    let total: f32 = items.iter().map(|x| x.3).sum();
    let gap = if justify { (x1 - x0 - total) / 4. } else { 32. };
    let mut left = x0;
    for (e, (label, shown, value, width)) in items.iter().enumerate() {
        let need = cond.and_then(|c| c[e]);
        let fill = match need {
            Some(n) => {
                if *value >= n as i16 { GREEN } else { RED }
            }
            None => {
                if *value < 0 { RED } else { TEXT }
            }
        };
        cv.tx(left, y, label, size, RES_COLORS[e]);
        cv.tx(left + cv.measure(size, label) + 5., y, shown, size, fill);
        if need.is_some() {
            cv.round_outline(left - 6., y - 3., left + width + 6., y + size + 6., 5., GREEN);
        }
        left += width + gap;
    }
}

/// Results screen row (5 armor, charm, weapon) -> decoration container index (armor 0-4, weapon 5, charm 6)
const CONTAINER: [usize; 7] = [0, 1, 2, 3, 4, CHARM, WEAPON];
/// Weapon row of the results screen (differs from the decoration container index WEAPON)
const WEAPON_ROW: usize = 6;

fn draw_detail(cv: &Cv, app: &App, r: &Set) {
    let (placed, caps) = placement_cached(app, r);
    cv.panel(60., 70., 440., 610., None, false);
    let target_trees: Vec<u16> = app.targets.iter().filter_map(|&id| app.db.skill(id)).map(|s| s.tree).collect();
    let step = 38.;
    if app.pidx < PARTS {
        cv.button(app, Button::ZL, 78., 86., 28., false);
        cv.text(250., 100., &format!("{} / 2", app.dpage + 1), 18., TEXT, None, MM);
        cv.button(app, Button::ZR, 394., 86., 28., false);
        cv.hit(72., 80., 112., 120., Target::Hint(Button::ZL));
        cv.hit(388., 80., 428., 120., Target::Hint(Button::ZR));
    }
    cv.line(74., 122., 426., 122., LINE, 1.);
    cv.tx(80., 130., t(app, k::DETAIL), 18., GOLD);
    cv.line(74., 162., 426., 162., LINE, 1.);
    let mut y = 178.;
    let (title, rows, skills, res): (String, Vec<(String, String, u32)>, Vec<Pt>, Option<[i8; RES]>);
    if app.pidx < PARTS {
        let id = r.head(app.pidx);
        let Some(a) = app.db.armor(id) else { return };
        if app.dpage == 1 {
            draw_materials(cv, app, a, id, y);
            return;
        }
        cv.armor_icon(a, 78., y - 2., 34.);
        title = nm(app, names::ARMOR, id).into();
        let rc = rare_color(a.rare);
        rows = alloc::vec![(t(app, k::RARE).into(), format!("{}", a.rare), rc), (t(app, k::MAX_DEF).into(), format!("{}", a.def), TEXT)];
        skills = a.skills.to_vec();
        res = Some(a.res);
    } else if app.pidx == CHARM_ROW {
        let c = r.charm_data(app.db);
        cv.charm_icon(r.charm_rare, 78., y - 2., 34.);
        title = t(app, if r.needed { k::NEED_TITLE } else if c.is_some() { k::CHARM } else { k::NO_CHARM }).into();
        rows = if r.charm_rare > 0 { alloc::vec![(t(app, k::RARE).into(), format!("{}", r.charm_rare), rare_color(r.charm_rare))] } else { Vec::new() };
        skills = c.map(|c| c.skills.to_vec()).unwrap_or_default();
        res = None;
    } else {
        title = t(app, k::WEAPON_ROW).into();
        rows = Vec::new();
        skills = Vec::new();
        res = None;
    }
    cv.text(if app.pidx < WEAPON_ROW { 122. } else { 82. }, y, &title, 22., TEXT, Some(300.), LA);
    y += 48.;
    for (label, value, color) in &rows {
        cv.tx(82., y, label, 18., *color);
        cv.text(420., y, value, 18., *color, None, RA);
        cv.line(78., y + 31., 422., y + 31., LINE, 1.);
        y += step;
    }
    cv.tx(82., y, t(app, k::COL_SLOTS), 18., TEXT);
    let cap = caps[CONTAINER[app.pidx]];
    let used: u8 = placed[CONTAINER[app.pidx]].iter().map(|&j| app.db.jewel(j).map_or(0, |j| j.size)).sum();
    cv.slots(426. - cap as f32 * 18., y + 6., cap, used);
    if cap == 0 {
        cv.text(420., y, "—", 18., MUTED, None, RA);
    }
    cv.line(78., y + 31., 422., y + 31., LINE, 1.);
    y += step;
    if let Some(res) = res {
        res_line(cv, app, 82., 420., y, &core::array::from_fn(|e| res[e] as i16), 16., None, true);
        cv.line(78., y + 31., 422., y + 31., LINE, 1.);
        y += step;
    }
    if app.pidx < WEAPON_ROW {
        cv.tx(82., y + 4., t(app, k::EQUIP_SKILLS), 15., MUTED);
        y += 30.;
        let shown: Vec<&Pt> = skills.iter().filter(|s| s.tree != 0).collect();
        for s in &shown {
            let color = if s.pt < 0 { RED } else if target_trees.contains(&s.tree) { GREEN } else { TEXT };
            cv.text(82., y, tree_name(app, s.tree), 19., color, Some(260.), LA);
            cv.text(420., y, &format!("{:+}", s.pt), 19., color, None, RA);
            cv.line(78., y + 29., 422., y + 29., LINE, 1.);
            y += 34.;
        }
        if shown.is_empty() {
            cv.tx(82., y, "—", 19., MUTED);
            y += 34.;
        }
        if app.pidx < PARTS && skills.iter().any(|s| s.tree == TREE_TORSO_UP && s.pt > 0) {
            cv.text(82., y + 2., t(app, k::TUP_NOTE1), 15., MUTED, Some(340.), LA);
            cv.text(82., y + 22., t(app, k::TUP_NOTE2), 15., MUTED, Some(340.), LA);
            y += 46.;
        }
    }
    if app.pidx == CHARM_ROW && r.needed {
        cv.text(82., y + 4., t(app, k::NEED_NOTE), 15., MUTED, Some(340.), LA);
        cv.text(82., y + 26., &fmt1(t(app, k::NEED_CHANCE), odds(app, r.chance)), 15., RED, Some(340.), LA);
        y += 50.;
    }
    cv.tx(82., y + 6., t(app, k::JEWELS_HERE), 15., MUTED);
    y += 30.;
    let mut counts: Vec<(u16, usize)> = Vec::new();
    for &j in &placed[CONTAINER[app.pidx]] {
        match counts.iter_mut().find(|c| c.0 == j) {
            Some(c) => c.1 += 1,
            None => counts.push((j, 1)),
        }
    }
    // One per line; when they would run past the panel's bottom (many armor skills + several decorations), two columns
    let two = y + counts.len() as f32 * 27. > 598.;
    for (i, (j, n)) in counts.iter().enumerate() {
        let (x, yy, w) = if two { (82. + (i % 2) as f32 * 172., y + (i / 2) as f32 * 27., 164.) } else { (82., y + i as f32 * 27., 330.) };
        cv.text(x, yy, &format!("{} ×{}", nm(app, names::JEWEL, *j), n), 18., TEXT, Some(w), LA);
    }
    if counts.is_empty() {
        cv.tx(82., y, "—", 18., MUTED);
    }
}

fn draw_materials(cv: &Cv, app: &App, a: &Armor, id: u16, mut y: f32) {
    cv.armor_icon(a, 78., y - 2., 34.);
    cv.text(122., y, nm(app, names::ARMOR, id), 22., TEXT, Some(300.), LA);
    y += 52.;
    cv.tx(82., y, t(app, k::MATERIALS), 15., MUTED);
    y += 28.;
    let (mats, price) = app.db.cost(id);
    for &(item, game, qty) in mats {
        let info = atlas::ITEMS.get(game as usize);
        let drawn = info.is_some_and(|i| cv.sprite(&format!("item-{}", i.0), 80., y - 2., 30., i.1, false));
        if !drawn {
            cv.sprite("item-46", 80., y - 2., 30., 0xffffff, false);
        }
        // Same notation as Wishlist: name, x required quantity, box icon, owned quantity (item box + pouch). Red when short.
        let inv = app.db.inv();
        let have = inv.items.get(&game).copied().unwrap_or(0);
        let short = inv.loaded && have < qty as u32;
        let color = if short { RED } else { TEXT };
        cv.text(122., y, nm(app, names::ITEM, item), 19., color, Some(190.), LA);
        cv.text(318., y, &format!("× {}", qty), 19., TEXT, None, LA);
        cv.sprite("chest", 364., y + 1., 24., 0xffffff, false);
        cv.text(420., y, &if inv.loaded { format!("{}", have) } else { String::from("?") }, 19., color, None, RA);
        cv.line(78., y + 33., 422., y + 33., LINE, 1.);
        y += 42.;
    }
    if mats.is_empty() {
        // Not in the game's smithy list either (checked against the recipes from the game data, 2026-11-19):
        // it is not made at the smithy, so no price either
        cv.text(82., y, t(app, k::NO_MATERIALS), 18., MUTED, Some(340.), LA);
        match reward_source(id) {
            Some(key) => {
                cv.tx(82., y + 40., t(app, k::OBTAIN), 15., MUTED);
                for (i, line) in t(app, key).split('\u{1f}').enumerate() {
                    cv.text(82., y + 68. + i as f32 * 27., line, 17., TEXT, Some(340.), LA);
                }
            }
            None => cv.text(82., y + 28., t(app, k::NO_MATERIALS2), 15., MUTED, Some(340.), LA),
        }
        return;
    }
    cv.round(76., y + 10., 424., y + 46., 4., SELECT);
    cv.text(90., y + 28., t(app, k::PRICE), 18., TEXT, None, LM);
    cv.text(410., y + 28., &format!("{} z", price), 18., GOLD, None, RM);
}

/// How reward armor (no smithy recipe) is obtained: string key. Checked against several guides and the game's own
/// quest names, arena conditions and village chiefs (2026-11-19). Same table as the preview (render.py REWARD_SOURCE).
fn reward_source(id: u16) -> Option<u16> {
    Some(match id {
        2933 => k::SRC_HAWK,
        2934 => k::SRC_FALCON,
        2935 => k::SRC_CRANE,
        2936 => k::SRC_ARENA_CLEAR,
        2937 => k::SRC_ARENA_A,
        2938 => k::SRC_COMET_BHERNA,
        2939 => k::SRC_COMET_KOKOTO,
        2940 => k::SRC_COMET_POKKE,
        2941 => k::SRC_COMET_YUKUMO,
        2942 => k::SRC_COMET_ARGOSY,
        4764..=4773 => k::SRC_KAISERIN,
        4774..=4793 => k::SRC_BLOODBATH,
        4794 => k::SRC_HEAD_BHERNA,
        4795 => k::SRC_HEAD_KOKOTO,
        4796 => k::SRC_HEAD_POKKE,
        4797 => k::SRC_HEAD_YUKUMO,
        4798 => k::SRC_ARENA_ALL_CLEAR,
        4799 => k::SRC_ARENA_ALL_A,
        _ => return None,
    })
}

/// Decoration placement is heavy (placement search), so it is not recomputed for the same set
fn placement_cached(app: &App, r: &Set) -> ([Vec<u16>; 7], [u8; 7]) {
    let key: crate::app::PlacementKey =
        (app.screen as u8, app.current_index().unwrap_or(usize::MAX), core::array::from_fn(|p| r.head(p)), r.charm, r.jewels.len());
    if let Some((k, v)) = app.placement_cache.borrow().as_ref() {
        if *k == key {
            return v.clone();
        }
    }
    let v = r.placement(app.db);
    *app.placement_cache.borrow_mut() = Some((key, v.clone()));
    v
}

fn draw_placement(cv: &Cv, app: &App, r: &Set, placed: &[Vec<u16>; 7]) {
    cv.tx(488., 420., t(app, k::JEWEL_PLACE), 16., MUTED);
    let mut y = 446.;
    let weapon = t(app, k::WEAPON_ROW);
    let tx = 488. + (cv.measure(13., weapon) + 8.).max(30.);
    let cx = (488. + tx - 8.) / 2.;
    let copies = r.torso_copies(app.db);
    let boost = r.talisman(app.db);
    for p in 0..7 {
        let items = &placed[CONTAINER[p]];
        if items.is_empty() {
            continue;
        }
        if p < PARTS {
            if let Some(a) = app.db.armor(r.head(p)) {
                cv.armor_icon(a, cx - 10., y, 20.);
            }
        } else if p == CHARM_ROW {
            cv.charm_icon(r.charm_rare, cx - 10., y, 20.);
        } else {
            // The equipped weapon's icon; the word 'Weapon' when nothing is equipped (or not read yet)
            let kind = app.db.inv().equipped_weapon;
            if kind == 0 || !cv.sprite(&format!("weapon-{}", kind), cx - 10., y, 20., 0xffffff, false) {
                cv.text(cx, y + 10., weapon, 13., MUTED, None, MM);
            }
        }
        let mut counts: Vec<(u16, usize)> = Vec::new();
        for &j in items {
            match counts.iter_mut().find(|c| c.0 == j) {
                Some(c) => c.1 += 1,
                None => counts.push((j, 1)),
            }
        }
        let label = counts.iter().map(|(j, n)| if *n > 1 { format!("{} ×{}", nm(app, names::JEWEL, *j), n) } else { nm(app, names::JEWEL, *j).into() }).collect::<Vec<_>>().join(", ");
        // Torso up / talisman boost: same size as the decorations, in parentheses right after them: "Decoration [2] (Torso Up ×2)"
        let mult = if p == TORSO && copies > 0 {
            Some(format!("({})", fmt1(t(app, k::TUP_MULT), copies + 1)))
        } else if p == CHARM_ROW && boost {
            Some(format!("({})", t(app, k::TAL_MULT)))
        } else {
            None
        };
        if let Some(mult) = mult {
            let gap = cv.measure(16., " ");
            let mw = cv.measure(16., &mult) + gap;
            let label = cv.fit(&label, 16., 828. - tx - mw);
            cv.text(tx, y + 10., &label, 16., TEXT, None, LM);
            cv.text(tx + cv.measure(16., &label) + gap, y + 10., &mult, 16., GOLD, None, LM);
        } else {
            cv.text(tx, y + 10., &label, 16., TEXT, Some(828. - tx), LM);
        }
        y += 21.;
    }
    if y == 446. {
        cv.tx(488., y, "—", 18., MUTED);
    }
}

fn draw_results(cv: &Cv, app: &mut App) {
    let cur = app.current().cloned();
    match (&cur, app.focus) {
        // With the alternatives popup open, the detail shows the armor under the popup cursor
        (Some(r), Focus::Parts) => match app.popup_preview() {
            Some(alt) => draw_detail(cv, app, &alt),
            None => draw_detail(cv, app, r),
        },
        _ => draw_list(cv, app),
    }
    cv.panel(460., 70., 1220., 610., None, true);
    if let Some(extra) = &app.extra {
        cv.popup(300., 100., 980., 606.);
        cv.tx(320., 112., t(app, if app.extra_any { k::EXTRA_TITLE_ANY } else { k::EXTRA_TITLE }), 18., MUTED);
        if extra.is_empty() {
            cv.text(640., 348., t(app, k::EXTRA_NONE), 18., MUTED, None, MM);
            cv.text(640., 376., t(app, k::EXTRA_NONE2), 15., MUTED, None, MM);
        }
        // Category tabs like the skill picker (only categories with skills), L/R or a click to switch
        let tabs = app.extra_tabs();
        if !tabs.is_empty() {
            let widths: Vec<f32> = tabs.iter().map(|(c, items)| cv.measure(17., &format!("{} {}", t(app, tab_key(*c)), items.len())) + 22.).collect();
            let total: f32 = widths.iter().sum::<f32>() + 6. * (tabs.len() as f32 - 1.);
            let mut x = 640. - total / 2.;
            if tabs.len() > 1 {
                cv.button(app, Button::L, x - 42., 146., 28., false);
                cv.button(app, Button::R, x + total + 14., 146., 28., false);
                cv.hit(x - 46., 142., x - 10., 178., Target::Hint(Button::L));
                cv.hit(x + total + 10., 142., x + total + 46., 178., Target::Hint(Button::R));
            }
            for (i, (c, items)) in tabs.iter().enumerate() {
                let w = widths[i];
                let here = i == app.etab;
                cv.hit(x, 144., x + w, 176., Target::ETab(i));
                if here {
                    cv.round(x, 146., x + w, 174., 6., GOLD);
                }
                let color = if here { PANEL } else if app.hover == Some(Target::ETab(i)) { TEXT } else { MUTED };
                cv.text(x + w / 2., 160., &format!("{} {}", t(app, tab_key(*c)), items.len()), 17., color, None, MM);
                x += w + 6.;
            }
            cv.line(316., 186., 964., 186., LINE, 1.);
        }
        let list = app.extra_list();
        for (row, &id) in list.iter().enumerate().skip(app.etop).take(EXTRA_ROWS) {
            let y = 200. + (row - app.etop) as f32 * 38.;
            cv.row(312., y - 4., 968., y + 30., row == app.eidx, Target::Extra(row));
            cv.text(332., y, skill_name(app, id), 20., TEXT, Some(400.), LA);
            if let Some(s) = app.db.skill(id) {
                cv.text(950., y + 2., &format!("{} {}", tree_name(app, s.tree), s.threshold), 16., MUTED, None, RA);
            }
        }
        cv.pager(app, 640., 554., list.len(), EXTRA_ROWS, app.etop, 1);
        return;
    }
    if app.extra_busy() {
        // Progress of the extra-skill search (B cancels)
        let (done, all) = app.extra_prog;
        let p = if all > 0 { done as f32 / all as f32 } else { 0. };
        cv.popup(400., 280., 880., 420.);
        cv.text(640., 316., t(app, if app.extra_any { k::EXTRA_TITLE_ANY } else { k::EXTRA_TITLE }), 19., TEXT, None, MM);
        let (bx0, bx1, by) = (440., 840., 346.);
        cv.round(bx0, by, bx1, by + 18., 9., BG);
        cv.round_outline(bx0, by, bx1, by + 18., 9., LINE);
        if p > 0. {
            cv.round(bx0 + 2., by + 2., bx0 + 2. + ((bx1 - bx0 - 4.) * p).max(14.), by + 16., 7., GREEN);
        }
        cv.text(bx1, by + 30., &format!("{}%", (p * 100.) as u32), 15., MUTED, None, RA);
        return;
    }
    let Some(r) = cur else { return };
    if app.compare && app.screen == Screen::Results {
        draw_compare(cv, app, &r);
        return;
    }
    let (placed, caps) = placement_cached(app, &r);
    let used: Vec<u8> = (0..7).map(|c| placed[c].iter().map(|&j| app.db.jewel(j).map_or(0, |j| j.size)).sum()).collect();
    cv.tx(488., 82., t(app, k::COL_PART), 15., MUTED);
    cv.tx(560., 82., t(app, k::COL_ARMOR), 15., MUTED);
    cv.text(1104., 82., t(app, k::COL_SLOTS), 15., MUTED, None, MA);
    cv.text(1196., 82., t(app, k::COL_DEF), 15., MUTED, None, RA);
    cv.line(474., 106., 1206., 106., LINE, 1.);
    let row_y = |p: usize| 116. + p as f32 * 36.;
    for p in 0..6 {
        cv.line(474., row_y(p) + 31., 1206., row_y(p) + 31., LINE, 1.);
    }
    for p in 0..7 {
        let y = row_y(p);
        cv.row(472., y - 4., 1208., y + 30., app.focus == Focus::Parts && app.pidx == p, Target::Part(p));
        cv.slots(1080., y + 7., caps[CONTAINER[p]], used[CONTAINER[p]]);
        if p == CHARM_ROW {
            let c = r.charm_data(app.db);
            cv.charm_icon(r.charm_rare, 490., y - 1., 26.);
            let desc = c.map(|c| charm_text(app, &c)).unwrap_or_default();
            let label = if !desc.is_empty() { desc.clone() } else if c.is_some() { t(app, k::CHARM_SLOTS_ONLY).into() } else { t(app, k::NO_CHARM).into() };
            cv.text(560., y - 2., &label, 20., if desc.is_empty() { MUTED } else { TEXT }, Some(300.), LA);
            if c.is_some() && (r.needed || (app.fixed_charm.is_some() && app.screen == Screen::Results)) {
                let tag = if r.needed { fmt1(t(app, k::NEED_TAG_ODDS), odds(app, r.chance)) } else { t(app, k::FIXED_TAG).into() };
                let w = cv.measure(14., &tag) + 14.;
                cv.round_outline(872., y + 1., 872. + w, y + 24., 5., RED);
                cv.text(872. + w / 2., y + 12., &tag, 14., RED, None, MM);
            }
            continue;
        }
        if p == WEAPON_ROW {
            cv.tx(560., y - 2., t(app, k::WEAPON_ROW), 20., MUTED);
            continue;
        }
        let id = r.head(p);
        let Some(a) = app.db.armor(id) else { continue };
        cv.armor_icon(a, 490., y - 1., 26.);
        cv.text(560., y - 2., nm(app, names::ARMOR, id), 20., TEXT, Some(300.), LA);
        let mut x = 872.;
        // Computed just by resting the cursor on the result list, so it shows on the list screen too
        // Worthwhile alternatives only (the dimmed, strictly worse ones are still in the popup)
        let alts = if app.screen == Screen::Results { app.alts_useful(p) } else { 0 };
        let owned = app.db.inv().owned.contains(&id);
        for (tag, show, color) in [
            (fmt1(t(app, k::ALT_TAG), alts), alts > 0, GOLD),
            (String::from(t(app, k::OWNED)), owned, GREEN),
            (String::from(t(app, k::CRAFTABLE)), !owned && app.db.craftable(id), CRAFT),
            (String::from(t(app, k::FIXED_TAG)), app.fixed[p] == id, RED),
            (String::from(t(app, k::TUP_TAG)), p != TORSO && crate::db::Db::is_torso_up(a), MUTED),
        ] {
            if show {
                let w = cv.measure(14., &tag) + 14.;
                cv.round_outline(x, y + 1., x + w, y + 24., 5., color);
                cv.text(x + w / 2., y + 12., &tag, 14., color, None, MM);
                x += w + 6.;
            }
        }
        cv.text(1196., y - 1., &format!("{}", a.def), 19., TEXT, None, RA);
    }
    cv.line(474., 372., 1206., 372., LINE, 1.);
    let (total_def, total_res) = r.sums(app.db);
    cv.tx(488., 381., t(app, k::TOTAL), 17., MUTED);
    let cond = (app.screen == Screen::Results).then(|| core::array::from_fn(|e| RES_STEPS[app.res[e]]));
    res_line(cv, app, 560., 1100., 381., &total_res, 17., cond, false);
    cv.text(1196., 379., &format!("{}", total_def), 20., TEXT, None, RA);
    cv.line(474., 412., 1206., 412., LINE, 1.);
    draw_placement(cv, app, &r, &placed);
    // Active skills
    cv.tx(840., 420., t(app, k::SKILLS), 16., MUTED);
    let mut x = 1196.;
    for (key, color) in [(k::LG_NEG, RED), (k::LG_BONUS, GOLD), (k::LG_TARGET, GREEN)] {
        let label = t(app, key);
        x -= cv.measure(14., label);
        cv.tx(x, 421., label, 14., MUTED);
        cv.circle(x - 9.5, 431.5, 4.5, color, true);
        x -= 30.;
    }
    let mut target_trees: Vec<u16> = r.targets.iter().filter_map(|&id| app.db.skill(id)).map(|s| s.tree).collect();
    if target_trees.is_empty() {
        target_trees = app.targets.iter().filter_map(|&id| app.db.skill(id)).map(|s| s.tree).collect();
    }
    let active = app.db.active_skills(&r.totals(app.db));
    let two = active.len() > 6;
    for (i, s) in active.iter().enumerate() {
        let color = if s.threshold < 0 { RED } else if target_trees.contains(&s.tree) { GREEN } else { GOLD };
        let (col, row) = if two { (i % 2, i / 2) } else { (0, i) };
        cv.text(840. + col as f32 * 180., 446. + row as f32 * 24., skill_name(app, s.id), 18., color, Some(if two { 172. } else { 356. }), LA);
    }
    // Pick alternative armor (same layout as the preview render.py)
    if let Some(popup) = &app.popup {
        let rows = crate::app::ALT_ROWS;
        let shown = popup.len().min(rows);
        let paged = popup.len() > rows;
        let h = 70. + shown as f32 * 38. + if paged { 40. } else { 0. };
        // Keep the popup inside the right panel (it used to run over the help line and the window border)
        let py = (604. - h).min(200.);
        cv.popup(500., py, 1100., py + h);
        cv.tx(520., py + 12., t(app, k::ALTS), 18., MUTED);
        cv.text(970., py + 14., t(app, k::COL_FREE), 15., MUTED, None, MA);
        cv.text(1072., py + 14., t(app, k::COL_DEF), 15., MUTED, None, RA);
        // Page by page (left/right turn pages, up/down move through every row)
        let top = app.aidx / rows * rows;
        if paged {
            cv.pager(app, 800., py + h - 44., popup.len(), rows, top, 1);
        }
        let worse_from = app.alt_worse_from.get();
        for (row, (aid, free, _)) in popup.iter().enumerate().skip(top).take(rows) {
            let y = py + 52. + (row - top) as f32 * 38.;
            let Some(a) = app.db.armor(*aid) else { continue };
            // Strictly worse than an entry above (same or fewer free slots, no more defense, not owned): dimmed
            let worse = row >= worse_from;
            cv.row(512., y - 4., 1088., y + 30., row == app.aidx, Target::Alt(row));
            cv.armor_icon(a, 524., y - 1., 28.);
            cv.text(564., y, nm(app, names::ARMOR, *aid), 20., if worse { MUTED } else { TEXT }, Some(300.), LA);
            // Owned / craftable now: green pills like the other tags
            let owned = app.db.inv().owned.contains(aid);
            let tag = if owned {
                Some((k::OWNED, GREEN))
            } else if app.db.craftable(*aid) {
                Some((k::CRAFTABLE, CRAFT))
            } else {
                None
            };
            if let Some((key, color)) = tag {
                let label = t(app, key);
                let w = cv.measure(14., label) + 14.;
                cv.round_outline(900. - w / 2., y + 1., 900. + w / 2., y + 25., 5., color);
                cv.text(900., y + 13., label, 14., color, None, MM);
            }
            // Free slots: red if fewer than the current set
            cv.text(970., y, &fmt1(t(app, k::FREE_N), free), 19., if *free < r.free { RED } else { TEXT }, None, MA);
            cv.text(1072., y, &format!("{}", a.def), 19., if worse { MUTED } else { TEXT }, None, RA);
        }
    }
}

// ---- Dialogs ----
/// The equipped gear as a set (armor, charm and their decorations read from the game). None if not read.
fn equipped_set(app: &App) -> Option<Set> {
    let inv = app.db.inv();
    if !inv.loaded || inv.equipped_armor.iter().all(|&a| a == 0) {
        return None;
    }
    let parts: [Vec<u16>; PARTS] = core::array::from_fn(|p| alloc::vec![inv.equipped_armor[p]]);
    let charm = inv.equipped_charm.map_or(-1, |i| i as i32);
    Some(Set { parts, charm, jewels: inv.equipped_jewels.clone(), ..Set::default() })
}

/// Compare page: per part current -> this set, total differences, skill changes (same layout as the preview).
fn draw_compare(cv: &Cv, app: &App, r: &Set) {
    cv.tx(488., 82., t(app, k::CMP_TITLE), 17., GOLD);
    let Some(now) = equipped_set(app) else {
        cv.text(840., 300., t(app, k::CMP_NONE), 18., MUTED, None, MM);
        return;
    };
    cv.tx(560., 112., t(app, k::CMP_NOW), 15., TEXT);
    cv.tx(880., 112., t(app, k::CMP_SET), 15., TEXT);
    cv.line(474., 134., 1206., 134., LINE, 1.);
    let armor_name = |id: u16| if app.db.armor(id).is_some() { String::from(nm(app, names::ARMOR, id)) } else { "—".into() };
    for p in 0..6 {
        let y = 142. + p as f32 * 34.;
        if p < PARTS {
            let (a, b) = (now.head(p), r.head(p));
            if let Some(arm) = app.db.armor(b) {
                cv.armor_icon(arm, 490., y, 24.);
            }
            let same = a == b;
            cv.text(560., y + 1., &armor_name(a), 18., MUTED, Some(280.), LA);
            let shown = if same { String::from(t(app, k::CMP_SAME)) } else { armor_name(b) };
            cv.text(880., y + 1., &shown, 18., if same { MUTED } else { TEXT }, Some(300.), LA);
        } else {
            let (c0, c1) = (now.charm_data(app.db), r.charm_data(app.db));
            cv.charm_icon(r.charm_rare, 490., y, 24.);
            cv.text(560., y + 1., &c0.map_or("—".into(), |c| charm_text(app, &c)), 18., MUTED, Some(280.), LA);
            let same = c0 == c1;
            let shown = if same { String::from(t(app, k::CMP_SAME)) } else { c1.map_or("—".into(), |c| charm_text(app, &c)) };
            cv.text(880., y + 1., &shown, 18., if same { MUTED } else { TEXT }, Some(300.), LA);
        }
        cv.tx(850., y + 1., "→", 18., MUTED);
        cv.line(474., y + 29., 1206., y + 29., LINE, 1.);
    }
    let ((d0, r0), (d1, r1)) = (now.sums(app.db), r.sums(app.db));
    let mut y = 356.;
    cv.tx(488., y, t(app, k::COL_DEF), 17., MUTED);
    let diff = d1 as i32 - d0 as i32;
    let shown = format!("{}  →  {}", d0, d1);
    cv.tx(560., y, &shown, 18., TEXT);
    let (label, color) = if diff > 0 { (format!("({:+})", diff), GREEN) } else if diff < 0 { (format!("({:+})", diff), RED) } else { ("(±0)".into(), MUTED) };
    // Right after the numbers it belongs to
    cv.tx(560. + cv.measure(18., &shown) + 12., y + 1., &label, 17., color);
    // Defense is compared fully upgraded on both sides (the current gear may not be upgraded yet)
    cv.text(1196., y + 2., t(app, k::CMP_DEF_NOTE), 15., MUTED, None, RA);
    y += 30.;
    // This set's resistances (what matters), each with its change from the current gear in parentheses like defense
    let mut left = 560.;
    for e in 0..RES {
        let label = tl(app, k::RES_SHORT, e);
        let value = if r1[e] != 0 { format!("{:+}", r1[e]) } else { "0".into() };
        let d = r1[e] - r0[e];
        let (change, color) = if d > 0 { (format!("({:+})", d), GREEN) } else if d < 0 { (format!("({:+})", d), RED) } else { ("(±0)".into(), MUTED) };
        cv.tx(left, y, label, 17., RES_COLORS[e]);
        left += cv.measure(17., label) + 5.;
        cv.tx(left, y, &value, 17., if r1[e] < 0 { RED } else { TEXT });
        left += cv.measure(17., &value) + 5.;
        cv.tx(left, y + 1., &change, 15., color);
        left += cv.measure(15., &change) + 26.;
    }
    cv.tx(488., y, t(app, k::RES_CHANGE), 17., MUTED);
    cv.line(474., y + 30., 1206., y + 30., LINE, 1.);
    let before = app.db.active_skills(&now.totals(app.db));
    let after = app.db.active_skills(&r.totals(app.db));
    let has = |list: &[&Skill], id: u16| list.iter().any(|s| s.id == id);
    let gain: Vec<&Skill> = after.iter().copied().filter(|s| !has(&before, s.id)).collect();
    let lose: Vec<&Skill> = before.iter().copied().filter(|s| !has(&after, s.id)).collect();
    let keep: Vec<&Skill> = after.iter().copied().filter(|s| has(&before, s.id)).collect();
    for (k, (key, skills, color)) in [(k::CMP_GAIN, gain, GREEN), (k::CMP_LOSE, lose, RED), (k::CMP_KEEP, keep, TEXT)].into_iter().enumerate() {
        let x = 488. + k as f32 * 244.;
        cv.tx(x, y + 42., t(app, key), 15., MUTED);
        for (i, s) in skills.iter().take(6).enumerate() {
            cv.text(x, y + 66. + i as f32 * 24., skill_name(app, s.id), 17., color, Some(230.), LA);
        }
        if skills.is_empty() {
            cv.tx(x, y + 66., "—", 17., MUTED);
        }
    }
}

fn draw_dialog(cv: &Cv, app: &mut App) {
    let Some((kind, buttons)) = app.dialog() else { return };
    match kind {
        Dialog::Loading => return draw_loading(cv, app, &buttons),
        Dialog::DiagWait => return draw_diagwait(cv, app),
        Dialog::Diag => return draw_diag(cv, app, buttons.len()),
        _ => {}
    }
    let (title, note): (String, String) = match kind {
        Dialog::AskNeeded => (t(app, k::ASK_NEEDED).into(), t(app, k::ASK_NEEDED2).into()),
        Dialog::Confirm => {
            let (a, b) = match app.confirm.unwrap() {
                Confirm::ResetTargets => (k::CONFIRM_RESET_TARGETS, Some(k::CONFIRM_NOTE)),
                Confirm::ResetFixed => (k::CONFIRM_RESET_FIXED, Some(k::CONFIRM_NOTE)),
                Confirm::Release(_) | Confirm::ReleaseJewel(_) => (k::CONFIRM_RELEASE, Some(k::CONFIRM_RELEASE2)),
                Confirm::ReleaseAll => (k::CONFIRM_RELEASE_ALL, Some(k::CONFIRM_RELEASE_ALL2)),
                Confirm::Save => (k::CONFIRM_SAVE, Some(k::CONFIRM_SAVE2)),
                Confirm::Exclude(_) => (k::CONFIRM_EXCLUDE, Some(k::CONFIRM_EXCLUDE2)),
                Confirm::ExcludeJewel(_) => (k::CONFIRM_EXCLUDE_JEWEL, Some(k::CONFIRM_EXCLUDE2)),
                Confirm::DeleteSaved(_) => (k::CONFIRM_DELETE_SAVED, Some(k::CONFIRM_DELETE_SAVED2)),
                Confirm::ExtraAny => (k::CONFIRM_EXTRA_ANY, Some(k::CONFIRM_EXTRA_ANY2)),
                Confirm::LeaveResults => (k::CONFIRM_LEAVE_RESULTS, None),
            };
            (t(app, a).into(), b.map_or(String::new(), |b| t(app, b).into()))
        }
        Dialog::Notice => {
            let n = app.notice.unwrap();
            let (a, b) = match n {
                Notice::NoResult => (k::NOTICE_NO_RESULT, if app.mindef != 0 || app.res.iter().any(|&i| RES_STEPS[i].is_some()) { k::NOTICE_NO_RESULT_COND } else { k::NOTICE_NO_RESULT2 }),
                Notice::NoResultFixed => (k::NOTICE_NO_RESULT_FIXED, k::NOTICE_NO_RESULT_FIXED2),
                Notice::Needed => (k::NOTICE_NEEDED, k::NOTICE_NEEDED2),
            };
            (t(app, a).into(), t(app, b).into())
        }
        _ => {
            let title = if app.message == k::MSG_NOTHING_WORN {
                if app.message_slot == CHARM_ROW { t(app, k::MSG_NOTHING_WORN_CHARM).into() } else { fmt1(t(app, k::MSG_NOTHING_WORN), t(app, [k::SLOT_0, k::SLOT_1, k::SLOT_2, k::SLOT_3, k::SLOT_4][app.message_slot.min(4)])) }
            } else {
                t(app, app.message).into()
            };
            let note = match app.message {
                m if m == k::MSG_NOTHING_WORN => t(app, k::MSG_NOTHING_WORN2).into(),
                m if m == k::MSG_SAVED_FULL => t(app, k::MSG_SAVED_FULL2).into(),
                m if m == k::EXTRA_NONE_ANY => t(app, k::EXTRA_NONE_ANY2).into(),
                m if m == k::MSG_NO_TARGET => t(app, k::MSG_NO_TARGET2).into(),
                m if m == k::MSG_FULL => t(app, k::MSG_FULL2).into(),
                m if m == k::MSG_NO_RANK => t(app, k::MSG_NO_RANK2).into(),
                m if m == k::MSG_SMITHY_UNREAD => t(app, k::MSG_SMITHY_UNREAD2).into(),
                _ => String::new(),
            };
            (title, note)
        }
    };
    // Highlighted target box: the armor of an exclude/unexclude confirmation, the set of a saved-set deletion
    let item = match app.confirm {
        Some(Confirm::Exclude(id) | Confirm::Release(id)) if kind == Dialog::Confirm => app.db.armor(id).map(|a| (a, id)),
        _ => None,
    };
    let jewel = match app.confirm {
        Some(Confirm::ExcludeJewel(id) | Confirm::ReleaseJewel(id)) if kind == Dialog::Confirm => Some(id),
        _ => None,
    };
    let target = match app.confirm {
        Some(Confirm::DeleteSaved(i)) if kind == Dialog::Confirm => app.saved.get(i).map(|r| format!("{}   {} · {}", set_label(cv, app, r, 20., 560.), fmt1(t(app, k::FREE_N), r.free), r.def)),
        _ => None,
    };
    let item_w = if let Some((_, id)) = item {
        cv.measure(22., nm(app, names::ARMOR, id)) + 44.
    } else if let Some(id) = jewel {
        cv.measure(22., nm(app, names::JEWEL, id)) + 44.
    } else if let Some(s) = &target {
        cv.measure(20., s)
    } else {
        0.
    };
    let has_box = item.is_some() || jewel.is_some() || target.is_some();
    let text_w = cv.measure(22., &title).max(if note.is_empty() { 0. } else { cv.measure(17., &note) }).max(item_w + 40.);
    let w = (text_w + 120.).clamp(440., 760.);
    let h = if note.is_empty() { 160. } else { 196. } + if has_box { 54. } else { 0. };
    let (x0, y0) = (640. - w / 2., 360. - h / 2.);
    cv.popup(x0, y0, x0 + w, y0 + h);
    cv.text(640., y0 + 46., &title, 22., TEXT, None, MM);
    if let Some((a, id)) = item {
        let bw = item_w + 40.;
        let bx = 640. - bw / 2.;
        cv.rect(bx, y0 + 72., bx + bw, y0 + 116., SELECT);
        cv.armor_icon(a, bx + 20., y0 + 79., 30.);
        cv.text(bx + 60., y0 + 94., nm(app, names::ARMOR, id), 22., rare_color(a.rare), None, LM);
    } else if let Some(id) = jewel {
        let bw = item_w + 40.;
        let bx = 640. - bw / 2.;
        cv.rect(bx, y0 + 72., bx + bw, y0 + 116., SELECT);
        cv.sprite("jewel", bx + 20., y0 + 79., 30., 0xffffff, false);
        cv.text(bx + 60., y0 + 94., nm(app, names::JEWEL, id), 22., TEXT, None, LM);
    } else if let Some(s) = &target {
        let bw = item_w + 40.;
        cv.rect(640. - bw / 2., y0 + 72., 640. + bw / 2., y0 + 116., SELECT);
        cv.text(640., y0 + 94., s, 20., TEXT, None, MM);
    }
    if !note.is_empty() {
        cv.text(640., y0 + 82. + if has_box { 58. } else { 0. }, &note, 17., MUTED, None, MM);
    }
    let line_y = y0 + h - 72.;
    cv.line(x0 + 24., line_y, x0 + w - 24., line_y, LINE, 1.);
    dialog_buttons(cv, app, &buttons, y0 + h - 54.);
}

fn dialog_buttons(cv: &Cv, app: &App, buttons: &[u16], by: f32) {
    let bw = buttons.iter().map(|&b| cv.measure(19., t(app, b)) + 40.).fold(150., f32::max);
    let gap = 24.;
    let mut x = 640. - (buttons.len() as f32 * bw + (buttons.len() as f32 - 1.) * gap) / 2.;
    let sel = app.dialog_sel.min(buttons.len() - 1);
    for (i, &b) in buttons.iter().enumerate() {
        cv.row(x, by, x + bw, by + 38., i == sel, Target::Dialog(i));
        if i != sel {
            cv.outline(x, by, x + bw, by + 38., LINE);
        }
        cv.text(x + bw / 2., by + 19., t(app, b), 19., if i == sel { TEXT } else { MUTED }, None, MM);
        x += bw + gap;
    }
}

fn draw_loading(cv: &Cv, app: &App, buttons: &[u16]) {
    // While the searcher is being built (outside the lock) there is no job yet: show the same window at progress 0 (no flicker)
    let (paused, needed, found, started, target) = match (&app.job, &app.job_out, &app.building) {
        (Some(job), _, _) => (job.state == crate::job::State::Paused, job.needed, job.found(), job.started, job.progress().clamp(0., 1.)),
        // The compute thread is running it outside the lock: the copy from that time
        (None, Some(v), _) => (false, v.needed, v.found, v.started, v.progress.clamp(0., 1.)),
        (None, None, Some(b)) => (false, b.needed, 0, b.started, 0.),
        _ => return,
    };
    let (title, note) = if paused {
        (String::from(t(app, k::LOAD_PAUSED)), fmt1(t(app, k::LOAD_PAUSED2), found))
    } else {
        let limit = crate::job::Opts::SEARCH.max;
        let note = if found >= limit {
            // The results reached the limit (100): tell why it keeps searching (better combinations, or more likely ones for unowned charms)
            fmt1(t(app, if needed { k::LOAD_FULL_NEEDED } else { k::LOAD_FULL }), limit)
        } else {
            fmt1(t(app, k::LOAD_FOUND), found)
        };
        (String::from(t(app, if needed { k::LOAD_TITLE_NEEDED } else { k::LOAD_TITLE })), note)
    };
    // Runs with no limit: show only the elapsed time
    let eta: Option<String> = if paused {
        needed.then(|| t(app, k::LOAD_PAUSED_NEEDED).into())
    } else {
        Some(fmt1(t(app, k::LOAD_ELAPSED), crate::clock::ms_since(started) / 1000))
    };
    let w = (cv.measure(22., &title).max(cv.measure(17., &note)).max(eta.as_ref().map_or(0., |e| cv.measure(16., e))).max(420.) + 120.).min(760.);
    let h = 250. + if eta.is_some() { 26. } else { 0. };
    let (x0, y0) = (640. - w / 2., 360. - h / 2.);
    cv.popup(x0, y0, x0 + w, y0 + h);
    cv.text(640., y0 + 42., &title, 22., TEXT, None, MM);
    let (bx0, bx1, by) = (x0 + 50., x0 + w - 50., y0 + 78.);
    cv.round(bx0, by, bx1, by + 18., 9., BG);
    cv.round_outline(bx0, by, bx1, by + 18., 9., LINE);
    // The real progress rises in steps whenever a thread finishes a torso candidate: the shown value follows a fraction of
    // the remaining gap every frame (smoothly) and never goes down within a phase.
    let (mut shown, phase) = app.load_shown.get();
    if phase != needed {
        shown = 0.; // The unowned-charm phase starts from zero
    }
    if target > shown {
        // When the phase is done (100%), fill quickly so it is full before the question appears
        let rate = if target >= 1. { 0.25 } else { 0.06 };
        shown = (shown + (target - shown) * rate + 0.0004).min(target);
    }
    app.load_shown.set((shown, needed));
    let p = shown;
    if p > 0. {
        let fill = ((bx1 - bx0 - 4.) * p).max(14.);
        cv.round(bx0 + 2., by + 2., bx0 + 2. + fill, by + 16., 7., if paused { GOLD } else { GREEN });
    }
    cv.text(bx1, by + 30., &format!("{}%", (p * 100. + 0.5).min(100.) as u32), 15., MUTED, None, RA);
    cv.text(640., y0 + 140., &note, 17., MUTED, None, MM);
    if let Some(e) = &eta {
        cv.text(640., y0 + 166., e, 16., MUTED, None, MM);
    }
    cv.line(x0 + 24., y0 + h - 72., x0 + w - 24., y0 + h - 72., LINE, 1.);
    dialog_buttons(cv, app, buttons, y0 + h - 54.);
}

fn draw_diagwait(cv: &Cv, app: &App) {
    let (done, total) = app.diag_progress();
    let title = t(app, k::DIAG_WAIT);
    let note = fmt2(t(app, k::DIAG_WAIT2), (done + 1).min(total.max(1)), total.max(1));
    let w = (cv.measure(22., title).max(cv.measure(17., &note)).max(420.) + 120.).min(760.);
    let h = 230.;
    let (x0, y0) = (640. - w / 2., 360. - h / 2.);
    cv.popup(x0, y0, x0 + w, y0 + h);
    cv.text(640., y0 + 42., title, 22., TEXT, None, MM);
    let (bx0, bx1, by) = (x0 + 50., x0 + w - 50., y0 + 74.);
    cv.round(bx0, by, bx1, by + 18., 9., BG);
    cv.round_outline(bx0, by, bx1, by + 18., 9., LINE);
    if total > 0 {
        let fill = ((bx1 - bx0 - 4.) * done as f32 / total as f32).max(14.);
        cv.round(bx0 + 2., by + 2., bx0 + 2. + fill, by + 16., 7., GREEN);
    }
    cv.text(640., y0 + 118., &note, 17., MUTED, None, MM);
    cv.line(x0 + 24., y0 + h - 72., x0 + w - 24., y0 + h - 72., LINE, 1.);
    cv.row(640. - 75., y0 + h - 54., 640. + 75., y0 + h - 16., true, Target::Dialog(0));
    cv.text(640., y0 + h - 35., t(app, k::H_SKIP), 19., TEXT, None, MM);
}

fn diag_label(app: &App, l: &DiagLabel) -> String {
    match l {
        DiagLabel::Forbid => t(app, k::DIAG_FORBID).into(),
        DiagLabel::MinDef => t(app, k::DIAG_MINDEF).into(),
        DiagLabel::Res => t(app, k::DIAG_RES).into(),
        DiagLabel::Fixed => t(app, k::DIAG_FIXED).into(),
        DiagLabel::Excluded => t(app, k::DIAG_EXCLUDED).into(),
        DiagLabel::Smithy => t(app, k::DIAG_SMITHY).into(),
        DiagLabel::Lower(a, b) => fmt2(t(app, k::DIAG_LOWER), skill_name(app, *a), skill_name(app, *b)),
        DiagLabel::Remove(a) => fmt1(t(app, k::DIAG_REMOVE), skill_name(app, *a)),
    }
}

fn draw_diag(cv: &Cv, app: &App, n: usize) {
    let title = t(app, if app.complete { k::DIAG_TITLE } else { k::DIAG_TITLE_PARTIAL });
    let note = t(app, k::DIAG_NOTE);
    let labels: Vec<String> = app.diag.iter().map(|(l, _)| diag_label(app, l)).collect();
    let w = (labels.iter().map(|l| cv.measure(20., l) + 60.).fold(cv.measure(22., title).max(cv.measure(17., note)), f32::max) + 120.).min(760.);
    let h = 150. + labels.len() as f32 * 44. + 64.;
    let (x0, y0) = (640. - w / 2., 360. - h / 2.);
    cv.popup(x0, y0, x0 + w, y0 + h);
    cv.text(640., y0 + 42., title, 22., TEXT, None, MM);
    cv.text(640., y0 + 80., note, 17., MUTED, None, MM);
    cv.line(x0 + 24., y0 + 106., x0 + w - 24., y0 + 106., LINE, 1.);
    let sel = app.dialog_sel.min(n - 1);
    for (i, label) in labels.iter().enumerate() {
        let y = y0 + 118. + i as f32 * 44.;
        cv.row(x0 + 30., y, x0 + w - 30., y + 38., i == sel, Target::Dialog(i));
        cv.text(640., y + 19., label, 20., if i == sel { TEXT } else { MUTED }, None, MM);
    }
    let by = y0 + h - 54.;
    cv.line(x0 + 24., by - 18., x0 + w - 24., by - 18., LINE, 1.);
    cv.row(640. - 75., by, 640. + 75., by + 38., sel == n - 1, Target::Dialog(n - 1));
    if sel != n - 1 {
        cv.outline(640. - 75., by, 640. + 75., by + 38., LINE);
    }
    cv.text(640., by + 19., t(app, k::H_CLOSE), 19., if sel == n - 1 { TEXT } else { MUTED }, None, MM);
}

/// Entries in the Forge settings menu (overlay): open button, language, button layout, logging. Every text follows
/// the Language setting (the font holds all 9 languages).
pub fn menu(app: &mut App, ready: bool) -> bool {
    use alloc::ffi::CString;
    use forge::sys::imgui::{ImGuiComboFlags, ImGuiSelectableFlags};
    let mut changed = false;
    if !imgui::tree_node(c"Skill Simulator") {
        return false;
    }
    // Draw with our font: the Forge default font has no CJK characters (18, same as Better Skillinfo)
    let font = unsafe { FONT };
    if let Some(f) = font {
        imgui::push_font(&f, Some(18.));
    }
    // Labels carry a fixed id after '##' so a widget keeps its state when the language changes
    let lang = app.language.min(8);
    let label = move |key: u16, id: &str| CString::new(format!("{}##{}", k::TABLES[lang][key as usize], id)).unwrap();
    if !self::ready() {
        imgui::text("Resources missing: romfs/nativeNX/plugins/skill-simulator/ (font.otf)");
    } else if ready {
        if imgui::button(&label(k::MENU_OPEN, "open"), ImVec2 { x: 0., y: 0. }) {
            app.open = true;
        }
    } else {
        imgui::text(t(app, k::MENU_NEED_CHAR));
    }
    let langs = ["English", "简体中文", "繁體中文", "日本語", "한국어", "Deutsch", "Français", "Italiano", "Español"];
    // These two labels stay English in every language (so a user who picked an unreadable language can find them)
    for (name, options, which) in [(c"Language", &langs[..], 0), (c"Button layout", &LAYOUTS[..], 1)] {
        let current = if which == 0 { app.language } else { app.layout };
        let selected = CString::new(options[current.min(options.len() - 1)]).unwrap();
        if imgui::begin_combo(name, &selected, ImGuiComboFlags::None) {
            for (i, name) in options.iter().enumerate() {
                let text = CString::new(*name).unwrap();
                if imgui::selectable(&text, i == current, ImGuiSelectableFlags::None, ImVec2 { x: 0., y: 0. }) {
                    if which == 0 {
                        app.language = i;
                    } else {
                        app.layout = i;
                    }
                    app.touch(); // redraw the window in the new language / button pictures
                    changed = true;
                }
            }
            imgui::end_combo();
        }
    }
    if imgui::checkbox(&label(k::MENU_LOGGING, "logging"), &mut app.logging) {
        changed = true;
    }
    imgui::push_text_wrap_pos(0.);
    imgui::text(t(app, k::MENU_LOGGING_NOTE));
    imgui::pop_text_wrap_pos();
    if font.is_some() {
        imgui::pop_font();
    }
    imgui::tree_pop();
    changed
}

/// Help line text for what is under the cursor (None: no line)
fn help_key(app: &App) -> Option<u16> {
    if app.dialog().is_some() {
        return None;
    }
    match app.screen {
        Screen::Main => Some(match app.col {
            0 => k::HELP_TARGETS,
            1 => match OPTIONS.get(app.row) {
                Some(Opt::Kind) => k::HELP_KIND,
                Some(Opt::Weapon) => k::HELP_WEAPON,
                Some(Opt::Armor) => k::HELP_ARMOR,
                Some(Opt::Smithy) => k::HELP_SMITHY,
                Some(Opt::MinDef) => k::HELP_MINDEF,
                Some(Opt::Resist) => k::HELP_RESIST,
                Some(Opt::Priority) => k::HELP_PRIORITY,
                Some(Opt::Forbid) => k::HELP_FORBID,
                _ => k::HELP_SEARCH,
            },
            _ => k::FIX_HELP,
        }),
        Screen::Results | Screen::Saved => {
            if app.popup.is_some() {
                Some(if app.aidx >= app.alt_worse_from.get() { k::HELP_ALT_WORSE } else { k::HELP_ALT })
            } else if app.focus == Focus::Parts {
                Some(k::HELP_PARTS)
            } else if app.screen == Screen::Results {
                Some(if app.needed_mode { k::LIST_HELP_NEEDED } else { k::LIST_HELP })
            } else {
                None
            }
        }
        _ => None,
    }
}
