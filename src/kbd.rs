//! Mouse and keyboard for Mario, with the PC port's default keys (sm64ex / sm64coopdx):
//! WASD stick, L = A (jump), comma = B (punch), K = Z (crouch / ground pound), and the mouse:
//! right button = A, left button = B.
//!
//! The game reads the keyboard through DirectInput; in Mario mode a hook makes Mario's keys look
//! released to it, so WASD doesn't also walk the Tarnished (the mouse buttons' attack and guard
//! are already stripped from the Tarnished's actions).

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::{GUID, s, w};

use crate::log;

/// Hide Mario's keys from the game (set every frame by the mod).
pub static CAPTURE: AtomicBool = AtomicBool::new(false);

/// DirectInput scancodes the game must not see in Mario mode: W A S D, L, comma, K
const HIDDEN: [u32; 7] = [0x11, 0x1E, 0x1F, 0x20, 0x26, 0x33, 0x25];

fn hidden(k: u32) -> bool {
    HIDDEN.contains(&k) || (!crate::MENU_OPEN.load(Ordering::Relaxed) && (
        (crate::skate::enabled() && k == 0x2F)
        || (crate::skate::visual().mounted && matches!(k, 0x13 | 0x24 | 0x19))))
}

const VK_W: i32 = 0x57;
const VK_A: i32 = 0x41;
const VK_S: i32 = 0x53;
const VK_D: i32 = 0x44;
const VK_L: i32 = 0x4C;
const VK_K: i32 = 0x4B;
const VK_COMMA: i32 = 0xBC;
const VK_LBUTTON: i32 = 0x01;
const VK_RBUTTON: i32 = 0x02;

fn down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 != 0
}

/// Whether the game window has the focus (GetAsyncKeyState sees keys typed anywhere).
pub fn focused() -> bool {
    let hwnd: HWND = unsafe { GetForegroundWindow() };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid == unsafe { GetCurrentProcessId() }
}

pub struct Keys {
    pub skate_toggle: bool,
    pub skate_ollie: bool,
    pub skate_trick: u32,
    pub stick_x: f32,
    pub stick_y: f32,
    pub a: bool,
    pub b: bool,
    pub z: bool,
}

/// Mario's keyboard and mouse state (nothing when the game isn't focused).
pub fn read() -> Option<Keys> {
    if !focused() {
        return None;
    }
    let axis = |neg: i32, pos: i32| (down(pos) as i32 - down(neg) as i32) as f32;
    let (x, y) = (axis(VK_A, VK_D), axis(VK_W, VK_S));
    // full tilt diagonally too, like a stick pushed into the corner
    let len = (x * x + y * y).sqrt().max(1.0);
    Some(Keys {
        skate_toggle: down(0x56),
        skate_ollie: down(0x52),
        skate_trick: if down(0x50) { 3 } else if down(0x4A) { 2 } else { 0 },
        stick_x: x / len,
        stick_y: y / len,
        a: down(VK_L) || down(VK_RBUTTON),
        b: down(VK_COMMA) || down(VK_LBUTTON),
        z: down(VK_K),
    })
}

// ---- DirectInput hook ------------------------------------------------------------------------

const IID_IDIRECTINPUT8W: GUID = GUID::from_u128(0xBF798031_483A_4DA2_AA99_5D64ED369700);
const GUID_SYSKEYBOARD: GUID = GUID::from_u128(0x6F1D2B61_D5A0_11CF_BFC7_444553540000);
const DI8DEVTYPE_KEYBOARD: u32 = 0x13;

type DirectInput8Create = unsafe extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void, *mut c_void) -> i32;
type CreateDevice = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void, *mut c_void) -> i32;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;
type GetDeviceInfo = unsafe extern "system" fn(*mut c_void, *mut u8) -> i32;

/// device -> is a keyboard
static KEYBOARDS: Mutex<Option<HashMap<usize, bool>>> = Mutex::new(None);
static GET_INFO: Mutex<Option<usize>> = Mutex::new(None);

fn is_keyboard(device: usize) -> bool {
    let mut map = KEYBOARDS.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(HashMap::new);
    *map.entry(device).or_insert_with(|| {
        let Some(info) = *GET_INFO.lock().unwrap_or_else(|e| e.into_inner()) else { return false };
        let f: GetDeviceInfo = unsafe { std::mem::transmute(info) };
        // DIDEVICEINSTANCEW: dwSize, guidInstance, guidProduct, dwDevType, ...
        let mut buf = [0u8; 0x400];
        let size = 4 + 16 + 16 + 4 + 260 * 2 * 2 + 16 + 16 + 4 + 2;
        buf[..4].copy_from_slice(&(size as u32).to_le_bytes());
        let ok = unsafe { f(device as *mut c_void, buf.as_mut_ptr()) } >= 0;
        let kind = u32::from_le_bytes(buf[36..40].try_into().unwrap());
        ok && kind & 0xFF == DI8DEVTYPE_KEYBOARD
    })
}

