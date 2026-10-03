//! SM64's HUD, from the player's ROM: the power meter (the "POWER" frame with the 8-wedge health
//! pie), the counters in SM64's own font (Mario's head x deaths, coins, stars) and the spinning
//! coins enemies drop (coins.rs). Drawn as a 2D overlay on the game's final image (hudhook: a
//! DirectX 12 overlay), a few textured quads a frame: cheap, pinned to the screen and untouched by
//! the game's anti-aliasing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::Instant;

use eldenring::cs::{CSCamExt, CSCamera};
use fromsoftware_shared::FromStatic;
use glam::Vec3;
use hudhook::imgui::{self, TextureId};
use hudhook::{ImguiRenderLoop, RenderContext};

use crate::log;

/// MIO0 block with the power meter and the coins (actors/power_meter, actors/coin in the decomp)
const BLOCK: usize = 0x201410;
const LEFT: usize = 0x233E0;
const RIGHT: usize = 0x243E0;
/// pies for 1..=8 wedges
const PIES: [usize; 8] = [0x28BE0, 0x283E0, 0x27BE0, 0x273E0, 0x26BE0, 0x263E0, 0x25BE0, 0x253E0];
/// the yellow coin's spin: front, tilted right, side, tilted left (IA16, tinted yellow)
const COIN_FRAMES: [usize; 4] = [0x5780, 0x5F80, 0x6780, 0x6F80];
/// MIO0 block with the HUD font (segment 2): digits 0-9 at 0x200 each, then these
const SEGMENT2: usize = 0x108A40;
const GLYPH_MULTIPLY: usize = 0x4200;
const GLYPH_COIN: usize = 0x4400;
const GLYPH_HEAD: usize = 0x4600;
const GLYPH_STAR: usize = 0x4800;

/// HUD size relative to SM64's, and the meter centre's height on SM64's 240-line screen
const SIZE: f32 = 0.65;
const HEIGHT: f32 = 192.0;
/// ...and where it rests while Mario is hurt (just under the top edge), after sliding up
/// METER_RISE_AFTER seconds after the last health change, over METER_RISE_TIME seconds
const HEIGHT_TOP: f32 = 216.0;
/// above the top edge: where it slides to when Mario is back to full health
const HEIGHT_GONE: f32 = 270.0;
const METER_RISE_AFTER: f32 = 2.0;
const METER_RISE_TIME: f32 = 0.4;
/// seconds the meter stays up after Mario is back to full health
const LINGER: f32 = 1.5;
/// the coin counter's left edge, as a share of the screen width
const COINS_X: f32 = 0.56;
/// textures are uploaded this many times bigger (nearest neighbour), so the overlay's smooth
/// filtering keeps SM64's crisp pixels
const UPSCALE: usize = 4;

/// An RGBA image (already upscaled) and its original size.
struct Image {
    rgba: Vec<u8>,
    w: usize,
    h: usize,
}

impl Image {
    fn new(w: usize, h: usize, pixel: impl Fn(usize, usize) -> [u8; 4]) -> Self {
        let (bw, bh) = (w * UPSCALE, h * UPSCALE);
        let mut rgba = Vec::with_capacity(bw * bh * 4);
        for y in 0..bh {
            for x in 0..bw {
                rgba.extend_from_slice(&pixel(x / UPSCALE, y / UPSCALE));
            }
        }
        Image { rgba, w, h }
    }
}

/// RGBA16 (5-5-5-1) pixel
fn rgba16_px(t: &[u8], i: usize) -> [u8; 4] {
    let v = u16::from_be_bytes([t[i * 2], t[i * 2 + 1]]);
    let c = |shift: u16| (((v >> shift) & 31) as u32 * 255 / 31) as u8;
    [c(11), c(6), c(1), if v & 1 != 0 { 255 } else { 0 }]
}

fn rgba16(tex: &[u8], w: usize, h: usize) -> Image {
    Image::new(w, h, |x, y| rgba16_px(tex, y * w + x))
}

