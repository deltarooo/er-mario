//! Mario rendered by the game itself: the Vagabond chest piece (bd_m_1280, built by
//! tools/build_mario_parts.py) carries Mario's mesh, each SM64 body part skinned 100% to one
//! skeleton bone with an identity bind. Every frame we write SM64's part matrices into those
//! bones' model pose (character space), after the game's animation and before rendering.

use std::sync::Mutex;

use glam::{Mat3, Quat, Vec3};

use crate::{explore, log};

/// SM64 part -> skeleton bone (same list as tools/build_mario_parts.py).
const PART_BONES: [&str; PARTS] = [
    "", "Pelvis_Mantle", "Spine2", "Neck", "L_ShoulderArmor", "L_Pectoral", "Collar", "R_Shoulder", "R_Pectoral", "Spine2_Mantle", "L_Hip", "SpineArmor1", "Spine_Mantle", "R_Hip", "SpineArmor2", "L_Shoulder",
    // eye variants (open, half, closed, dead): only the one SM64 is drawing is shown
    "L_UpArmTwist", "L_Elbow", "L_ForeArmTwist", "L_ForeArmTwist1",
    // the peace-sign right hand (star dance), shown instead of the fist
    "R_Elbow",
];
/// SM64's body parts (0..16) plus the eye variants (16..20).
pub const SM64_PARTS: usize = 16;
pub const PARTS: usize = 21;
/// SM64's right hand part, and our peace-sign copy of it
const RIGHT_HAND: usize = 9;
const PEACE: usize = 20;
/// Triangles SM64 draws for the right hand as a fist (the peace sign has more)
pub const FIST_TRIANGLES: usize = 46;
const HEAD: usize = 3;
/// SM64 eye texture cells for the variant parts 16..20
const EYE_CELLS: [u8; 4] = [5, 6, 7, 8];
/// Into the head, in the head part's mesh space (metres): minus the eyes' mean normal (0.185, 0.983, 0
/// in SM64's head space, X mirrored) times 6 mm.
const EYE_TUCK: Vec3 = Vec3::new(0.185 * 0.006, -0.983 * 0.006, 0.0);
/// Mario's model scale inside SM64's part matrices (the mesh already has it baked in).
const MODEL_SCALE: f32 = 0.25;

/// One part's transform relative to Mario, in (mirrored) ER world axes, metres.
#[derive(Clone, Copy)]
pub struct PartPose {
    pub rot: Quat,
    pub pos: Vec3,
    /// ~0 hides the part (unused eye variants)
    pub scale: f32,
}

/// Part poses relative to Mario from one SM64 tick's matrices; None if the layout is unexpected.
pub fn relative_parts(mats: &[f32], count: i32, mario: [f32; 3], eye_cell: u8, peace: bool) -> Option<[PartPose; PARTS]> {
    if (count as usize) < SM64_PARTS || mats.len() < SM64_PARTS * 16 {
        return None;
    }
    let mut out = [PartPose { rot: Quat::IDENTITY, pos: Vec3::ZERO, scale: 1.0 }; PARTS];
    for (i, p) in out.iter_mut().enumerate().take(SM64_PARTS).skip(1) {
        let m = &mats[i * 16..i * 16 + 16];
        // row-vector storage: rows of M are the columns of the column-vector matrix
        let a = Mat3::from_cols(Vec3::new(m[0], m[1], m[2]), Vec3::new(m[4], m[5], m[6]), Vec3::new(m[8], m[9], m[10]))
            * (1.0 / MODEL_SCALE);
        // SM64 grows the fist on a punch and the foot on a kick (up to 3x): that scale is in the
        // matrix on top of the model scale
        let grow = (a.x_axis.length() + a.y_axis.length() + a.z_axis.length()) / 3.0;
        p.scale = if grow.is_finite() && grow > 0.1 { grow } else { 1.0 };
        let q = Quat::from_mat3(&(a * (1.0 / p.scale))).normalize();
        // SM64 is mirrored on X: S R S
        p.rot = Quat::from_xyzw(q.x, -q.y, -q.z, q.w);
        let t = Vec3::new(m[12] - mario[0], m[13] - mario[1], m[14] - mario[2]) * crate::SCALE;
        p.pos = Vec3::new(-t.x, t.y, t.z);
    }
    // right hand: fist or peace sign (the hidden one shrinks; it only swaps twice per star dance)
    out[PEACE] = PartPose { scale: if peace { out[RIGHT_HAND].scale } else { 0.01 }, ..out[RIGHT_HAND] };
    if peace {
        out[RIGHT_HAND].scale = 0.01;
    }
    // eyes ride the head; only the variant SM64 is drawing is visible
    for (k, &cell) in EYE_CELLS.iter().enumerate() {
        // hidden variants stay full size, tucked 6 mm into the head behind the visible one (scaling
        // them away made the game's motion blur smear every blink; zero scale also darkened the model)
        let head = out[HEAD];
        let pos = if cell == eye_cell { head.pos } else { head.pos + head.rot * EYE_TUCK };
        out[SM64_PARTS + k] = PartPose { pos, ..head };
    }
    Some(out)
}

