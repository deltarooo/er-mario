//! The game's other ways of reading a controller. Mario's sticks must not reach the game (it
//! walked the Tarnished under Mario, turning him back and forth every frame against Mario's
//! facing: Mario flickered between two directions while running). lib.rs hides them from
//! XInput, the way the game reads Xbox pads, but the game merges every pad it finds: under
//! Proton one controller can show up more than once (a second XInput slot, a DirectInput
//! gamepad, or libScePad, Sony's own pad library the game uses for PlayStation pads). Each of
//! those is hidden here the same way (lib.rs `hidden_sticks`), and logged when first seen.
//!
//! With `debug = 1`, every 2 s while a stick is pushed, what each of these reports (raw, before
//! hiding) and what the game's input devices hold goes to the log.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use eldenring::dluid::DLUserInputManagerImpl;
use fromsoftware_shared::FromStatic;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::GetProcAddress;
use windows::Win32::UI::Input::XboxController::XINPUT_STATE;
use windows::core::{GUID, s};

use crate::{debug, explore, hidden_sticks, log};

// ---- XInput (hooked in lib.rs) -----------------------------------------------------------------

/// Controller slots the game has read successfully (bit per slot).
static XINPUT_SEEN: AtomicU32 = AtomicU32::new(0);
/// Each slot's last raw left stick (before hiding), for the debug report.
static XINPUT_STICKS: Mutex<[Option<(i16, i16)>; 4]> = Mutex::new([None; 4]);

/// The game read controller `index` (0..3): note it (before its sticks are hidden).
pub fn xinput_seen(index: u32, s: &XINPUT_STATE) {
    let bit = 1 << index;
    if XINPUT_SEEN.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        log(if index == 0 {
            "pads: XInput controller 0 connected (Mario's)".to_string()
        } else {
            format!("pads: XInput controller {index} connected too (the game reads it as well: its sticks are hidden from the game like controller 0's)")
        });
    }
    if debug() {
        XINPUT_STICKS.lock().unwrap_or_else(|e| e.into_inner())[index as usize] = Some((s.Gamepad.sThumbLX, s.Gamepad.sThumbLY));
    }
}

// ---- DirectInput (GetDeviceState is hooked in kbd.rs) ------------------------------------------

/// DIPROPRANGE: DIPROPHEADER (size, header size, object, how) and the axis range
#[repr(C)]
struct DiPropRange {
    size: u32,
    header_size: u32,
    obj: u32,
    how: u32,
    min: i32,
    max: i32,
}

type GetProperty = unsafe extern "system" fn(*mut c_void, *const GUID, *mut DiPropRange) -> i32;

/// device -> its left stick's resting values (X, Y)
static DI_CENTRES: Mutex<Option<HashMap<usize, [i32; 2]>>> = Mutex::new(None);
/// The last raw left stick a DirectInput gamepad reported, for the debug report.
static DI_STICK: Mutex<Option<(usize, i32, i32)>> = Mutex::new(None);

/// The resting value of the axis at `offset` in DIJOYSTATE (the middle of its range).
fn axis_centre(device: usize, offset: u32) -> i32 {
    let vtbl = unsafe { *(device as *const *const usize) };
    // IDirectInputDevice8W: 5 = GetProperty; DIPROP_RANGE = MAKEDIPROP(4) (the GUID pointer is 4),
    // DIPH_BYOFFSET = 1
    let get: GetProperty = unsafe { std::mem::transmute(*vtbl.add(5)) };
    let mut p = DiPropRange { size: 24, header_size: 16, obj: offset, how: 1, min: 0, max: 0xFFFF };
    if unsafe { get(device as *mut c_void, 4 as *const GUID, &mut p) } < 0 {
        // DirectInput's default range
        (p.min, p.max) = (0, 0xFFFF);
    }
    ((p.min as i64 + p.max as i64) / 2) as i32
}

/// A DirectInput gamepad's state (DIJOYSTATE / DIJOYSTATE2: lX, lY first) was just read.
pub fn dinput_joystick(device: usize, data: *mut u8) {
    let centre = {
        let mut map = DI_CENTRES.lock().unwrap_or_else(|e| e.into_inner());
        *map.get_or_insert_with(HashMap::new).entry(device).or_insert_with(|| {
            let c = [axis_centre(device, 0), axis_centre(device, 4)];
            log(format!("pads: the game reads a gamepad through DirectInput ({device:#x}, left stick rests at {c:?}): its left stick is hidden from the game too"));
            c
        })
    };
    let axes = data as *mut i32;
    if debug() {
        *DI_STICK.lock().unwrap_or_else(|e| e.into_inner()) = Some((device, unsafe { *axes }, unsafe { *axes.add(1) }));
    }
    // (only the left stick: DirectInput pads put the right one on different axes, Z/Rz or Rx/Ry;
    // the game's camera underneath Lakitu's is all it could move)
    if hidden_sticks().0 {
        unsafe {
            *axes = centre[0];
            *axes.add(1) = centre[1];
        }
    }
}

// ---- libScePad ---------------------------------------------------------------------------------

static SCE_HOOKED: AtomicBool = AtomicBool::new(false);
static SCE_SEEN: AtomicBool = AtomicBool::new(false);
/// The last ScePadData's first 12 bytes (raw), for the debug report.
static SCE_RAW: Mutex<Option<[u8; 12]>> = Mutex::new(None);

