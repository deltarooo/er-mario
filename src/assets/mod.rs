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
pub mod tex;
pub mod yoshi;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{log, paths};

/// Bump when the generated files change, so existing installs rebuild.
const VERSION: &str = "er-mario assets 5";
const STAMP: &str = "package/.built";
const PIECES: [&str; 4] = ["hd", "bd", "am", "lg"];
const QUALITIES: [&str; 2] = ["hi", "low"];

/// The package files were complete when the game started (so me3 serves them this session).
static READY: AtomicBool = AtomicBool::new(false);
/// ...and Yoshi's among them: Torrent's model c8002 and its textures. They're only there if he
/// was found in the ROM.
static YOSHI: AtomicBool = AtomicBool::new(false);
const TORRENT: [&str; 3] = ["package/chr/c8002.chrbnd.dcx", "package/chr/c8002_h.texbnd.dcx", "package/chr/c8002_l.texbnd.dcx"];
/// The mesh of Torrent's model Yoshi goes into: his body, with the simple material
const TORRENT_BODY: usize = 7;

/// Torrent is Yoshi this session.
pub fn yoshi_ready() -> bool {
    YOSHI.load(Ordering::Relaxed)
}

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

/// A picture of the player's own for the Vagabond's card in character creation, next to the DLL.
/// Without it Mario is rendered from the ROM's model.
const PORTRAIT_FILE: &str = "portrait.png";

/// What the package was built from: a new version or another portrait file builds it again.
fn stamp() -> String {
    match std::fs::metadata(paths::file(PORTRAIT_FILE)) {
        Ok(m) => format!("{VERSION}, portrait {}", m.len()),
        Err(_) => VERSION.to_string(),
    }
}

/// portrait.png fitted into the card, as (hi, low) RGBA; None without the file.
fn own_portrait() -> Option<Result<(Vec<u8>, Vec<u8>), String>> {
    let file = std::fs::File::open(paths::file(PORTRAIT_FILE)).ok()?;
    Some((|| {
        let mut decoder = png::Decoder::new(file);
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let (w, h) = (info.width as usize, info.height as usize);
        let n = info.color_type.samples();
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return Err(format!("{w}x{h} is no size for a portrait"));
        }
        let mut src = tex::Image::new(w, h, [0.0; 4]);
        for (i, p) in buf[..w * h * n].chunks_exact(n).enumerate() {
            let v = |k: usize| p[k] as f32 / 255.0;
            let (rgb, a) = match n {
                1 => ([v(0); 3], 1.0),
                2 => ([v(0); 3], v(1)),
                3 => ([v(0), v(1), v(2)], 1.0),
                _ => ([v(0), v(1), v(2)], v(3)),
            };
            // premultiplied, so transparent pixels don't bleed their colour into the edges
            src.px[i] = [rgb[0] * a, rgb[1] * a, rgb[2] * a, a];
        }
        let (cw, ch) = icons::PORTRAIT;
        Ok((tex::fit(&src, cw, ch), tex::fit(&src, cw / 2, ch / 2)))
    })())
}

/// Checks the package once at startup. Returns true if it has to be built.
pub fn check() -> bool {
    let complete = std::fs::read_to_string(paths::file(STAMP)).is_ok_and(|s| s.trim() == stamp())
        && outputs().iter().all(|f| paths::file(f).is_file());
    READY.store(complete, Ordering::Relaxed);
    YOSHI.store(complete && TORRENT.iter().all(|f| paths::file(f).is_file()), Ordering::Relaxed);
    !complete
}

fn write(rel: &str, data: &[u8]) -> Result<(), String> {
    let path: PathBuf = paths::file(rel);
    // The folders one by one from the mod's own down, whatever each attempt answers. Not
    // create_dir_all: it goes by the error code to tell "the parent is missing" from the rest,
    // and on one player's PC that code came back as "already exists" for a folder that wasn't
    // there. Nothing was created and the build stopped. If a folder really can't be made, the
    // write below fails and names the file.
    let mut dir = paths::mod_dir().to_path_buf();
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop();
    for part in parts {
        dir.push(part);
        let _ = std::fs::create_dir(&dir);
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
    let (hi, low) = match own_portrait() {
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            log(format!("assets: {PORTRAIT_FILE} not used: {e}"));
            icons::portrait(model)
        }
        None => icons::portrait(model),
    };
    log(format!("assets: portrait done ({:.1} s)", t0.elapsed().as_secs_f32()));
    for (i, q) in QUALITIES.iter().enumerate() {
        let tpf = dcx::decompress(&archives.read(&format!("/menu/{q}/01_common.tpf.dcx"))?)?;
        let mut tpf = tex::patch_icon_atlas(&tpf, &icons)?;
        let (w, h) = icons::PORTRAIT;
        tex::patch_portrait(&mut tpf, i, if i == 0 { &hi } else { &low }, (w >> i, h >> i))?;
        write(&format!("package/menu/{q}/01_common.tpf.dcx"), &dcx::compress(&tpf)?)?;
    }
    step(0.95, "Building Yoshi");
    if let Err(e) = build_yoshi(&archives) {
        // (Torrent stays Torrent then: none of his files may be left half done)
        log(format!("assets: no Yoshi ({e})"));
        for f in TORRENT {
            let _ = std::fs::remove_file(paths::file(f));
        }
    }
    write(STAMP, stamp().as_bytes())?;
    log(format!("assets: all built in {:.1} s", t0.elapsed().as_secs_f32()));
    Ok(())
}

/// Yoshi in Torrent's place: his mesh into the body mesh of c8002, his colours and eyes over
/// the body's textures.
fn build_yoshi(archives: &archive::Archives) -> Result<(), String> {
    let rom = paths::read_rom()?;
    let model = yoshi::model(&rom).ok_or("he wasn't found in the ROM")?;
    let src = dcx::decompress(&archives.read("/chr/c8002.chrbnd.dcx")?)?;
    let mut files = bnd4::read(&src)?;
    let f = files.iter_mut().find(|f| f.name.to_lowercase().ends_with(".flver")).ok_or("Torrent's model has no FLVER")?;
    let fl = flver::Flver::new(std::mem::take(&mut f.data))?;
    f.data = flver::build_mount(fl, &crate::yoshi::BONES, &model.verts, &model.tris, TORRENT_BODY)?;
    let mut out = vec![(TORRENT[0], dcx::compress(&bnd4::build(&src, &files))?)];
    for path in &TORRENT[1..] {
        let src = dcx::decompress(&archives.read(&path["package".len()..])?)?;
        let mut files = bnd4::read(&src)?;
        let f = files.iter_mut().find(|f| f.name.to_lowercase().ends_with(".tpf")).ok_or("Torrent's textures have no TPF")?;
        f.data = tex::build_mount_tpf(&f.data, &model.albedo, "Body")?;
        out.push((path, dcx::compress(&bnd4::build(&src, &files))?));
    }
    // all three or none
    for (path, data) in &out {
        write(path, data)?;
    }
    log(format!("assets: Yoshi built ({} vertices)", model.verts.len()));
    Ok(())
}