pub fn blend(a: &[PartPose; PARTS], b: &[PartPose; PARTS], t: f32) -> [PartPose; PARTS] {
    let mut out = *b;
    for i in 0..PARTS {
        if a[i].pos.distance(b[i].pos) < 2.0 {
            out[i].pos = a[i].pos.lerp(b[i].pos, t);
            out[i].rot = a[i].rot.slerp(b[i].rot, t);
        }
    }
    out
}

/// The pose to show this frame, in character space (set by the Mario frame, applied by `apply`).
pub static POSE: Mutex<Option<[PartPose; PARTS]>> = Mutex::new(None);

/// Converts world-relative part poses to the character space of a character facing `char_rot`.
pub fn to_character(parts: &[PartPose; PARTS], char_rot: Quat) -> [PartPose; PARTS] {
    let inv = char_rot.inverse();
    parts.map(|p| PartPose { rot: (inv * p.rot).normalize(), pos: inv * p.pos, scale: p.scale })
}

struct BoneMap {
    skeleton: usize,
    bones: [usize; PARTS],
}

static BONES: Mutex<Option<BoneMap>> = Mutex::new(None);

fn c_str(p: usize) -> Option<String> {
    if p == 0 || !explore::readable(p & !7, 72) {
        return None;
    }
    let mut s = Vec::new();
    for k in 0..64 {
        let b = unsafe { *((p + k) as *const u8) };
        if b == 0 {
            break;
        }
        s.push(b);
    }
    String::from_utf8(s).ok()
}

fn map_bones(skeleton: usize) -> Option<BoneMap> {
    let names = explore::read_u64(skeleton + 0x30)? as usize;
    let count = (explore::read_u64(skeleton + 0x38)? as u32 as usize).min(1024);
    if names == 0 || !explore::readable(names, count * 16) {
        return None;
    }
    let all: Vec<String> = (0..count).map(|i| c_str(unsafe { *((names + i * 16) as *const usize) } & !1).unwrap_or_default()).collect();
    let mut bones = [usize::MAX; PARTS];
    for (i, name) in PART_BONES.iter().enumerate().skip(1) {
        let Some(b) = all.iter().position(|n| n == name) else {
            log(format!("engine mario: bone {name} not found in {count} names (first {:?}, raw {:#x})", all.iter().take(5).collect::<Vec<_>>(), unsafe { *(names as *const usize) }));
            return None;
        };
        bones[i] = b;
    }
    log(format!("engine mario: skeleton {skeleton:#x} ({count} bones), part bones {:?}", &bones[1..]));
    Some(BoneMap { skeleton, bones })
}

#[derive(Clone, Copy, PartialEq)]
struct PoseLayout {
    imp: usize,
    skeleton: usize,
    model: usize,
    local: usize,
    parents: usize,
    count: usize,
}

static LAYOUT: Mutex<Option<(usize, PoseLayout)>> = Mutex::new(None);

