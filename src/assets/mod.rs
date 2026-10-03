//! Builds the mod's game files on the player's PC: Mario's armour model (his mesh from libsm64,
//! which compiles in the SM64 decompilation's model code), his textures from the player's own SM64
//! ROM, and the menu icons rendered from both, patched into copies of the game's own files, read
//! straight from its archives.
//!
//! me3 picks up the package folder when the game starts, so a fresh build is used from the next
//! launch on; until then Mario mode stays off.

pub mod archive;
pub mod bnd4;
pub mod dcx;
pub mod flver;
pub mod icons;
pub mod matbin;
pub mod model;
pub mod skateboard;
pub mod tex;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{log, paths};

/// Bump when the generated files change, so existing installs rebuild.
const VERSION: &str = "er-mario assets 3";
const STAMP: &str = "package/.built";
const PIECES: [&str; 4] = ["hd", "bd", "am", "lg"];
const QUALITIES: [&str; 2] = ["hi", "low"];

/// The package files were complete when the game started (so me3 serves them this session).
static READY: AtomicBool = AtomicBool::new(false);

pub fn ready() -> bool {
    READY.load(Ordering::Relaxed)
}

fn outputs() -> Vec<String> {
    let mut out: Vec<String> = PIECES
        .iter()
        .flat_map(|p| ["", "_l"].map(|s| format!("package/parts/{p}_m_0999{s}.partsbnd.dcx")))
        .collect();
    out.push("package/material/allmaterial.matbinbnd.dcx".into());
    out.extend(QUALITIES.map(|q| format!("package/menu/{q}/01_common.tpf.dcx")));
    out
}

/// Checks the package once at startup. Returns true if it has to be built.
pub fn check() -> bool {
    let complete = std::fs::read_to_string(paths::file(STAMP)).is_ok_and(|s| s.trim() == VERSION)
        && outputs().iter().all(|f| paths::file(f).is_file());
    READY.store(complete, Ordering::Relaxed);
    !complete
}

fn write(rel: &str, data: &[u8]) -> Result<(), String> {
    let path: PathBuf = paths::file(rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Builds every package file from the exported Mario model.
pub fn build(model: &model::MarioModel) -> Result<(), String> {
    let t0 = std::time::Instant::now();
    let step = |p: f32, text: &str| crate::hud::setup_progress("Setting up ER Mario", p, text);
    step(0.05, "Reading the game archives");
    let _ = std::fs::remove_file(paths::file(STAMP));
    let archives = archive::Archives::open()?;
    log(format!("assets: archives indexed in {:.1} s", t0.elapsed().as_secs_f32()));
    let albedo = tex::mario_albedo(model);

    // the armour set, as its own model 999: Mario in the chest piece, the rest empty
    let mut done = 0.0;
    for piece in PIECES {
        for suffix in ["", "_l"] {
            step(0.15 + 0.5 * done / (PIECES.len() * 2) as f32, "Building the model and textures");
            done += 1.0;
            let src = dcx::decompress(&archives.read(&format!("/parts/{piece}_m_1280{suffix}.partsbnd.dcx"))?)?;
            let mut files = bnd4::read(&src)?;
            for f in &mut files {
                let lower = f.name.to_lowercase();
                if lower.ends_with(".flver") {
                    let fl = flver::Flver::new(std::mem::take(&mut f.data))?;
                    f.data = if piece == "bd" {
                        let mut d = flver::build_mario(fl, model, 1)?;
                        tex::rename(&mut d, "P[BD_M_1280]_Fabric.matxml", "P[BD_M_0999]_Fabric.matxml");
                        d
                    } else {
                        fl.empty()
                    };
                } else if lower.ends_with(".tpf") && piece == "bd" {
                    f.data = tex::build_tpf(&f.data, &albedo)?;
                    tex::rename(&mut f.data, "BD_M_1280_", "BD_M_0999_");
                }
                f.name = f.name.replace("_1280", "_0999");
            }
            write(&format!("package/parts/{piece}_m_0999{suffix}.partsbnd.dcx"), &dcx::compress(&bnd4::build(&src, &files))?)?;
        }
    }
    log(format!("assets: armour built ({:.1} s)", t0.elapsed().as_secs_f32()));
    step(0.7, "Building the material");

    // its material: the vanilla list plus P[BD_M_0999]_Fabric (renamed textures, no detail grime)
    let mat = dcx::decompress(&archives.read("/material/allmaterial.matbinbnd.dcx")?)?;
    let mut files = bnd4::read(&mat)?;
    let src = files.iter().find(|f| f.name.ends_with("P[BD_M_1280]_Fabric.matbin")).ok_or("Vagabond material not found")?;
    let mut data = src.data.clone();
    tex::rename(&mut data, "BD_M_1280", "BD_M_0999");
    matbin::clear_detail(&mut data);
    let new = bnd4::File { id: files.iter().map(|f| f.id).max().unwrap_or(0) + 1, name: src.name.replace("BD_M_1280", "BD_M_0999"), data };
    files.push(new);
    write("package/material/allmaterial.matbinbnd.dcx", &dcx::compress(&bnd4::build(&mat, &files))?)?;

    log(format!("assets: material built ({:.1} s)", t0.elapsed().as_secs_f32()));

    // menu icons
    step(0.75, "Building the menu icons");
    let icons = icons::render_all(model);
    log(format!("assets: icons rendered ({:.1} s)", t0.elapsed().as_secs_f32()));
    step(0.85, "Saving the menu icons");
    for q in QUALITIES {
        let tpf = dcx::decompress(&archives.read(&format!("/menu/{q}/01_common.tpf.dcx"))?)?;
        write(&format!("package/menu/{q}/01_common.tpf.dcx"), &dcx::compress(&tex::patch_icon_atlas(&tpf, &icons)?)?)?;
    }
    write(STAMP, VERSION.as_bytes())?;
    log(format!("assets: all built in {:.1} s", t0.elapsed().as_secs_f32()));
    Ok(())
}