/// Hooks scePadReadState(handle, ScePadData*) in the game's libScePad. ScePadData starts with the
/// buttons (u32), then the left stick (x, y) and the right stick (x, y), a byte each, 0x80 at rest.
fn hook_sce(module: usize) {
    use ilhook::x64::{CallbackOption, HookFlags, hook_closure_retn};
    let Some(read_state) = (unsafe { GetProcAddress(HMODULE(module as *mut c_void), s!("scePadReadState")) }) else {
        log("pads: libScePad is loaded but has no scePadReadState");
        return;
    };
    let hook = |reg: *mut ilhook::x64::Registers, original: usize| -> usize {
        let (handle, data) = unsafe { ((*reg).rcx as i32, (*reg).rdx as *mut u8) };
        let f: unsafe extern "C" fn(i32, *mut u8) -> i32 = unsafe { std::mem::transmute(original) };
        let rc = unsafe { f(handle, data) };
        if rc == 0 && !data.is_null() {
            if !SCE_SEEN.swap(true, Ordering::Relaxed) {
                log(format!("pads: the game reads a pad through libScePad (handle {handle}): its sticks are hidden from the game too"));
            }
            if debug() {
                *SCE_RAW.lock().unwrap_or_else(|e| e.into_inner()) = Some(unsafe { *(data as *const [u8; 12]) });
            }
            let (left, right) = hidden_sticks();
            unsafe {
                if left {
                    *data.add(4) = 0x80;
                    *data.add(5) = 0x80;
                }
                if right {
                    *data.add(6) = 0x80;
                    *data.add(7) = 0x80;
                }
            }
        }
        rc as u32 as usize
    };
    match unsafe { hook_closure_retn(read_state as usize, hook, CallbackOption::None, HookFlags::empty()) } {
        Ok(h) => {
            std::mem::forget(h);
            log("pads: hooked libScePad's scePadReadState");
        }
        Err(e) => log(format!("pads: libScePad hook failed: {e:?}")),
    }
}

// ---- the game's input devices ------------------------------------------------------------------

/// Every frame (any game state): hooks libScePad once the game has loaded it, logs the game's
/// input devices when they change, and the debug report.
pub fn task() {
    static NEXT: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut next = NEXT.lock().unwrap_or_else(|e| e.into_inner());
        if next.is_some_and(|t| Instant::now() < t) {
            return;
        }
        *next = Some(Instant::now() + Duration::from_secs(if debug() { 2 } else { 1 }));
    }
    let Ok(manager) = (unsafe { DLUserInputManagerImpl::instance() }) else { return };

    let sce = manager.lib_sce_pad_x64.0 as usize;
    if sce != 0 && !SCE_HOOKED.swap(true, Ordering::Relaxed) {
        hook_sce(sce);
    }

    // the device list, logged when it changes (a controller connecting adds one)
    let devices: Vec<usize> = manager.user_input_devices.iter().map(|p| p.as_ptr() as usize).collect();
    static LOGGED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
    if *logged != devices {
        let names: Vec<String> = devices.iter().map(|&d| format!("{} {d:#x}", explore::class_of(d).unwrap_or_else(|| "?".into()))).collect();
        log(format!(
            "pads: the game's input devices: {} | libScePad {} (in use: {}, {} pad handle(s))",
            names.join(", "),
            if sce != 0 { "loaded" } else { "not loaded" },
            manager.use_lib_sce_pad,
            manager.sce_pad_handles.iter().filter(|h| h.is_valid()).count(),
        ));
        *logged = devices.clone();
    }
    drop(logged);

    if debug() {
        report(&devices);
    }
}

/// Debug: while any source reports a pushed left stick, what each one read (raw, before hiding)
/// and the non-zero analog values each of the game's devices holds (after hiding).
fn report(devices: &[usize]) {
    let xinput = *XINPUT_STICKS.lock().unwrap_or_else(|e| e.into_inner());
    let di = *DI_STICK.lock().unwrap_or_else(|e| e.into_inner());
    let sce = *SCE_RAW.lock().unwrap_or_else(|e| e.into_inner());
    let di_centre = di.and_then(|(d, ..)| DI_CENTRES.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&d).copied()));
    let pushed = xinput.iter().flatten().any(|&(x, y)| (x as i32).abs() > 12000 || (y as i32).abs() > 12000)
        || di.zip(di_centre).is_some_and(|((_, x, y), c)| (x - c[0]).abs() > 4000 || (y - c[1]).abs() > 4000)
        || sce.is_some_and(|b| (b[4] as i32 - 0x80).abs() > 40 || (b[5] as i32 - 0x80).abs() > 40);
    if !pushed {
        return;
    }
    let mut line = format!("pads: stick pushed (left stick hidden from the game: {}) | xinput", hidden_sticks().0);
    for (i, s) in xinput.iter().enumerate() {
        if let Some((x, y)) = s {
            line += &format!(" [{i}] ({x}, {y})");
        }
    }
    if let Some((d, x, y)) = di {
        line += &format!(" | dinput {d:#x} ({x}, {y}) rest {di_centre:?}");
    }
    if let Some(b) = sce {
        line += &format!(" | libScePad {b:02x?}");
    }
    for &d in devices {
        let Ok(manager) = (unsafe { DLUserInputManagerImpl::instance() }) else { return };
        let Some(dev) = manager.user_input_devices.iter().find(|p| p.as_ptr() as usize == d) else { continue };
        let dev = unsafe { dev.as_ref() };
        let analog: Vec<String> = dev
            .virtual_input_data
            .analog_key_info
            .vector
            .iter()
            .enumerate()
            .filter(|(_, v)| v.abs() > 0.05)
            .map(|(i, v)| format!("{i}:{v:.2}"))
            .collect();
        if !analog.is_empty() {
            line += &format!(" | {} {d:#x} analog {}", explore::class_of(d).unwrap_or_else(|| "?".into()), analog.join(" "));
        }
    }
    log(line);
}