/// The player's pose buffers, validated once per change of any pointer.
fn pose_layout(chr: usize, raw: impl Fn(usize) -> usize) -> Option<PoseLayout> {
    let mut cached = LAYOUT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((c, l)) = *cached {
        // cheap check: same character, same importer, same buffers
        if c == chr && raw(chr + 0x398) == l.imp && raw(l.imp + 0x60) == l.model && raw(l.imp + 0x50) == l.local {
            return Some(l);
        }
    }
    let imp = explore::read_u64(chr + 0x398)? as usize;
    let skeleton = explore::read_u64(imp + 0x48)? as usize;
    let model = explore::read_u64(imp + 0x60)? as usize;
    let count = explore::read_u64(imp + 0x68)? as u32 as usize;
    let local = explore::read_u64(imp + 0x50)? as usize;
    let local_count = explore::read_u64(imp + 0x58)? as u32 as usize;
    let parents = explore::read_u64(skeleton + 0x20)? as usize;
    let parent_count = explore::read_u64(skeleton + 0x28)? as u32 as usize;
    if count == 0 || count > 1024 || local_count != count || parent_count != count {
        return None;
    }
    if !explore::readable(model, count * 0x30) || !explore::readable(local, count * 0x30) || !explore::readable(parents & !7, count * 2 + 8) {
        return None;
    }
    let l = PoseLayout { imp, skeleton, model, local, parents, count };
    *cached = Some((chr, l));
    Some(l)
}

/// Where the game's character animation job has just written a pose (found with a hardware
/// watchpoint on a bone; run for every character, on the game's worker threads): right after the call that
/// writes the model pose (the job goes on to hand the bones to rendering, so later is too late),
/// and the job's return as a fallback.
const ANIM_DONE_RVAS: [usize; 2] = [0x41daa4, 0x402224];

/// The renderer draws the previous frame while the next one updates: without this, Mario's bones
/// sit in the Tarnished's animated pose from the animation job until our next task runs, and
/// anything reading them in between (shadows) gets the Tarnished's shape. Putting our pose back the
/// moment the animation job is done closes that window.
pub unsafe fn install_anim_hook() {
    use ilhook::x64::{CallbackOption, HookFlags, hook_closure_jmp_back};
    let Ok(base) = (unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }) else { return };
    for rva in ANIM_DONE_RVAS {
        let hook = |_: *mut ilhook::x64::Registers| {
            let _ = std::panic::catch_unwind(reassert);
        };
        match unsafe { hook_closure_jmp_back(base.0 as usize + rva, hook, CallbackOption::None, HookFlags::empty()) } {
            Ok(h) => {
                std::mem::forget(h);
                log(format!("engine mario: hooked the animation job at +{rva:#x}"));
            }
            Err(e) => log(format!("engine mario: animation hook at +{rva:#x} failed: {e:?}")),
        }
    }
}

/// Our pose was overwritten (the head bone isn't where we put it): write it again.
fn reassert() {
    if !crate::ENABLED.load(std::sync::atomic::Ordering::Relaxed) || POSE.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
        return;
    }
    // (waits if one of our pose tasks is writing right now: it runs alongside the animation jobs,
    // and the animation may land after its write)
    let Some((chr, l)) = *LAYOUT.lock().unwrap_or_else(|e| e.into_inner()) else { return };
    let Some(b) = BONES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|m| m.bones[HEAD]) else { return };
    let Some(want) = *LAST_HEAD.lock().unwrap_or_else(|e| e.into_inner()) else { return };
    // the cached character may be gone after a load: only the live player, with the same pose
    // buffers (no memory checks here: this runs on the game's animation workers many times a frame)
    use fromsoftware_shared::FromStatic;
    let live = unsafe { eldenring::cs::WorldChrMan::instance() }
        .ok()
        .and_then(|w| w.main_player.as_ref())
        .map(|p| &p.chr_ins as *const _ as usize);
    let raw = |a: usize| unsafe { *(a as *const usize) };
    if live != Some(chr) || b >= l.count || raw(chr + 0x398) != l.imp || raw(l.imp + 0x60) != l.model {
        return;
    }
    let now = unsafe { *((l.model + b * 0x30) as *const [f32; 3]) };
    if Vec3::from(now).distance(Vec3::from(want)) > 1e-4 {
        apply(chr);
    }
}

