//! What's new, once per version, in the game's own message box.
//!
//! The title screen has a dialog for "the last session didn't end properly" (system message
//! 400003): the step that queues it skips the dialog unless the save check's result is 7 (an
//! abnormal exit). On the first launch of a new version that skip is taken out and the message's
//! text is swapped for the notes; both are put back once the player is in the world.

use std::time::{Duration, Instant};

use windows::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtect};

use crate::{log, names, paths};

/// Shown once after updating. Rewritten for every release (the box holds about 9 lines).
const NOTES: &str = concat!(
    "ER Mario ",
    env!("CARGO_PKG_VERSION"),
    " Patch notes:\n\n",
    "* Fixed the stutter framedrops many players had, especially on Windows.\n",
    "* Fixed Mario flicking between two directions with some controllers.\n",
    "* Fixed Yoshi not moving with keyboard controls.\n",
    "* Fewer hitches when new areas load.\n",
    "* Mario loses 2 pieces of health at most per hit.\n",
    "* Punches and kicks do as much damage as ground pounds, and more on bosses."
);

/// Which version's notes were shown last.
const SEEN: &str = "package/.notes";
/// System messages, and the "last session didn't end properly" text in it.
const FMG: usize = 203;
const MESSAGE: i32 = 400003;
/// mov rax,[rcx]; cmp dword [rax+4],7; jne skip (6 bytes); call ...; test al,al; je dialog
const CHECK: &str = "48 8b 01 83 78 04 07 0f 85 ?? ?? ?? ?? e8 ?? ?? ?? ?? 84 c0 74";
/// where the `jne skip` is in that, and its length
const JNE: usize = 7;
const JNE_LEN: usize = 6;

fn poke(addr: usize, bytes: &[u8]) -> bool {
    let mut old = PAGE_PROTECTION_FLAGS::default();
    if unsafe { VirtualProtect(addr as *const _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old) }.is_err() {
        return false;
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len()) };
    let mut back = PAGE_PROTECTION_FLAGS::default();
    let _ = unsafe { VirtualProtect(addr as *const _, bytes.len(), old, &mut back) };
    true
}

pub fn start() {
    let version = env!("CARGO_PKG_VERSION");
    if std::fs::read_to_string(paths::file(SEEN)).is_ok_and(|s| s.trim() == version) {
        return;
    }
    std::thread::spawn(move || {
        let sites = crate::equip::scan(CHECK);
        let [site] = sites.as_slice() else {
            log(format!("notes: the title screen's dialog check wasn't found ({} matches), not shown", sites.len()));
            return;
        };
        let jne = site + JNE;
        let original: [u8; JNE_LEN] = unsafe { *(jne as *const [u8; JNE_LEN]) };
        // no skip: the dialog shows whether or not the last exit was clean
        if !poke(jne, &[0x90; JNE_LEN]) {
            log("notes: could not patch the dialog check");
            return;
        }
        // the text loads a moment after the game starts
        let t0 = Instant::now();
        let patch = loop {
            if let Some(p) = names::override_id(FMG, MESSAGE, NOTES) {
                break Some(p);
            }
            if t0.elapsed() > Duration::from_secs(30) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        log(format!("notes: showing what's new in {version} ({})", if patch.is_some() { "text in place" } else { "text not found" }));
        while !crate::in_world() && t0.elapsed() < Duration::from_secs(600) {
            std::thread::sleep(Duration::from_millis(500));
        }
        poke(jne, &original);
        if let Some(p) = patch {
            names::restore(&[p]);
            if let Err(e) = std::fs::write(paths::file(SEEN), version) {
                log(format!("notes: could not remember they were shown: {e}"));
            }
        }
    });
}
