//! SM64's game-over screen instead of "YOU DIED": Bowser laughs while the view closes through a
//! Bowser-shaped hole to black, which stays until the player respawns.
//!
//! "YOU DIED" (and the "... FELLED" banners) go through CSMenuManImp::display_status_message; a
//! hook swallows "YOU DIED" in Mario mode and starts this instead (and a "felled" banner queues
//! Mario's star dance). The shape is SM64's own transition texture, read from the player's ROM (it
//! can't ship with the mod), drawn with the game's debug renderer on a plane just in front of the
//! camera.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;


use crate::log;

const STATUS_MESSAGE_YOU_DIED: i32 = 5;
/// "DEMIGOD FELLED", "LEGEND FELLED", "GREAT ENEMY FELLED"
const FELLED: [i32; 3] = [1, 2, 3];
const IRIS_SECONDS: f32 = 1.3;
/// Mario's own death animation plays before Bowser shows up
const DELAY_SECONDS: f32 = 3.0;
/// SOUND_MENU_BOWSER_LAUGH
pub const BOWSER_LAUGH: i32 = 0x7018_8081;

/// Set by the hook: "YOU DIED" was swallowed, start the game-over screen.
pub static STARTED: AtomicBool = AtomicBool::new(false);
/// Set by the hook: a boss "... FELLED" banner showed.
pub static BOSS_FELLED: AtomicBool = AtomicBool::new(false);
/// Whether Mario mode is on (the hook only swallows "YOU DIED" then).
pub static ACTIVE: AtomicBool = AtomicBool::new(false);

static STATE: Mutex<Option<Instant>> = Mutex::new(None);
static LAUGHED: AtomicBool = AtomicBool::new(false);
/// The Bowser mask as opaque runs (row, first column, last column + 1) on a 64x64 grid.
static RUNS: Mutex<Vec<(u8, u8, u8)>> = Mutex::new(Vec::new());

/// Decompresses a MIO0 block (SM64's compression).
pub fn mio0(data: &[u8], start: usize) -> Option<Vec<u8>> {
    let be32 = |o: usize| data.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize);
    if data.get(start..start + 4)? != b"MIO0" {
        return None;
    }
    let (size, comp_off, raw_off) = (be32(start + 4)?, be32(start + 8)?, be32(start + 12)?);
    let (mut layout, mut comp, mut raw) = (start + 16, start + comp_off, start + raw_off);
    let mut out = Vec::with_capacity(size);
    let (mut flags, mut bit) = (0u32, 0u32);
    while out.len() < size {
        if bit == 0 {
            flags = be32(layout)? as u32;
            layout += 4;
            bit = 32;
        }
        bit -= 1;
        if flags & (1 << bit) != 0 {
            out.push(*data.get(raw)?);
            raw += 1;
        } else {
            let v = u16::from_be_bytes(data.get(comp..comp + 2)?.try_into().ok()?) as usize;
            comp += 2;
            let (length, back) = ((v >> 12) + 3, (v & 0xFFF) + 1);
            for _ in 0..length {
                let b = *out.get(out.len().checked_sub(back)?)?;
                out.push(b);
            }
        }
    }
    Some(out)
}

/// Loads SM64's Bowser transition texture (texture_transition_bowser_half: IA8 32x64, the left half
/// of the face) from the US ROM's segment 2 and turns it into opaque runs.
pub fn load_mask(rom: &[u8]) {
    const SEGMENT2: usize = 0x108A40;
    const TEXTURE: usize = 0x142B8;
    let Some(seg) = mio0(rom, SEGMENT2) else {
        log("gameover: ROM segment 2 not found (not a US ROM?)");
        return;
    };
    let Some(tex) = seg.get(TEXTURE..TEXTURE + 2048) else { return };
    let mut runs = Vec::new();
    for y in 0..64u8 {
        let opaque = |x: u8| {
            // the texture's centre line is on its left edge: the left half of the face is the
            // texture mirrored, the right half is the texture as stored
            let hx = if x < 32 { 31 - x } else { x - 32 };
            tex[y as usize * 32 + hx as usize] & 0xF >= 8
        };
        let mut x = 0u8;
        while x < 64 {
            if opaque(x) {
                let start = x;
                while x < 64 && opaque(x) {
                    x += 1;
                }
                runs.push((y, start, x));
            } else {
                x += 1;
            }
        }
    }
    log(format!("gameover: Bowser mask loaded ({} runs)", runs.len()));
    *RUNS.lock().unwrap_or_else(|e| e.into_inner()) = runs;
}

type StatusFn = unsafe extern "C" fn(usize, i32) -> u8;