/// Where apply last put the head bone (model space).
static LAST_HEAD: Mutex<Option<[f32; 3]>> = Mutex::new(None);

/// Writes the Mario pose into the player's render skeleton (after the game's animation, in every
/// task group up to drawing).
pub fn apply(chr: usize) {
    let Some(pose) = *POSE.lock().unwrap_or_else(|e| e.into_inner()) else { return };
    // ChrIns+0x398 CSFD4LocationHkaPoseImporter: +0x48 hkaSkeleton, +0x50 local / +0x60 model pose
    // (hkQsTransform, 0x30 each, counts at +0x58/+0x68); hkaSkeleton +0x20 parent indices.
    // Validating memory costs a slow Wine server call, and this runs ~9 times a frame: validate once,
    // then only re-validate when one of the pointers changes.
    let raw = |a: usize| unsafe { *(a as *const usize) };
    let Some(layout) = pose_layout(chr, raw) else { return };
    let PoseLayout { imp, skeleton, .. } = layout;
    let mut guard = BONES.lock().unwrap_or_else(|e| e.into_inner());
    if guard.as_ref().is_none_or(|b| b.skeleton != skeleton) {
        *guard = map_bones(skeleton);
    }
    let Some(map) = guard.as_ref() else {
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log(format!("engine mario: bone mapping failed (importer {imp:#x}, skeleton {skeleton:#x})"));
        }
        return;
    };
    if let Some(head) = write_pose(&layout, &map.bones, &pose, false) {
        *LAST_HEAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(head);
    }
}

/// Mario's pose for a skeleton outside the world (the character creation preview, menu_mario).
pub static STANDING: Mutex<Option<[PartPose; PARTS]>> = Mutex::new(None);

pub fn set_standing(parts: &[PartPose; PARTS]) {
    // facing the camera
    let pose = to_character(parts, Quat::from_rotation_y(std::f32::consts::PI));
    *STANDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(pose);
}

/// The save's picture (pause menu, quit screen) is taken of a menu model, head and shoulders of
/// a person: Mario is made smaller and held up so his head is what's in it (scale, m).
const PICTURE_SIZE: f32 = 0.5;
const PICTURE_LIFT: f32 = 1.07;
/// (degrees, and about where his head is at full size, m)
const PICTURE_TILT: f32 = 25.0;
const PICTURE_HEAD: Vec3 = Vec3::new(0.0, 0.8, 0.0);

/// Mario standing still (menu_mario::still), in character space.
static STILL: Mutex<Option<[PartPose; PARTS]>> = Mutex::new(None);

pub fn set_still(parts: &[PartPose; PARTS]) {
    // facing the picture's camera, and leaning back around his head to look up at it (it looks
    // down on a person's face)
    let tilt = Quat::from_rotation_x(PICTURE_TILT.to_radians());
    let pose = to_character(parts, Quat::from_rotation_y(std::f32::consts::PI)).map(|q| PartPose { rot: (tilt * q.rot).normalize(), pos: PICTURE_HEAD + tilt * (q.pos - PICTURE_HEAD), ..q });
    *STILL.lock().unwrap_or_else(|e| e.into_inner()) = Some(pose);
}

/// In the world: the menus' Mario models stand still, so the picture is the same every time.
pub fn stand_for_picture() {
    let Some(pose) = *STILL.lock().unwrap_or_else(|e| e.into_inner()) else { return };
    let pose = pose.map(|q| PartPose { pos: q.pos * PICTURE_SIZE + Vec3::Y * PICTURE_LIFT, scale: q.scale * PICTURE_SIZE, ..q });
    *STANDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(pose);
}