/// Hooks the DirectInput device methods the game reads keys with (GetDeviceState for the whole
/// keyboard, GetDeviceData for buffered key events).
pub unsafe fn install_hooks() {
    use ilhook::x64::{CallbackOption, HookFlags, hook_closure_retn};
    let result = (|| -> Result<(), String> {
        let dll = unsafe { GetModuleHandleW(w!("dinput8.dll")).or_else(|_| LoadLibraryW(w!("dinput8.dll"))) }.map_err(|e| e.to_string())?;
        let create: DirectInput8Create =
            unsafe { std::mem::transmute(GetProcAddress(dll, s!("DirectInput8Create")).ok_or("no DirectInput8Create")?) };
        let hinst = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
        let mut di: *mut c_void = std::ptr::null_mut();
        if unsafe { create(hinst.0, 0x0800, &IID_IDIRECTINPUT8W, &mut di, std::ptr::null_mut()) } < 0 || di.is_null() {
            return Err("DirectInput8Create failed".into());
        }
        let di_vtbl = unsafe { *(di as *const *const usize) };
        let create_device: CreateDevice = unsafe { std::mem::transmute(*di_vtbl.add(3)) };
        let mut dev: *mut c_void = std::ptr::null_mut();
        if unsafe { create_device(di, &GUID_SYSKEYBOARD, &mut dev, std::ptr::null_mut()) } < 0 || dev.is_null() {
            return Err("CreateDevice(keyboard) failed".into());
        }
        let vtbl = unsafe { *(dev as *const *const usize) };
        let (get_state, get_data, get_info) = unsafe { (*vtbl.add(9), *vtbl.add(10), *vtbl.add(15)) };
        *GET_INFO.lock().unwrap_or_else(|e| e.into_inner()) = Some(get_info);
        // the methods live in dinput8.dll's code: the device can go again
        unsafe {
            (std::mem::transmute::<usize, Release>(*vtbl.add(2)))(dev);
            (std::mem::transmute::<usize, Release>(*di_vtbl.add(2)))(di);
        }

        // GetDeviceState(this, size, data): the whole keyboard as 256 bytes
        let state_hook = |reg: *mut ilhook::x64::Registers, original: usize| -> usize {
            let (this, size, data) = unsafe { ((*reg).rcx, (*reg).rdx as u32, (*reg).r8 as *mut u8) };
            let f: unsafe extern "system" fn(u64, u32, *mut u8) -> i32 = unsafe { std::mem::transmute(original) };
            let rc = unsafe { f(this, size, data) };
            if rc >= 0 && size == 256 && !data.is_null() && CAPTURE.load(Ordering::Relaxed) {
                for k in HIDDEN.into_iter().chain([0x2Fu32, 0x13, 0x24, 0x19]) {
                    if hidden(k) { unsafe { *data.add(k as usize) = 0 }; }
                }
            }
            rc as u32 as usize
        };
        // GetDeviceData(this, object size, data, in/out count, flags): buffered events
        let data_hook = |reg: *mut ilhook::x64::Registers, original: usize| -> usize {
            let (this, obj, data, count, flags) =
                unsafe { ((*reg).rcx, (*reg).rdx as u32, (*reg).r8 as *mut u8, (*reg).r9 as *mut u32, *(((*reg).rsp + 0x28) as *const u32)) };
            let f: unsafe extern "system" fn(u64, u32, *mut u8, *mut u32, u32) -> i32 = unsafe { std::mem::transmute(original) };
            let rc = unsafe { f(this, obj, data, count, flags) };
            if rc >= 0 && !data.is_null() && !count.is_null() && obj >= 8 && CAPTURE.load(Ordering::Relaxed) && is_keyboard(this as usize) {
                // DIDEVICEOBJECTDATA: dwOfs (the scancode), dwData (0x80 = pressed), ...
                for i in 0..unsafe { *count } as usize {
                    let e = unsafe { data.add(i * obj as usize) };
                    if hidden(unsafe { *(e as *const u32) }) {
                        unsafe { *(e.add(4) as *mut u32) = 0 };
                    }
                }
            }
            rc as u32 as usize
        };
        for (addr, name) in [(get_state, "GetDeviceState"), (get_data, "GetDeviceData")] {
            let res = if name == "GetDeviceState" {
                unsafe { hook_closure_retn(addr, state_hook, CallbackOption::None, HookFlags::empty()) }
            } else {
                unsafe { hook_closure_retn(addr, data_hook, CallbackOption::None, HookFlags::empty()) }
            };
            match res {
                Ok(h) => std::mem::forget(h),
                Err(e) => return Err(format!("{name}: {e:?}")),
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => log("kbd: hooked the game's keyboard (DirectInput)"),
        Err(e) => log(format!("kbd: keyboard hook failed ({e}); WASD also reaches the game")),
    }
}