/// Hooks the status banner function ("YOU DIED", "... FELLED").
pub unsafe fn install_hook() {
    use ilhook::x64::{CallbackOption, HookFlags, hook_closure_retn};
    // CSMenuManImp::display_status_message (fromsoftware-rs keeps its RVA table private; same RVA in
    // its WW and JP tables for the supported game version, see version.rs)
    const DISPLAY_STATUS_MESSAGE_RVA: u32 = 0x7676a0;
    let Ok(base) = (unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }) else {
        log("gameover: game module not found");
        return;
    };
    let addr = base.0 as usize + DISPLAY_STATUS_MESSAGE_RVA as usize;
    let hook = move |reg: *mut ilhook::x64::Registers, original: usize| -> usize {
        let (this, message) = unsafe { ((*reg).rcx as usize, (*reg).rdx as i32) };
        if ACTIVE.load(Ordering::Relaxed) {
            if message == STATUS_MESSAGE_YOU_DIED {
                STARTED.store(true, Ordering::Relaxed);
                return 1; // swallowed: Mario's game over instead
            }
            if FELLED.contains(&message) {
                BOSS_FELLED.store(true, Ordering::Relaxed);
            }
        }
        let original: StatusFn = unsafe { std::mem::transmute(original) };
        unsafe { original(this, message) as usize }
    };
    match unsafe { hook_closure_retn(addr, hook, CallbackOption::None, HookFlags::empty()) } {
        Ok(h) => {
            std::mem::forget(h);
            log("gameover: hooked the status banners");
        }
        Err(e) => log(format!("gameover: hook failed: {e:?}")),
    }
}

/// Every frame in Mario mode: start / draw / end the game-over screen. `alive` is false while the
/// player is dead (the screen stays black until the respawn). Returns true when it just started
/// (so the caller plays Bowser's laugh).
pub fn update(alive: bool) -> bool {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let mut started = false;
    if STARTED.swap(false, Ordering::Relaxed) && state.is_none() {
        *state = Some(Instant::now());
    }
    let Some(t0) = *state else { return false };
    // never keep the player behind a black screen: if the game hasn't respawned them by now,
    // something else is going on
    if t0.elapsed().as_secs_f32() > DELAY_SECONDS + 15.0 {
        log("gameover: no respawn after 15 s, black screen removed");
        *state = None;
        LAUGHED.store(false, Ordering::Relaxed);
        return false;
    }
    let t = t0.elapsed().as_secs_f32() - DELAY_SECONDS;
    if t < 0.0 {
        return false;
    }
    // the laugh with the first frame of the iris
    if !LAUGHED.swap(true, Ordering::Relaxed) {
        started = true;
    }
    if alive && t > IRIS_SECONDS {
        *state = None; // respawned
        LAUGHED.store(false, Ordering::Relaxed);
        return started;
    }
    draw((t / IRIS_SECONDS).min(1.0));
    started
}

/// Whether the game-over screen is up (or about to be).
pub fn showing() -> bool {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

pub fn reset() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    LAUGHED.store(false, Ordering::Relaxed);
    STARTED.store(false, Ordering::Relaxed);
}

/// The iris for the overlay (hud.rs): its progress 0..1 and when it was last set (it goes when
/// the Mario frame stops, e.g. on the loading screen).
static IRIS: Mutex<Option<(f32, Instant)>> = Mutex::new(None);

fn draw(t: f32) {
    *IRIS.lock().unwrap_or_else(|e| e.into_inner()) = Some((t, Instant::now()));
}

/// The iris to draw now, if any: 0 (whole view) .. 1 (black).
pub fn iris() -> Option<f32> {
    // a prompt after death (revive at the Stake of Marika or the grace?) must be visible
    if crate::game_menu_open() {
        return None;
    }
    IRIS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .filter(|(_, at)| at.elapsed().as_secs_f32() < 0.2)
        .map(|(t, _)| t)
}

/// SM64's Bowser transition texture as a 64x64 black mask (the face mirrored around its centre
/// line, see load_mask), with its soft edges: alpha per pixel.
pub fn mask_alpha(rom: &[u8]) -> Option<Vec<u8>> {
    const SEGMENT2: usize = 0x108A40;
    const TEXTURE: usize = 0x142B8;
    let seg = mio0(rom, SEGMENT2)?;
    let tex = seg.get(TEXTURE..TEXTURE + 2048)?;
    let mut out = Vec::with_capacity(64 * 64);
    for y in 0..64usize {
        for x in 0..64usize {
            let hx = if x < 32 { 31 - x } else { x - 32 };
            out.push((tex[y * 32 + hx] & 0xF) * 17);
        }
    }
    Some(out)
}