/// hkaPose::syncModelSpace: every skeleton's model pose goes through it, the menus' too (the
/// character creation preview isn't a ChrIns and never reaches the animation job above).
const SYNC_MODEL_RVA: usize = 0x16568b0;
const SYNC_MODEL_CODE: [u8; 10] = [0x48, 0x83, 0xec, 0x18, 0x80, 0x79, 0x38, 0x00, 0x0f, 0x85];

pub unsafe fn install_menu_hook() {
    use ilhook::x64::{CallbackOption, HookFlags, hook_closure_retn};
    let Ok(base) = (unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }) else { return };
    let at = base.0 as usize + SYNC_MODEL_RVA;
    if unsafe { *(at as *const [u8; 10]) } != SYNC_MODEL_CODE {
        log("engine mario: pose sync not where it's expected, no Mario in character creation");
        return;
    }
    let hook = |r: *mut ilhook::x64::Registers, original: usize| -> usize {
        let pose = unsafe { (*r).rcx } as usize;
        let sync: extern "win64" fn(usize) = unsafe { std::mem::transmute(original) };
        sync(pose);
        if HAVE_RENDS.load(std::sync::atomic::Ordering::Relaxed) && ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = std::panic::catch_unwind(|| {
                note_importer(pose);
                if ANY.load(std::sync::atomic::Ordering::Relaxed) {
                    menu_pose(pose);
                }
            });
        }
        0
    };
    match unsafe { hook_closure_retn(at, hook, CallbackOption::None, HookFlags::empty()) } {
        Ok(h) => {
            std::mem::forget(h);
            log("engine mario: hooked the pose sync");
        }
        Err(e) => {
            log(format!("engine mario: pose sync hook failed: {e:?}"));
            return;
        }
    }
    let at = base.0 as usize + REND_UPDATE_RVA;
    if unsafe { *(at as *const [u8; 16]) } != REND_UPDATE_CODE {
        log("engine mario: menu model update not where it's expected, no Mario in character creation");
        return;
    }
    let seen = |r: *mut ilhook::x64::Registers| {
        let _span = crate::perf::span(crate::perf::MENU_MODEL);
        let rend = unsafe { (*r).rcx } as usize;
        let mut rends = RENDS.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        if let Some(e) = rends.iter_mut().find(|e| e.0 == rend) {
            e.2 = now;
        } else if rends.len() < 16 {
            rends.push((rend, now, now));
            log(format!("menu pose: a menu model appears ({} now)", rends.len()));
        }
        HAVE_RENDS.store(true, std::sync::atomic::Ordering::Relaxed);
    };
    match unsafe { ilhook::x64::hook_closure_jmp_back(at, seen, CallbackOption::None, HookFlags::empty()) } {
        Ok(h) => {
            std::mem::forget(h);
            MENU_HOOKS.store(true, std::sync::atomic::Ordering::Relaxed);
            log("engine mario: hooked the menu model update");
        }
        Err(e) => log(format!("engine mario: menu model hook failed: {e:?}")),
    }
}

/// CSMenuAsmModelRend's update (its task at +0xe0 calls it every frame): the menus' character
/// models. Character creation keeps four alive, a dressed and a bare one per body type, and shows
/// one of them.
const REND_UPDATE_RVA: usize = 0xbbc2c0;
const REND_UPDATE_CODE: [u8; 16] = [0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x20, 0x48, 0x8b, 0xd9, 0x48, 0x8b, 0xfa];
/// The renderers updated lately: (address, first seen, last seen).
static RENDS: Mutex<Vec<(usize, std::time::Instant, std::time::Instant)>> = Mutex::new(Vec::new());
/// Mario is wanted on the menus' models (set every frame, lib.rs).
pub static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// MARIO_IMPORTERS has any (the pose sync runs for every skeleton in the game).
static ANY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Both menu hooks are in (they aren't on a game version they don't know): without them the
/// menus' models keep the Vagabond's look.
pub static MENU_HOOKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// RENDS has any.
static HAVE_RENDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A pose importer's vtable, once known.
static IMPORTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The importers synced lately, sorted: (address, when). Filled while menu models exist, so
/// finding a model's own is comparing addresses, not reading every object it points at.
static LIVE: Mutex<Vec<(usize, std::time::Instant)>> = Mutex::new(Vec::new());