/// IA16 (intensity, alpha) times a tint
fn ia16(tex: &[u8], w: usize, h: usize, tint: [u8; 3]) -> Image {
    Image::new(w, h, |x, y| {
        let (i, a) = (tex[(y * w + x) * 2], tex[(y * w + x) * 2 + 1]);
        let c = tint.map(|t| (t as u32 * i as u32 / 255) as u8);
        [c[0], c[1], c[2], a]
    })
}

/// Every HUD image, in a fixed order (see the indices below).
static IMAGES: Mutex<Option<Vec<Image>>> = Mutex::new(None);
const IMG_BASE: usize = 0;
const IMG_PIES: usize = 1; // 8
const IMG_DIGITS: usize = 9; // 10
const IMG_MULTIPLY: usize = 19;
const IMG_COIN: usize = 20;
const IMG_HEAD: usize = 21;
const IMG_STAR: usize = 22;
const IMG_COIN_FRAMES: usize = 23; // 4
const IMG_BOWSER: usize = 27;
/// SM64's HUD letters (it has no J, Q, V, X, Z), then the apostrophe
const LETTERS: [(char, usize); 21] = [
    ('A', 0x1400), ('B', 0x1600), ('C', 0x1800), ('D', 0x1A00), ('E', 0x1C00), ('F', 0x1E00), ('G', 0x2000),
    ('H', 0x2200), ('I', 0x2400), ('K', 0x2600), ('L', 0x2800), ('M', 0x2A00), ('N', 0x2C00), ('O', 0x2E00),
    ('P', 0x3000), ('R', 0x3200), ('S', 0x3400), ('T', 0x3600), ('U', 0x3800), ('W', 0x3A00), ('Y', 0x3C00),
];
const IMG_LETTERS: usize = 28; // 21
const IMG_APOSTROPHE: usize = 49;
/// the camera status (bottom right): camera, Lakitu, no-camera X, and the 8x8 zoom arrows
const IMG_CAMERA: usize = 50;
const IMG_LAKITU: usize = 51;
const IMG_ARROW_UP: usize = 52;
const IMG_ARROW_DOWN: usize = 53;

/// The glyph for a character of a name in SM64's HUD font (look-alikes for the letters it lacks).
fn glyph_for(c: char) -> Option<usize> {
    let c = match c.to_ascii_uppercase() {
        'J' => 'I',
        'Q' => 'O',
        'V' => 'U',
        'X' => 'K',
        'Z' => 'S',
        c => c,
    };
    if let Some(d) = c.to_digit(10) {
        return Some(IMG_DIGITS + d as usize);
    }
    if c == '\'' || c == ',' {
        return Some(IMG_APOSTROPHE);
    }
    LETTERS.iter().position(|&(l, _)| l == c).map(|i| IMG_LETTERS + i)
}

/// Reads the HUD's textures from the ROM.
pub fn load(rom: &[u8]) {
    let (Some(d), Some(seg2)) = (crate::gameover::mio0(rom, BLOCK), crate::gameover::mio0(rom, SEGMENT2)) else {
        log("hud: HUD textures not found in the ROM");
        return;
    };
    let tex = |d: &[u8], off: usize, len: usize| d.get(off..off + len).map(<[u8]>::to_vec);
    let load = || -> Option<Vec<Image>> {
        let (left, right) = (tex(&d, LEFT, 4096)?, tex(&d, RIGHT, 4096)?);
        // the frame: two 32x64 halves side by side
        let base = Image::new(64, 64, |x, y| if x < 32 { rgba16_px(&left, y * 32 + x) } else { rgba16_px(&right, y * 32 + x - 32) });
        let mut out = vec![base];
        for &o in &PIES {
            out.push(rgba16(&tex(&d, o, 2048)?, 32, 32));
        }
        let glyph = |off: usize| tex(&seg2, off, 512).map(|t| rgba16(&t, 16, 16));
        for i in 0..10 {
            out.push(glyph(i * 0x200)?);
        }
        for off in [GLYPH_MULTIPLY, GLYPH_COIN, GLYPH_HEAD, GLYPH_STAR] {
            out.push(glyph(off)?);
        }
        for &o in &COIN_FRAMES {
            out.push(ia16(&tex(&d, o, 2048)?, 32, 32, [255, 255, 0]));
        }
        // the game-over mask: black, soft edges (smooth filtering: no upscale needed, but keep
        // the same layout)
        let alpha = crate::gameover::mask_alpha(rom)?;
        out.push(Image::new(64, 64, |x, y| [0, 0, 0, alpha[y * 64 + x]]));
        for &(_, off) in &LETTERS {
            out.push(glyph(off)?);
        }
        out.push(glyph(0x3E00)?); // apostrophe
        out.push(glyph(0x7000)?); // camera
        out.push(glyph(0x7200)?); // Lakitu
        out.push(tex(&seg2, 0x7600, 128).map(|t| rgba16(&t, 8, 8))?); // arrow up
        out.push(tex(&seg2, 0x7680, 128).map(|t| rgba16(&t, 8, 8))?); // arrow down
        Some(out)
    };
    match load() {
        Some(images) => {
            log(format!("hud: SM64 HUD loaded ({} textures)", images.len()));
            *IMAGES.lock().unwrap_or_else(|e| e.into_inner()) = Some(images);
        }
        None => log("hud: HUD textures incomplete in the ROM"),
    }
}

