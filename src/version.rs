//! Game version check: the mod's game addresses (fromsoftware-rs, byte patterns, the status banner
//! function) are for one game version. On any other version the mod stays out of the way.

use fromsoftware_shared::game_version::{GameVersion, LANG_ID_EN, LANG_ID_JP};
use pelite::pe64::PeView;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

/// exe version 2.7.2.0 (worldwide) / 2.7.2.1 (Japan): what fromsoftware-rs supports
pub struct Supported;

impl GameVersion for Supported {
    const NAME: &'static str = "elden ring";

    fn from_lang_version(lang_id: u16, version: &str) -> Option<Self> {
        matches!((lang_id, version), (LANG_ID_EN, "2.7.2.0") | (LANG_ID_JP, "2.7.2.1")).then_some(Supported)
    }
}

/// Ok, or why the mod can't run on this game.
pub fn check() -> Result<(), String> {
    let module = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
    let view = unsafe { PeView::module(module.0 as *const u8) };
    std::panic::catch_unwind(|| Supported::detect(&view).map(|_| ()).map_err(|e| e.to_string()))
        .unwrap_or_else(|_| Err("could not read the game's version".into()))
}