fn note_importer(pose: usize) {
    let imp = pose.wrapping_sub(0x48);
    let vtable = IMPORTER.load(std::sync::atomic::Ordering::Relaxed);
    if vtable == 0 || explore::read_u64(imp) != Some(vtable as u64) {
        return;
    }
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    match live.binary_search_by_key(&imp, |e| e.0) {
        Ok(i) => live[i].1 = now,
        Err(i) if live.len() < 4096 => live.insert(i, (imp, now)),
        Err(_) => {}
    }
}
/// The pose importers of the renderers that wear the Mario set (refreshed by `menu_models`).
static MARIO_IMPORTERS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Every frame outside the world: which of the menus' character models wear the Mario set. Only
/// their skeletons get Mario's pose (the bare ones would be stretched over it). True if any does.
pub fn menu_models() -> bool {
    find_models(false)
}

/// `now`: don't wait for the next round (a new model: the save's picture is taken of one that
/// only lives for a moment).
fn find_models(now: bool) -> bool {
    use std::sync::atomic::Ordering;
    let chest = crate::equip::MARIO_CHEST;
    // (a model can be updated once and then sit there while its parts load: it's kept for a
    // while after its last update. Reading one that's gone is harmless, only poses the game
    // itself syncs are written to)
    let (rends, young): (Vec<usize>, bool) = {
        let mut r = RENDS.lock().unwrap_or_else(|e| e.into_inner());
        r.retain(|e| e.2.elapsed().as_secs_f32() < 3.0);
        HAVE_RENDS.store(!r.is_empty(), Ordering::Relaxed);
        (r.iter().map(|e| e.0).collect(), r.iter().any(|e| e.1.elapsed().as_secs_f32() < 3.0))
    };
    // (ten times a second while a model is new, a few times for those that stay: the walk
    // asks Windows about a lot of memory, and that can take a quarter of a millisecond a time)
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let every = if now || young { 0.1 } else { 0.25 };
        if last.is_some_and(|t| t.elapsed().as_secs_f32() < every) {
            return ANY.load(Ordering::Relaxed);
        }
        *last = Some(std::time::Instant::now());
    }
    if rends.is_empty() {
        let mut known = MARIO_IMPORTERS.lock().unwrap_or_else(|e| e.into_inner());
        known.clear();
        ANY.store(false, Ordering::Relaxed);
        return false;
    }
    let _span = crate::perf::span(crate::perf::MENU_WALK);
    let pointer = |at: usize| explore::read_u64(at).map(|p| p as usize).filter(|p| *p > 0x10000 && p % 8 == 0 && p >> 47 == 0);
    let live: Vec<usize> = {
        let mut l = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        l.retain(|e| e.1.elapsed().as_secs_f32() < 1.0);
        l.iter().map(|e| e.0).collect()
    };
    let is_live = |p: usize| live.binary_search(&p).is_ok();
    // the pointers in `len` bytes of an object (checked once, then read as they are)
    let pointers = |obj: usize, len: usize| -> Vec<usize> {
        if !explore::readable(obj, len) {
            return Vec::new();
        }
        (0..len).step_by(8).map(|off| unsafe { *((obj + off) as *const usize) }).filter(|p| *p > 0x10000 && p % 8 == 0 && p >> 47 == 0).collect()
    };
    // which vtables are armour pieces (class names are slow to read: once per kind)
    static PIECES: Mutex<Vec<(usize, bool)>> = Mutex::new(Vec::new());
    let is_piece = |obj: usize| -> bool {
        let Some(vtable) = explore::read_u64(obj).map(|v| v as usize) else { return false };
        let mut known = PIECES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(k) = known.iter().find(|k| k.0 == vtable) {
            return k.1;
        }
        let piece = explore::class_of(obj).is_some_and(|c| c.ends_with("PartsModelIns"));
        if known.len() < 256 {
            known.push((vtable, piece));
        }
        piece
    };
    let mut found = Vec::new();
    for rend in rends {
        // renderer +0x1ac the armour ids of its ChrAsm (head, chest, arms, legs from +0x1dc),
        // +0x770 the body's model, +0x778 the armour's (its parts are models of their own)
        if explore::read_u64(rend + 0x1e0).map(|v| v as u32 as i32) != Some(chest) {
            continue;
        }
        // the body's pose importer: model +0x18 display entity, its +0x210 exporter, that one's
        // +0x158 source
        let body = pointer(rend + 0x770).and_then(|m| pointer(m + 0x18)).and_then(|e| pointer(e + 0x210)).and_then(|e| pointer(e + 0x158));
        // what an importer is (its vtable): from this body, or from the player in the world
        // (ChrIns +0x398). The picture's model doesn't always have a body of its own.
        if IMPORTER.load(Ordering::Relaxed) == 0 {
            use fromsoftware_shared::FromStatic;
            let player = unsafe { eldenring::cs::WorldChrMan::instance() }
                .ok()
                .and_then(|w| w.main_player.as_ref())
                .and_then(|p| pointer(&p.chr_ins as *const _ as usize + 0x398));
            if let Some(imp) = [body, player].into_iter().flatten().find(|p| explore::class_of(*p).as_deref() == Some("CS::CSFD4LocationHkaPoseImporter")) {
                IMPORTER.store(explore::read_u64(imp).unwrap_or(0) as usize, Ordering::Relaxed);
            }
            // (the sync hook fills LIVE from the next frame on)
            continue;
        }
        if let Some(body) = body.filter(|b| is_live(*b)) {
            found.push(body);
        }
        // an armour piece's importer: the piece points at it itself, or it's on the piece's
        // cloth instance (piece +0x130 or +0x2f0, then +0x120). Only these places are read:
        // following every pointer a piece holds meant asking Windows about hundreds of
        // addresses, which takes a millisecond each on some systems (0.3 s a walk, and the
        // game makes the save picture's model every so often while playing).
        let Some(asm) = pointer(rend + 0x778) else { continue };
        for part in pointers(asm, 0x200).into_iter().filter(|p| is_piece(*p)) {
            found.extend(pointers(part, 0x3b0).into_iter().filter(|p| is_live(*p)));
            for cloth in [0x130, 0x2f0] {
                if let Some(imp) = pointer(part + cloth).and_then(|c| pointer(c + 0x120)).filter(|p| is_live(*p)) {
                    found.push(imp);
                }
            }
        }
    }
    found.sort();
    found.dedup();
    let mut known = MARIO_IMPORTERS.lock().unwrap_or_else(|e| e.into_inner());
    if *known != found {
        crate::dlog(format!("menu pose: Mario's skeletons {found:x?}"));
        *known = found;
    }
    ANY.store(!known.is_empty(), Ordering::Relaxed);
    !known.is_empty()
}