// ---- what to show (set by the Mario frame) ------------------------------------------------------

/// wedges 0..=8, or 0xFF: no HUD
/// The first-launch setup box: what's happening, progress (None: no bar), a line of detail.
pub struct Setup {
    pub title: String,
    pub progress: Option<f32>,
    pub text: String,
}

static SETUP: Mutex<Option<Setup>> = Mutex::new(None);

/// Shows (Some) or hides (None) the first-launch setup box.
pub fn set_setup(setup: Option<Setup>) {
    *SETUP.lock().unwrap_or_else(|e| e.into_inner()) = setup;
}

/// Shows the setup box with a progress bar.
pub fn setup_progress(title: &str, progress: f32, text: &str) {
    set_setup(Some(Setup { title: title.into(), progress: Some(progress), text: text.into() }));
}

static WEDGES: AtomicU8 = AtomicU8::new(0xFF);
static DEATHS: AtomicU32 = AtomicU32::new(0);
static COINS: AtomicU32 = AtomicU32::new(0);
static STARS: AtomicU32 = AtomicU32::new(0);
/// when the Mario frame last reported (the HUD goes when it stops, e.g. on a loading screen)
static LAST_SET: Mutex<Option<Instant>> = Mutex::new(None);

/// `hidden`: why the HUD should go (None: it stays). A reason must last HIDE_AFTER before the HUD
/// goes (a one-frame "menu" guess or pause blip made it flicker); death and loading hide at once.
pub fn set(wedges: u8, hidden: Option<&'static str>, active: bool) {
    const HIDE_AFTER: f32 = 0.15;
    static SINCE: Mutex<Option<(Instant, &'static str)>> = Mutex::new(None);
    static SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    let mut since = SINCE.lock().unwrap_or_else(|e| e.into_inner());
    let hide = match hidden {
        None => {
            *since = None;
            false
        }
        Some(why) => {
            let (t, _) = *since.get_or_insert((Instant::now(), why));
            why == "dead" || why == "loading" || t.elapsed().as_secs_f32() >= HIDE_AFTER
        }
    };
    let show = active && !hide;
    if SHOWN.swap(show, Ordering::Relaxed) != show {
        crate::log(format!("hud: {}", if show { "shown".to_string() } else { format!("hidden ({})", hidden.unwrap_or("inactive")) }));
    }
    WEDGES.store(if show { wedges } else { 0xFF }, Ordering::Relaxed);
    *LAST_SET.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

pub fn set_counters(deaths: u32, coins: u32, stars: u32) {
    DEATHS.store(deaths, Ordering::Relaxed);
    COINS.store(coins, Ordering::Relaxed);
    STARS.store(stars, Ordering::Relaxed);
}

/// An enemy health bar (combat.rs): feet position, HP now and before the combo (0..1), combo damage.
pub struct Tag {
    pub pos: Vec3,
    pub hp: f32,
    pub before: f32,
    pub dmg: i32,
}

static TAGS: Mutex<Vec<Tag>> = Mutex::new(Vec::new());

/// A boss health bar (the bottom of the screen): name, HP now / before the combo, combo damage.
pub struct BossBar {
    pub name: String,
    pub hp: f32,
    pub before: f32,
    pub dmg: i32,
}

static BOSSES: Mutex<Vec<BossBar>> = Mutex::new(Vec::new());

pub fn set_bosses(bosses: Vec<BossBar>) {
    *BOSSES.lock().unwrap_or_else(|e| e.into_inner()) = bosses;
}

pub fn set_tags(tags: Vec<Tag>) {
    *TAGS.lock().unwrap_or_else(|e| e.into_inner()) = tags;
}

// ---- the overlay ---------------------------------------------------------------------------------

pub struct Overlay {
    textures: Vec<TextureId>,
    meter: Option<Meter>,
}

/// The power meter's motion (SM64's: damage brings it down, healing fills it where it is).
struct Meter {
    /// wedges last frame
    wedges: u8,
    /// when Mario last lost health (the meter came down then)
    hit: Instant,
    /// when he got back to full health (it slides out then), and its height at that moment
    full: Option<(Instant, f32)>,
}

/// Starts the overlay (hooks the game's DirectX 12 presentation).
pub fn install(module: usize) {
    use hudhook::hooks::dx12::ImguiDx12Hooks;
    let hmodule = hudhook::windows::Win32::Foundation::HINSTANCE(module as _);
    let overlay = Overlay { textures: Vec::new(), meter: None };
    match hudhook::Hudhook::builder().with::<ImguiDx12Hooks>(overlay).with_hmodule(hmodule).build().apply() {
        Ok(()) => log("hud: overlay hooked"),
        Err(e) => log(format!("hud: overlay hook failed: {e:?}")),
    }
}

/// World position -> screen pixel (None behind the camera), and its distance.
fn project(p: Vec3, size: [f32; 2]) -> Option<([f32; 2], f32)> {
    let cam = unsafe { CSCamera::instance() }.ok()?;
    let pers = &cam.pers_cam_1;
    let v = |d: eldenring::position::PositionDelta| Vec3::new(d.0, d.1, d.2);
    let c = pers.position();
    let (right, up, fwd) = (v(pers.right()).normalize_or_zero(), v(pers.up()).normalize_or_zero(), v(pers.forward()).normalize_or_zero());
    let d = p - Vec3::new(c.0, c.1, c.2);
    let z = d.dot(fwd);
    if z < 0.1 {
        return None;
    }
    let t = (pers.fov * 0.5).tan();
    let x = d.dot(right) / (z * t * size[0] / size[1]);
    let y = d.dot(up) / (z * t);
    Some(([size[0] * 0.5 * (1.0 + x), size[1] * 0.5 * (1.0 - y)], z))
}

impl ImguiRenderLoop for Overlay {
    fn initialize<'a>(&'a mut self, ctx: &mut imgui::Context, _render_context: &'a mut dyn RenderContext) {
        // no cursor, no settings file
        ctx.io_mut().mouse_draw_cursor = false;
        ctx.set_ini_filename(None);
    }

    fn before_render<'a>(&'a mut self, _ctx: &mut imgui::Context, render_context: &'a mut dyn RenderContext) {
        if !self.textures.is_empty() {
            return;
        }
        let images = IMAGES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(images) = images.as_ref() else { return };
        let ids: Option<Vec<TextureId>> = images
            .iter()
            .map(|i| render_context.load_texture(&i.rgba, (i.w * UPSCALE) as u32, (i.h * UPSCALE) as u32).ok())
            .collect();
        match ids {
            Some(ids) => {
                log("hud: overlay textures uploaded");
                self.textures = ids;
            }
            None => log("hud: overlay texture upload failed"),
        }
    }

    fn render(&mut self, ui: &mut imgui::Ui) {
        // first launch: building Mario from the ROM (needs no textures, so it shows from the start)
        if let Some(setup) = SETUP.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let size = ui.io().display_size;
            let scale = (size[1] / 1080.0).max(1.0) * 1.6;
            let w = 560.0 * scale / 1.6;
            ui.window("##er_mario_setup")
                .position([size[0] * 0.5, size[1] * 0.42], imgui::Condition::Always)
                .position_pivot([0.5, 0.5])
                .size([w, 0.0], imgui::Condition::Always)
                .bg_alpha(0.9)
                .no_decoration()
                .no_inputs()
                .build(|| {
                    ui.set_window_font_scale(scale);
                    ui.text("ER MARIO");
                    ui.separator();
                    ui.text_wrapped(&setup.title);
                    if let Some(p) = setup.progress {
                        imgui::ProgressBar::new(p.clamp(0.0, 1.0)).size([-1.0, 0.0]).build(ui);
                    }
                    if !setup.text.is_empty() {
                        ui.text_wrapped(&setup.text);
                    }
                });
        }
        if self.textures.len() <= IMG_ARROW_DOWN {
            return;
        }
        let size = ui.io().display_size;
        let textures = &self.textures;
        let dl = ui.get_foreground_draw_list();
        let image = |id: usize, x: f32, y: f32, w: f32, h: f32| {
            dl.add_image(textures[id], [x, y], [x + w, y + h]).build();
        };

        // SM64's game over: black everywhere except a Bowser-shaped hole that shrinks to nothing
        if let Some(t) = crate::gameover::iris() {
            let black = |x0: f32, y0: f32, x1: f32, y1: f32| {
                dl.add_rect([x0, y0], [x1, y1], [0.0, 0.0, 0.0, 1.0]).filled(true).build();
            };
            let s = size[0].max(size[1]) * 2.6 * (1.0 - t).powi(2);
            let (cx, cy) = (size[0] * 0.5, size[1] * 0.5);
            if s < 1.0 {
                black(0.0, 0.0, size[0], size[1]);
            } else {
                let (l, r, top, bot) = (cx - s * 0.5, cx + s * 0.5, cy - s * 0.5, cy + s * 0.5);
                black(0.0, 0.0, size[0], top.max(0.0));
                black(0.0, bot.min(size[1]), size[0], size[1]);
                black(0.0, top, l.max(0.0), bot);
                black(r.min(size[0]), top, size[0], bot);
                image(IMG_BOWSER, l, top, s, s);
            }
            return;
        }

        // outside the world (title screen, menus, loading): which mod is loaded, bottom left, in
        // SM64's HUD font (it has no dot: a small square at the baseline); only once Mario's files are
        // built and loaded this session (not during the first-launch setup)
        let g = 10.0 * size[1] / 240.0 * SIZE;
        let text = |text: &str, y: f32, g: f32| {
            let mut x = size[1] * 0.04;
            for c in text.chars() {
                match c {
                    ' ' => x += g * 0.5,
                    '.' => {
                        let d = g * 0.18;
                        dl.add_rect([x, y + g - d * 1.6], [x + d, y + g - d * 0.6], [1.0, 1.0, 1.0, 1.0]).filled(true).build();
                        x += g * 0.35;
                    }
                    c => {
                        if let Some(id) = glyph_for(c) {
                            image(id, x, y, g, g);
                        }
                        x += g * 0.8;
                    }
                }
            }
        };
        if !crate::in_world() && crate::assets::ready() {
            let y = size[1] * 0.96 - g;
            text(concat!("ER MARIO ", env!("CARGO_PKG_VERSION")), y, g);
            // smaller, right above the version
            if let Some(v) = crate::update::newer() {
                text(&format!("UPDATE AVAILABLE {v}"), y - g * 1.1, g * 0.75);
            }
        }

        // nothing outside Mario mode, in menus, or when the Mario frame stopped (loading)
        let wedges = WEDGES.load(Ordering::Relaxed);
        let alive = LAST_SET.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed().as_secs_f32() < 0.5);
        if wedges == 0xFF || !alive {
            return;
        }
        // in game: a small note in the bottom left corner once a newer version is out
        if let Some(v) = crate::update::newer() {
            text(&format!("ER MARIO {v} IS OUT"), size[1] * 0.985 - g * 0.5, g * 0.5);
        }

        // the coins in the world (up to 40 m away): SM64's 64 units = 0.64 m
        for (pos, age) in crate::coins::visible() {
            let (Some((foot, z)), Some((head, _))) = (project(pos, size), project(pos + Vec3::Y * 0.64, size)) else { continue };
            let h = foot[1] - head[1];
            if z > 40.0 || h < 1.0 {
                continue;
            }
            image(IMG_COIN_FRAMES + (age * 15.0) as usize % 4, foot[0] - h * 0.5, head[1], h, h);
        }

        // one of SM64's pixels (its screen is 240 lines high), at HUD size
        let px = size[1] / 240.0 * SIZE;

        // boss bars at the bottom (Elden Ring's layout), the name in SM64's font
        for (i, boss) in BOSSES.lock().unwrap_or_else(|e| e.into_inner()).iter().enumerate() {
            let w = (size[1] * 16.0 / 9.0 * 0.55).min(size[0] * 0.9);
            let h = (size[1] * 0.008).max(4.0);
            let x0 = (size[0] - w) * 0.5;
            let y0 = size[1] * 0.86 - i as f32 * size[1] * 0.07;
            let g = 11.0 * px;
            let mut x = x0;
            for c in boss.name.chars() {
                if let Some(id) = glyph_for(c) {
                    image(id, x, y0 - g - 6.0, g, g);
                }
                x += g * 0.8;
            }
            let rect = |a: f32, b: f32, col: [f32; 4]| {
                dl.add_rect([x0 + w * a, y0], [x0 + w * b, y0 + h], col).filled(true).build();
            };
            dl.add_rect([x0 - 2.0, y0 - 2.0], [x0 + w + 2.0, y0 + h + 2.0], [0.0, 0.0, 0.0, 0.7]).filled(true).build();
            rect(boss.hp, boss.before.max(boss.hp), [0.85, 0.7, 0.2, 1.0]);
            rect(0.0, boss.hp, [0.6, 0.05, 0.05, 1.0]);
            if boss.dmg > 0 {
                let digits = boss.dmg.to_string();
                let mut x = x0 + w - digits.len() as f32 * g * 0.8;
                for ch in digits.bytes() {
                    image(IMG_DIGITS + (ch - b'0') as usize, x, y0 - g - 6.0, g, g);
                    x += g * 0.8;
                }
            }
        }

        let board = crate::skate::visual();
        if board.enabled {
            let label = if board.mounted {
                format!("Skate {:.1} m/s  {}  X ollie / airborne: RB flip, LT shuv-it", board.speed * 0.30,
                    match board.trick { 2 => "Kickflip", 3 => "Shuv-it", 1 => "Ollie", _ => "A push / B brake" })
            } else if board.bail_ticks > 0 { "Skate: recover before remounting".into() }
            else { "Skate: D-pad Left / V to mount".into() };
            dl.add_text([size[0] * 0.5 + 50.0 * px, size[1] - 110.0 * px], [0.9, 0.85, 0.35, 1.0], label);
        }

        // enemy health bars (Elden Ring's style: dark frame, red HP, yellow for the combo's damage)
        // over the heads of what Mario hit, with the combo's damage in SM64's digits
        for tag in TAGS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            let Some((top, z)) = project(tag.pos + Vec3::Y * 2.1, size) else { continue };
            if z > 40.0 {
                continue;
            }
            let (w, h) = (size[1] * 0.075, (size[1] * 0.006).max(3.0));
            let (x0, y0) = (top[0] - w * 0.5, top[1]);
            let rect = |a: f32, b: f32, col: [f32; 4]| {
                dl.add_rect([x0 + w * a, y0], [x0 + w * b, y0 + h], col).filled(true).build();
            };
            dl.add_rect([x0 - 2.0, y0 - 2.0], [x0 + w + 2.0, y0 + h + 2.0], [0.0, 0.0, 0.0, 0.7]).filled(true).build();
            rect(tag.hp, tag.before.max(tag.hp), [0.85, 0.7, 0.2, 1.0]);
            rect(0.0, tag.hp, [0.6, 0.05, 0.05, 1.0]);
            if tag.dmg > 0 {
                let g = 10.0 * px;
                for (k, ch) in tag.dmg.to_string().bytes().enumerate() {
                    image(IMG_DIGITS + (ch - b'0') as usize, x0 + w + 6.0 + k as f32 * g * 0.75, y0 + h * 0.5 - g * 0.5, g, g);
                }
            }
        }
        // the power meter's height (None: out of the screen). Damage brings it down to HEIGHT,
        // a moment later it slides up to HEIGHT_TOP; healing just fills it where it is; back to
        // full health it stays LINGER s, then slides out of the screen.
        let now = Instant::now();
        let ease = |x: f32| {
            let x = x.clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        let hurt_height = |hit: Instant| {
            HEIGHT + (HEIGHT_TOP - HEIGHT) * ease((now.duration_since(hit).as_secs_f32() - METER_RISE_AFTER) / METER_RISE_TIME)
        };
        let long_ago = now.checked_sub(std::time::Duration::from_secs(60)).unwrap_or(now);
        let m = self.meter.get_or_insert(Meter { wedges, hit: long_ago, full: (wedges >= 8).then_some((long_ago, HEIGHT_GONE)) });
        if wedges < m.wedges {
            m.hit = now;
        }
        if wedges >= 8 && m.wedges < 8 {
            m.full = Some((now, hurt_height(m.hit)));
        } else if wedges < 8 {
            m.full = None;
        }
        m.wedges = wedges;
        let meter_height = match m.full {
            None => Some(hurt_height(m.hit)),
            Some((t, h0)) => {
                let out = (now.duration_since(t).as_secs_f32() - LINGER) / METER_RISE_TIME;
                (out < 1.0).then(|| h0 + (HEIGHT_GONE - h0) * ease(out))
            }
        };

        // the counters (SM64's layout: icon, x, digits 12 px apart, 15 px from the top), in the
        // top of the screen, always
        {
            let top = 15.0 * px;
            let counter = |icon: usize, x: f32, n: u32| {
                image(icon, x, top, 16.0 * px, 16.0 * px);
                image(IMG_MULTIPLY, x + 16.0 * px, top, 16.0 * px, 16.0 * px);
                for (k, ch) in n.to_string().bytes().enumerate() {
                    image(IMG_DIGITS + (ch - b'0') as usize, x + (32.0 + 12.0 * k as f32) * px, top, 16.0 * px, 16.0 * px);
                }
            };
            counter(IMG_HEAD, 22.0 * px, DEATHS.load(Ordering::Relaxed));
            // the coins right of the screen's centre, where SM64's widescreen ports put them
            counter(IMG_COIN, size[0] * COINS_X, COINS.load(Ordering::Relaxed));
            counter(IMG_STAR, size[0] - 78.0 * px, STARS.load(Ordering::Relaxed));
        }

        // the camera status, bottom right like SM64's (x 54 from the right, line 205): camera,
        // Lakitu, and the zoom arrow (down: zoomed out, up: the C-up view)
        if let Some(state) = crate::lakitu::hud_state() {
            let (x, y) = (size[0] - 54.0 * px, size[1] - (240.0 - 205.0) * px);
            image(IMG_CAMERA, x, y, 16.0 * px, 16.0 * px);
            image(IMG_LAKITU, x + 16.0 * px, y, 16.0 * px, 16.0 * px);
            match state {
                crate::lakitu::HudState::Far => image(IMG_ARROW_DOWN, x + 4.0 * px, y + 16.0 * px, 8.0 * px, 8.0 * px),
                crate::lakitu::HudState::FirstPerson => image(IMG_ARROW_UP, x + 4.0 * px, y - 8.0 * px, 8.0 * px, 8.0 * px),
                crate::lakitu::HudState::Near => {}
            }
        }

        // the power meter, centred at the top (its height worked out above)
        if let Some(height) = meter_height {
            let (cx, cy) = (size[0] * 0.5, size[1] * (1.0 - height / 240.0));
            image(IMG_BASE, cx - 32.0 * px, cy - 32.0 * px, 64.0 * px, 64.0 * px);
            if wedges > 0 {
                image(IMG_PIES + (wedges.min(8) - 1) as usize, cx - 16.0 * px, cy - 16.0 * px, 32.0 * px, 32.0 * px);
            }
        }
    }
}