/// A pose that was just synced outside the world: if it's in one of Mario's pose importers (the
/// hkaPose is the importer's +0x48), Mario stands in it.
fn menu_pose(pose: usize) {
    let imp = pose.wrapping_sub(0x48);
    if !MARIO_IMPORTERS.lock().unwrap_or_else(|e| e.into_inner()).contains(&imp) {
        return;
    }
    let Some(stand) = *STANDING.lock().unwrap_or_else(|e| e.into_inner()) else { return };
    let layout = (|| {
        let skeleton = explore::read_u64(imp + 0x48)? as usize;
        let model = explore::read_u64(imp + 0x60)? as usize;
        let count = explore::read_u64(imp + 0x68)? as u32 as usize;
        let local = explore::read_u64(imp + 0x50)? as usize;
        let parents = explore::read_u64(skeleton + 0x20)? as usize;
        (count > 0 && count <= 1024 && explore::readable(model, count * 0x30) && explore::readable(local, count * 0x30) && explore::readable(parents & !7, count * 2 + 8))
            .then_some(PoseLayout { imp, skeleton, model, local, parents, count })
    })();
    let Some(layout) = layout else { return };
    // (the two body types have their own skeletons)
    static MAPS: Mutex<Vec<(usize, Option<[usize; PARTS]>)>> = Mutex::new(Vec::new());
    let mut maps = MAPS.lock().unwrap_or_else(|e| e.into_inner());
    let bones = match maps.iter().find(|m| m.0 == layout.skeleton) {
        Some(m) => m.1,
        None => {
            let b = map_bones(layout.skeleton).map(|m| m.bones);
            if maps.len() < 32 {
                maps.push((layout.skeleton, b));
            }
            b
        }
    };
    if let Some(bones) = bones {
        write_pose(&layout, &bones, &stand, true);
    }
}

/// Puts the part poses on their bones and hides the rest; returns where the head bone went.
/// `away`: hidden bones also go far below, each hidden branch by its first bone (in the menus the
/// weapons hang on theirs and don't shrink with them; in the world the game aims the camera at
/// some, so they stay put there).
fn write_pose(layout: &PoseLayout, bones: &[usize; PARTS], pose: &[PartPose; PARTS], away: bool) -> Option<[f32; 3]> {
    let PoseLayout { model, local, parents, count, .. } = *layout;
    type Qs = (Vec3, Quat);
    let read = |base: usize, b: usize| -> Qs {
        let v = unsafe { *((base + b * 0x30) as *const [f32; 12]) };
        (Vec3::new(v[0], v[1], v[2]), Quat::from_xyzw(v[4], v[5], v[6], v[7]).normalize())
    };
    let write = |base: usize, b: usize, t: Qs, s: f32| unsafe {
        *((base + b * 0x30) as *mut [f32; 12]) = [t.0.x, t.0.y, t.0.z, 0.0, t.1.x, t.1.y, t.1.z, t.1.w, s, s, s, 0.0];
    };
    let n = count.min(512);
    let parent_of = |b: usize| unsafe { *((parents + b * 2) as *const i16) };
    let mut pinned: [Option<(Qs, f32)>; 512] = [None; 512];
    // bones that must keep scale 1: Mario's bones and their ancestors (the rest is scaled to 0,
    // which hides everything else on the Tarnished: weapons, face, fingers...)
    let mut keep = [false; 512];
    for i in 1..PARTS {
        let mut b = bones[i];
        if b >= n {
            continue;
        }
        pinned[b] = Some(((pose[i].pos, pose[i].rot), pose[i].scale));
        while b < n && !keep[b] {
            keep[b] = true;
            let p = parent_of(b);
            if p < 0 {
                break;
            }
            b = p as usize;
        }
    }
    let mut world: Vec<Qs> = Vec::with_capacity(n);
    for b in 0..n {
        let p = parent_of(b);
        let parent: Qs = if p >= 0 && (p as usize) < b { world[p as usize] } else { (Vec3::ZERO, Quat::IDENTITY) };
        let scale = match pinned[b] {
            Some((_, s)) => s,
            None if keep[b] => 1.0,
            None => 0.0,
        };
        let m = match pinned[b] {
            Some((want, s)) => {
                let inv = parent.1.inverse();
                write(local, b, (inv * (want.0 - parent.0), (inv * want.1).normalize()), s);
                want
            }
            None if keep[b] => {
                let l = read(local, b);
                (parent.0 + parent.1 * l.0, (parent.1 * l.1).normalize())
            }
            None => {
                // hidden: scaled to 0 where the animation puts it (not moved: the game aims the camera
                // at bones like the head, e.g. when resting at a grace)
                let mut l = read(local, b);
                if away && (p < 0 || keep[p as usize]) {
                    l.0.y -= 1000.0;
                }
                write(local, b, l, 0.0);
                (parent.0 + parent.1 * l.0, (parent.1 * l.1).normalize())
            }
        };
        write(model, b, m, scale);
        world.push(m);
    }
    world.get(bones[HEAD]).map(|head| head.0.into())
}
