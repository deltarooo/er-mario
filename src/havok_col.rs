//! Real collision: reads Elden Ring's live Havok world and hands Mario the game's own triangles.
//!
//! Layout (found with the explorer, see tools/decode_near.py):
//!   CSHavokMan+0x98 -> CSPhysWorld, +0x8 -> hknpWorld
//!   hknpWorld+0x28 bodies (0xb0 each, count at +0x30)
//!   body +0x30 translation, +0x60 shape, +0x6c collision layer/filter, +0x80 rotation quat (xyzw)
//!   fsnpCustomParamCompressedMeshShape +0x48 -> hknpCompressedMeshShapeData:
//!     +0x30 domain min, +0x40 domain max, +0x60 sections (0x60 each), +0x70 primitives (u8 x4),
//!     +0x80 shared index (u16), +0x90 packed vertices (u32 11/11/10), +0xa0 shared vertices (u64 21/21/22)
//!   section +0x30 offset, +0x3c scale, +0x48 first packed, +0x4c shared start, +0x50 prim start,
//!           +0x58 byte0 = packed count, byte1 = primitive count

use std::collections::HashMap;
use std::sync::Arc;

use eldenring::cs::CSHavokMan;
use fromsoftware_shared::FromStatic;
use glam::{Quat, Vec3};

use crate::explore::{class_of, read_u64, readable};
use crate::log;

const CELL: f32 = 4.0; // metres, per-shape bucket size
pub const RADIUS: f32 = 4.0; // horizontal query radius around Mario (m)
pub const BELOW: f32 = 40.0; // floor column depth under Mario (m)
pub const BOX_DOWN: f32 = 3.0;
pub const COLUMN: f32 = 1.5; // floor column half-width (m)
pub const ABOVE: f32 = 3.0;
const MAX_TRIS: usize = 8000;

pub type Tri = [Vec3; 3];

/// A decoded mesh in shape-local space, bucketed into cells for quick queries.
pub struct Mesh {
    tris: Vec<Tri>,
    cells: HashMap<(i32, i32, i32), Vec<u32>>,
    radius: f32,
}

impl Mesh {
    pub fn tris(&self) -> &[Tri] {
        &self.tris
    }

    /// Farthest vertex from the body origin (metres, body space).
    pub fn radius(&self) -> f32 {
        self.radius
    }

    /// A small closed mesh (a box or wedge modelled as triangles, every edge shared by two of
    /// them): it has an inside, so its faces can point away from the middle like a convex shape.
    /// A pillar lift was such a box modelled facing inward, and Mario walked in from the side.
    pub fn small_closed(&self) -> bool {
        if self.tris.len() > 24 {
            return false;
        }
        let key = |v: Vec3| (v * 100.0).round().to_array().map(|x| x as i64);
        let mut edges: HashMap<([i64; 3], [i64; 3]), u32> = HashMap::new();
        for t in &self.tris {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (a, b) = (key(a), key(b));
                *edges.entry(if a < b { (a, b) } else { (b, a) }).or_default() += 1;
            }
        }
        !edges.is_empty() && edges.values().all(|&n| n == 2)
    }

    fn new(tris: Vec<Tri>) -> Self {
        let mut cells: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        let mut radius: f32 = 0.0;
        for (i, t) in tris.iter().enumerate() {
            if !t.iter().all(|v| v.is_finite() && v.abs().max_element() < 20_000.0) {
                continue;
            }
            let lo = t[0].min(t[1]).min(t[2]);
            let hi = t[0].max(t[1]).max(t[2]);
            radius = radius.max(lo.length()).max(hi.length());
            let c = |v: f32| (v / CELL).floor() as i32;
            let span = (c(hi.x) - c(lo.x) + 1) as i64 * (c(hi.y) - c(lo.y) + 1) as i64 * (c(hi.z) - c(lo.z) + 1) as i64;
            if span > 50_000 {
                continue; // absurdly large triangle: skip rather than flood the grid
            }
            for x in c(lo.x)..=c(hi.x) {
                for y in c(lo.y)..=c(hi.y) {
                    for z in c(lo.z)..=c(hi.z) {
                        cells.entry((x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        Self { tris, cells, radius }
    }
}

/// Hashing for maps keyed by an address or a body index: one multiplication. The standard
/// hasher is made to withstand hostile keys, and the query looks tens of thousands of bodies
/// up in these maps several times a second.
#[derive(Default)]
pub struct Plain(u64);

impl std::hash::Hasher for Plain {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ *b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_usize(&mut self, n: usize) {
        self.0 = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(26);
    }
    fn write_u32(&mut self, n: u32) {
        self.write_usize(n as usize);
    }
}

type PlainMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<Plain>>;
pub type PlainSet<K> = std::collections::HashSet<K, std::hash::BuildHasherDefault<Plain>>;

#[derive(Default)]
pub struct HavokCollision {
    /// shape address -> (mesh data address, primitive count, decoded mesh)
    meshes: PlainMap<usize, (usize, u32, Option<Arc<Mesh>>)>,
    /// convex shapes (boxes, hulls, cylinders): shape address -> (vtable, decoded hull)
    convex: PlainMap<usize, (usize, Option<Arc<Mesh>>)>,
    pub layers: Vec<u32>,
    /// bodies handled elsewhere (moving platforms become SM64 surface objects)
    pub exclude: PlainSet<u32>,
    /// bodies that contributed triangles to the last query
    pub last_bodies: std::collections::HashSet<u32>,
    bodies: usize,
    body_count: usize,
    logged_layers: bool,
    /// shapes collided as a box (custom-piece meshes): the game's rays pass their gaps, so they
    /// can't confirm them
    pub boxed: PlainSet<usize>,
    /// bodies skipped for not being in the physics world (diagnostics)
    pub not_in_world: u32,
    queries: u32,
    /// What the last look at each body slot found (see `Slot`), by body index.
    slots: Vec<Slot>,
    /// Meshes too far off to be worth decoding yet: shape address -> (vtable, mesh data, how
    /// far it can reach).
    waiting: PlainMap<usize, (usize, usize, f32)>,
}

/// A body slot as last looked at: its shape and place, and how far its mesh reaches from
/// there (negative: no mesh to collide with). While both are unchanged, a body further away
/// than that is passed over on the body table alone, without touching its shape. That's nearly
/// all of them: the table has tens of thousands of bodies for the hundred or so near Mario,
/// and looking each one's shape up every time cost 5 to 12 ms a query on an ordinary CPU.
#[derive(Clone, Copy)]
struct Slot {
    shape: usize,
    at: Vec3,
    reach: f32,
}

/// How far ahead of needing it a mesh is decoded (m).
const AHEAD: f32 = 20.0;

/// How far a compressed mesh can reach from its body's origin, from the box its data keeps its
/// vertices in (md +0x30 min, +0x40 max), without decoding it.
fn domain_bound(md: usize) -> Option<f32> {
    if md == 0 || !readable(md, 0xb0) {
        return None;
    }
    let (lo, hi) = (vec3_at(md + 0x30), vec3_at(md + 0x40));
    let far = lo.abs().max(hi.abs()).length();
    (far.is_finite() && far < 100_000.0).then_some(far)
}

/// One body in this many gets the full look every query whatever its slot says, so a slot
/// that went stale (its shape's memory reused for another, in the same place) is put right.
const RECHECK: usize = 16;


fn u32_at(a: usize) -> u32 {
    unsafe { *(a as *const u32) }
}

fn f32_at(a: usize) -> f32 {
    unsafe { *(a as *const f32) }
}

fn vec3_at(a: usize) -> Vec3 {
    Vec3::new(f32_at(a), f32_at(a + 4), f32_at(a + 8))
}

fn u16_at(a: usize) -> u16 {
    unsafe { *(a as *const u16) }
}

/// A body's rotation in the world: the rotation part of its transform (three columns at +0x00,
/// +0x10, +0x20; the position we use is the fourth at +0x30). None if it isn't a clean rotation.
/// The quaternion at +0x80 is the body's rotation relative to its motion: for static bodies that's
/// (the inverse of) the world one, but a moving lift's is only its turn within the asset, which had
/// its collision built turned and Mario flung.
fn body_rotation(body: usize) -> Option<Quat> {
    let (x, y, z) = (vec3_at(body), vec3_at(body + 0x10), vec3_at(body + 0x20));
    let unit = |v: Vec3| (v.length() - 1.0).abs() < 0.02;
    if !(unit(x) && unit(y) && unit(z)) || x.dot(y).abs() > 0.02 || x.dot(z).abs() > 0.02 || y.dot(z).abs() > 0.02 {
        return None;
    }
    let q = Quat::from_mat3(&glam::Mat3::from_cols(x, y, z));
    q.is_finite().then(|| q.normalize())
}

/// Decodes a hknp convex shape (hknpBoxShape / hknpConvexPolytopeShape / hknpCylinderShape) into
/// shape-local triangles. Layout: hkRelArray {u16 count, u16 offset from the field} for vertices at
/// +0x3a (vec4, index in w), faces at +0x44 ({u16 first index, u8 count, u8}), u8 indices at +0x48.
fn decode_convex(shape: usize) -> Option<Vec<Tri>> {
    if !readable(shape, 0x50) {
        return None;
    }
    let rel = |field: usize| (shape + field + u16_at(shape + field + 2) as usize, u16_at(shape + field) as usize);
    let (verts, nv) = rel(0x3a);
    let (faces, nf) = rel(0x44);
    let (idx, ni) = rel(0x48);
    if nv == 0 || nv > 1024 || nf == 0 || nf > 1024 || ni > 8192 {
        return None;
    }
    if !readable(verts, nv * 16) || !readable(faces, nf * 4) || !readable(idx, ni) {
        return None;
    }
    let v: Vec<Vec3> = (0..nv).map(|i| vec3_at(verts + i * 16)).collect();
    if !v.iter().all(|p| p.is_finite() && p.abs().max_element() < 1000.0) {
        return None;
    }
    let mut tris = Vec::new();
    for f in 0..nf {
        let first = u16_at(faces + f * 4) as usize;
        let count = unsafe { *((faces + f * 4 + 2) as *const u8) } as usize;
        if count < 3 || first + count > ni {
            continue;
        }
        let at = |k: usize| unsafe { *((idx + first + k) as *const u8) } as usize;
        for k in 1..count - 1 {
            let (a, b, c) = (at(0), at(k), at(k + 1));
            if a < nv && b < nv && c < nv {
                tris.push([v[a], v[b], v[c]]);
            }
        }
    }
    Some(tris)
}

/// A compressed mesh made only of custom (convex piece) primitives, like the portcullis gates:
/// its pieces aren't plain triangles, so it becomes the box of its domain, if that box is thin
/// (gates, grilles, fences). Shape-local triangles; None for anything else.
fn custom_pieces_box(md: usize) -> Option<Vec<Tri>> {
    if !readable(md, 0xb0) {
        return None;
    }
    let prims = read_u64(md + 0x70)? as usize;
    let n = u32_at(md + 0x78) as usize;
    if n == 0 || n > 100_000 || !readable(prims, n * 4) {
        return None;
    }
    // (a primitive whose first two indices match is a custom one)
    let all_custom = (0..n).all(|k| unsafe { *((prims + k * 4) as *const u8) == *((prims + k * 4 + 1) as *const u8) });
    if !all_custom {
        return None;
    }
    let lo = vec3_at(md + 0x30);
    let hi = vec3_at(md + 0x40);
    let size = hi - lo;
    if !(size.min_element() > 0.0 && size.min_element() < 1.0 && size.max_element() < 60.0) {
        return None;
    }
    let c = |i: u32| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
    // the 6 faces as 12 triangles (facing is decided later, per triangle)
    let faces = [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]];
    Some(faces.iter().flat_map(|f| [[c(f[0]), c(f[1]), c(f[2])], [c(f[0]), c(f[2]), c(f[3])]]).collect())
}

/// A compressed mesh's custom pieces: shapes of their own, placed in the mesh's space. The shape
/// lists them at +0x70 (count at +0x78), 0x70 bytes each, laid out like a compound's instances:
/// rotation columns at +0x00/+0x10/+0x20, translation +0x30, scale +0x40, child shape +0x50. Lift
/// platforms and a drawbridge are made of nothing else, and decoded to no triangles at all.
fn decode_custom_pieces(shape: usize) -> Option<Vec<Tri>> {
    if !readable(shape, 0x80) {
        return None;
    }
    let list = read_u64(shape + 0x70)? as usize;
    let n = u32_at(shape + 0x78) as usize;
    if n == 0 || n > 4096 || !readable(list, n * 0x70) {
        return None;
    }
    let mut out = Vec::new();
    for k in 0..n {
        let a = list + k * 0x70;
        let (c0, c1, c2) = (vec3_at(a), vec3_at(a + 0x10), vec3_at(a + 0x20));
        let t = vec3_at(a + 0x30);
        let scale = vec3_at(a + 0x40);
        if ![c0, c1, c2, t, scale].iter().all(|v| v.is_finite()) || c0.length() > 100.0 || t.length() > 10_000.0 {
            return None;
        }
        let child = read_u64(a + 0x50)? as usize;
        if child < 0x10000 || !readable(child, 0x50) {
            continue;
        }
        let class = class_of(child).unwrap_or_default();
        let tris = if class.contains("ConvexPolytopeShape") || class.contains("BoxShape") || class.contains("CylinderShape") {
            decode_convex(child)
        } else if class.contains("CompressedMeshShape") {
            read_u64(child + 0x48).and_then(|md| decode(md as usize))
        } else {
            None
        };
        for tri in tris.unwrap_or_default() {
            out.push(tri.map(|v| {
                let v = v * scale;
                c0 * v.x + c1 * v.y + c2 * v.z + t
            }));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Decodes a hknpCompressedMeshShapeData into shape-local triangles. None if the layout looks wrong.
fn decode(md: usize) -> Option<Vec<Tri>> {
    if !readable(md, 0xb0) {
        return None;
    }
    let arr = |off: usize, elem: usize, max: usize| -> Option<(usize, usize)> {
        let p = read_u64(md + off)? as usize;
        let n = u32_at(md + off + 8) as usize;
        (n <= max && (n == 0 || readable(p, n * elem))).then_some((p, n))
    };
    let (secs, nsec) = arr(0x60, 0x60, 20_000)?;
    let (prims, nprims) = arr(0x70, 4, 4_000_000)?;
    let (sidx, nsidx) = arr(0x80, 2, 4_000_000)?;
    let (packed, npacked) = arr(0x90, 4, 4_000_000)?;
    let (shared, nshared) = arr(0xa0, 8, 4_000_000)?;
    let dmin = vec3_at(md + 0x30);
    let dmax = vec3_at(md + 0x40);
    let span = dmax - dmin;
    let mut tris = Vec::new();
    for s in 0..nsec {
        let a = secs + s * 0x60;
        let off = vec3_at(a + 0x30);
        let scale = vec3_at(a + 0x3c);
        let first_packed = u32_at(a + 0x48) as usize;
        let shared_start = u32_at(a + 0x4c) as usize;
        let prim_start = u32_at(a + 0x50) as usize;
        let counts = u32_at(a + 0x58);
        let (num_packed, num_prims) = ((counts & 0xff) as usize, ((counts >> 8) & 0xff) as usize);
        let vert = |i: usize| -> Option<Vec3> {
            if i < num_packed {
                let k = first_packed + i;
                if k >= npacked {
                    return None;
                }
                let v = u32_at(packed + k * 4);
                Some(off + scale * Vec3::new((v & 0x7ff) as f32, ((v >> 11) & 0x7ff) as f32, (v >> 22) as f32))
            } else {
                let k = shared_start + i - num_packed;
                if k >= nsidx {
                    return None;
                }
                let si = unsafe { *((sidx + k * 2) as *const u16) } as usize;
                if si >= nshared {
                    return None;
                }
                let v = unsafe { *((shared + si * 8) as *const u64) };
                let f = Vec3::new(
                    (v & 0x1f_ffff) as f32 / 0x1f_ffff as f32,
                    ((v >> 21) & 0x1f_ffff) as f32 / 0x1f_ffff as f32,
                    (v >> 42) as f32 / 0x3f_ffff as f32,
                );
                Some(dmin + span * f)
            }
        };
        for p in prim_start..(prim_start + num_prims).min(nprims) {
            let idx = u32_at(prims + p * 4).to_le_bytes().map(|b| b as usize);
            let (Some(a), Some(b), Some(c)) = (vert(idx[0]), vert(idx[1]), vert(idx[2])) else { continue };
            tris.push([a, b, c]);
            if idx[3] != idx[2] {
                if let Some(d) = vert(idx[3]) {
                    tris.push([a, c, d]);
                }
            }
        }
    }
    Some(tris)
}

/// hknpCompoundShape: +0x48 instances (0x80 each), +0x50 count. Instance: +0x00/+0x10/+0x20
/// rotation columns, +0x30 translation, +0x40 scale, +0x50 child shape.
fn decode_compound(shape: usize) -> Option<Vec<Tri>> {
    let inst = read_u64(shape + 0x48)? as usize;
    let n = (u32_at(shape + 0x50) as usize).min(1024);
    if n == 0 || !readable(inst, n * 0x80) {
        return None;
    }
    let mut out = Vec::new();
    for k in 0..n {
        let a = inst + k * 0x80;
        let (c0, c1, c2) = (vec3_at(a), vec3_at(a + 0x10), vec3_at(a + 0x20));
        let t = vec3_at(a + 0x30);
        let scale = vec3_at(a + 0x40);
        let child = read_u64(a + 0x50)? as usize;
        if !class_of(child).is_some_and(|c| c.contains("CompressedMeshShape")) {
            continue;
        }
        let Some(tris) = read_u64(child + 0x48).and_then(|md| decode(md as usize)) else { continue };
        for tri in tris {
            out.push(tri.map(|v| {
                let v = v * scale;
                c0 * v.x + c1 * v.y + c2 * v.z + t
            }));
        }
    }
    Some(out)
}

/// Dynamic physics props (layer 0x1e: barrels, crates, debris) within `range` of `center`: (body index, position).
/// Debug: every body whose origin is within `range` m (index, layer, shape class).
fn bodies_near(center: Vec3, range: f32) -> Vec<String> {
    let mut out = Vec::new();
    let Some(havok) = unsafe { CSHavokMan::instance() }.ok() else { return out };
    let base = havok as *const CSHavokMan as usize;
    let Some(world) = read_u64(base + 0x98).and_then(|pw| read_u64(pw as usize + 0x8)) else { return out };
    let world = world as usize;
    let Some(bodies) = read_u64(world + 0x28) else { return out };
    let bodies = bodies as usize;
    let count = (u32_at(world + 0x30) as usize).min(262_144);
    if !readable(bodies, count * 0xb0) {
        return out;
    }
    for i in 0..count {
        let body = bodies + i * 0xb0;
        let pos = vec3_at(body + 0x30);
        if (pos - center).length() > range {
            continue;
        }
        let shape = unsafe { *((body + 0x60) as *const usize) };
        out.push(format!(
            "  near body #{i} layer {:#x} shape {:?} at {:.2?} ({:.1} m)",
            u32_at(body + 0x6c),
            class_of(shape),
            pos,
            (pos - center).length()
        ));
    }
    out
}

/// Debug: the raw words (+0x40..+0xb0) of every body on `layer` (low byte) whose AABB is within
/// `range` m of `center`, to compare a live body with one the game switched off.
pub fn dump_layer_near(center: Vec3, range: f32, layer: u32) -> Vec<String> {
    let mut out = Vec::new();
    let Some(havok) = unsafe { CSHavokMan::instance() }.ok() else { return out };
    let base = havok as *const CSHavokMan as usize;
    let Some(world) = read_u64(base + 0x98).and_then(|pw| read_u64(pw as usize + 0x8)) else { return out };
    let world = world as usize;
    let Some(bodies) = read_u64(world + 0x28) else { return out };
    let bodies = bodies as usize;
    let count = (u32_at(world + 0x30) as usize).min(262_144);
    if !readable(bodies, count * 0xb0) {
        return out;
    }
    for i in 0..count {
        let body = bodies + i * 0xb0;
        if u32_at(body + 0x6c) & 0xff != layer || unsafe { *((body + 0x60) as *const usize) } == 0 {
            continue;
        }
        let p = vec3_at(body + 0x30);
        if (p - center).length() > range {
            continue;
        }
        let words: Vec<String> = (0x40..0xb0).step_by(4).map(|o| format!("{:08x}", u32_at(body + o))).collect();
        out.push(format!("  raw body #{i} L{layer:x} at {p:.2?}: {}", words.join(" ")));
    }
    out
}

/// A lift pillar sits in a shaft a little wider than its collision box: next to it the floor ends
/// ~0.4 m before the pillar's side. The Tarnished's round capsule rests on that edge, but SM64
/// checks the floor at Mario's centre, which stopped just past it, and he fell in. A big closed box
/// on a lift (flags 0x02/0x03, never a building's 0x04) is grown sideways by this much, and more
/// along its short axis (the long octagon's ends reach further past the box than its long faces);
/// its sides are no longer where the game's rays would confirm them, so it's trusted like a box
/// (`boxed`).
const PILLAR_GROW: f32 = 0.6;
const PILLAR_GROW_ENDS: f32 = 1.4;

fn grow_pillar(mesh: &Mesh) -> Option<Mesh> {
    if !mesh.small_closed() || mesh.radius() < 5.0 {
        return None;
    }
    let v = mesh.tris().iter().flatten();
    let lo = v.clone().fold(Vec3::splat(f32::MAX), |a, b| a.min(*b));
    let hi = v.fold(Vec3::splat(f32::MIN), |a, b| a.max(*b));
    // a pillar stands taller than it is wide (a lowered bridge is a lift too, but flat)
    if hi.y - lo.y < (hi.x - lo.x).max(hi.z - lo.z) {
        return None;
    }
    let mid = (lo + hi) / 2.0;
    let short = if hi.x - lo.x < hi.z - lo.z { 0 } else { 2 };
    let tris = mesh.tris().iter().map(|t| t.map(|p| {
        let mut p = p;
        for axis in [0, 2] {
            let grow = if axis == short { PILLAR_GROW_ENDS } else { PILLAR_GROW };
            p[axis] += if p[axis] > mid[axis] { grow } else { -grow };
        }
        p
    })).collect();
    Some(Mesh::new(tris))
}

/// Breakable things near `center`: (body index, origin, layer). Physics props (layer 0x1e: loose
/// barrels, pots) have their origin at the centre of mass; map assets (0x3a: crates and barrels
/// in dungeons, also lifts and gates, which just shrug a hit off) at their base.
pub fn props_near(center: Vec3, range: f32) -> Vec<(u32, Vec3, u32)> {
    let mut out = Vec::new();
    let Some(havok) = unsafe { CSHavokMan::instance() }.ok() else { return out };
    let base = havok as *const CSHavokMan as usize;
    let Some(world) = read_u64(base + 0x98).and_then(|pw| read_u64(pw as usize + 0x8)) else { return out };
    let world = world as usize;
    let Some(bodies) = read_u64(world + 0x28) else { return out };
    let bodies = bodies as usize;
    let count = (u32_at(world + 0x30) as usize).min(262_144);
    if !readable(bodies, count * 0xb0) {
        return out;
    }
    for i in 0..count {
        let body = bodies + i * 0xb0;
        let layer = u32_at(body + 0x6c) & 0xff;
        let shape = unsafe { *((body + 0x60) as *const usize) };
        if shape == 0 || !matches!(layer, 0x1e | 0x3a | 0x49 | 0x55) || u32_at(body + 0x78) == u32::MAX {
            continue;
        }
        let p = vec3_at(body + 0x30);
        if (p - center).length() >= range {
            continue;
        }
        // 0x49 / 0x55: small clutter (pots, jars, stools, debris) are boxes; anything else there
        // is map collision
        if matches!(layer, 0x49 | 0x55) && class_of(shape).as_deref() != Some("hknpBoxShape") {
            continue;
        }
        // one target for a cluster of pieces (a hit breaks everything around it anyway)
        if out.iter().any(|(_, q, _): &(u32, Vec3, u32)| q.distance(p) < 0.6) {
            continue;
        }
        out.push((i as u32, p, layer));
    }
    out
}

impl HavokCollision {
    /// Debug: every body (any layer) with something at `p` (x/z inside a triangle, y within 0.5 m),
    /// plus undecodable shapes nearby. Returns log lines.
    pub fn probe(&mut self, p: Vec3) -> Vec<String> {
        let mut lines = bodies_near(p, 3.0);
        let saved = std::mem::take(&mut self.layers);
        let tris = self.query(p).unwrap_or_default();
        self.layers = saved;
        let inside = |t: &Tri| {
            let (a, b, c) = (t[0], t[1], t[2]);
            let s = |p1: Vec3, p2: Vec3, p3: Vec3| (p1.x - p3.x) * (p2.z - p3.z) - (p2.x - p3.x) * (p1.z - p3.z);
            let (d1, d2, d3) = (s(p, a, b), s(p, b, c), s(p, c, a));
            let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            if neg && pos {
                return None;
            }
            // height of the triangle plane at p
            let n = (b - a).cross(c - a);
            if n.y.abs() < 1e-6 {
                return None;
            }
            Some(a.y - (n.x * (p.x - a.x) + n.z * (p.z - a.z)) / n.y)
        };
        let mut found: HashMap<(u32, u32), f32> = HashMap::new();
        for (t, layer, body) in &tris {
            if let Some(h) = inside(t) {
                if (h - p.y).abs() < 2.0 {
                    found.insert((*body, *layer), h);
                }
            }
        }
        lines.push(format!("probe at {p:?}: {} triangles nearby (all layers)", tris.len()));
        for ((body, layer), h) in found {
            let allowed = self.layers.contains(&layer);
            lines.push(format!("  body #{body} layer {layer:#x} surface y {h:.2} {}", if allowed { "(used)" } else { "(FILTERED OUT)" }));
        }
        // shapes we can't decode near p
        if let Some(havok) = unsafe { CSHavokMan::instance() }.ok() {
            let base = havok as *const CSHavokMan as usize;
            if let Some(world) = read_u64(base + 0x98).and_then(|pw| read_u64(pw as usize + 0x8)) {
                let world = world as usize;
                let bodies = read_u64(world + 0x28).unwrap_or(0) as usize;
                let count = (u32_at(world + 0x30) as usize).min(262_144);
                for i in 0..count {
                    let body = bodies + i * 0xb0;
                    if !readable(body, 0xb0) {
                        break;
                    }
                    let shape = unsafe { *((body + 0x60) as *const usize) };
                    if shape == 0 || (vec3_at(body + 0x30) - p).length() > 6.0 {
                        continue;
                    }
                    if matches!(self.meshes.get(&shape), Some((_, _, Some(_)))) {
                        continue;
                    }
                    lines.push(format!("  undecoded body #{i} layer {:#x} class {:?}", u32_at(body + 0x6c), class_of(shape)));
                    // a compressed mesh we couldn't read: which pointer in its header is the mesh data?
                    if class_of(shape).is_some_and(|c| c.contains("CompressedMeshShape")) && readable(shape, 0xa0) {
                        for off in (0x08..0xa0).step_by(8) {
                            let md = unsafe { *((shape + off) as *const usize) };
                            if md < 0x10000 || !readable(md, 0xb0) {
                                continue;
                            }
                            let n = decode(md).map(|t| t.len());
                            lines.push(format!("    shape +{off:#x} -> {md:#x}: decode {n:?}"));
                            if off == 0x48 {
                                // the data's arrays (pointer, count, capacity) and the rest as words
                                let mut row = Vec::new();
                                for o in (0x00..0x180).step_by(8) {
                                    if !readable(md + o, 16) {
                                        break;
                                    }
                                    let p = unsafe { *((md + o) as *const u64) };
                                    let n = u32_at(md + o + 8);
                                    let cap = u32_at(md + o + 12);
                                    if p > 0x10000 && p >> 47 == 0 && n > 0 && n < 10_000_000 && (cap & 0x3fff_ffff) >= n {
                                        row.push(format!("+{o:#x}: array[{n}]"));
                                    }
                                }
                                lines.push(format!("      data arrays: {}", row.join(", ")));
                                let words: Vec<String> = (0..0x60).step_by(4).map(|o| format!("{:08x}", u32_at(md + 0x20 + o))).collect();
                                lines.push(format!("      data +0x20..: {}", words.join(" ")));
                                // the shape object itself, words
                                let words: Vec<String> = (0..0xa0).step_by(4).map(|o| format!("{:08x}", u32_at(shape + o))).collect();
                                lines.push(format!("      shape: {}", words.join(" ")));
                            }
                        }
                    }
                }
            }
        }
        lines
    }

    /// Refresh the live body table before per-tick reads: streaming can reallocate it between queries.
    pub fn refresh_bodies(&mut self) -> bool {
        self.bodies = 0;
        self.body_count = 0;
        let Some(havok) = unsafe { CSHavokMan::instance() }.ok() else { return false };
        let base = havok as *const CSHavokMan as usize;
        let Some(world) = read_u64(base + 0x98).and_then(|pw| read_u64(pw as usize + 0x8)) else { return false };
        let world = world as usize;
        if !readable(world, 0x34) { return false; }
        let Some(bodies) = read_u64(world + 0x28) else { return false };
        let count = (u32_at(world + 0x30) as usize).min(262_144);
        if !readable(bodies as usize, count * 0xb0) { return false; }
        self.bodies = bodies as usize;
        self.body_count = count;
        true
    }

    fn body(&self, i: u32) -> Option<usize> {
        if self.bodies == 0 || i as usize >= self.body_count { return None; }
        let body = self.bodies + i as usize * 0xb0;
        if !readable(body, 0xb0) || u32_at(body + 0x78) == u32::MAX { return None; }
        Some(body)
    }

    /// Current active body -> world transform.
    pub fn transform(&self, i: u32) -> Option<(Vec3, Quat)> {
        let body = self.body(i)?;
        let q = body_rotation(body)
            .unwrap_or_else(|| Quat::from_xyzw(f32_at(body + 0x80), f32_at(body + 0x84), f32_at(body + 0x88), f32_at(body + 0x8c)).conjugate());
        let p = vec3_at(body + 0x30);
        let norm = q.length_squared();
        (p.is_finite() && q.is_finite() && norm.is_finite() && norm > 0.5).then(|| (p, q.normalize()))
    }

    /// Shape address changes when the game reuses a slot; inactive bodies have no shape.
    pub fn shape_of(&self, i: u32) -> usize {
        self.body(i).map_or(0, |body| unsafe { *((body + 0x60) as *const usize) })
    }

    /// Whether body `i` is a convex shape (box, hull, cylinder): its faces all point outwards.
    /// Whether this body collides as its box (see `boxed`).
    pub fn is_boxed(&self, i: u32) -> bool {
        self.boxed.contains(&self.shape_of(i))
    }

    pub fn is_convex(&self, i: u32) -> bool {
        self.convex.contains_key(&self.shape_of(i))
    }

    /// The decoded (cached) mesh of body `i`, if any.
    pub fn mesh_of(&self, i: u32) -> Option<Arc<Mesh>> {
        let shape = self.shape_of(i);
        if let Some((_, m)) = self.convex.get(&shape) {
            return m.clone();
        }
        self.meshes.get(&shape).and_then(|(_, _, m)| m.clone())
    }

    pub fn clear_cache(&mut self) {
        self.meshes.clear();
        self.convex.clear();
        self.slots.clear();
        self.waiting.clear();
    }

    pub fn new(layers: Vec<u32>) -> Self {
        Self { layers, ..Default::default() }
    }

    /// Debug: every body within `range` metres (horizontally) of `p`: layer, class, flags, triangle
    /// count, whether the last query used it, and the box it covers in the world.
    pub fn bodies_around(&self, p: Vec3, range: f32) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..self.body_count {
            let body = self.bodies + i * 0xb0;
            let shape = unsafe { *((body + 0x60) as *const usize) };
            if shape == 0 {
                continue;
            }
            let t = vec3_at(body + 0x30);
            if !t.is_finite() || t.distance(p) > 60.0 {
                continue;
            }
            let class = class_of(shape).unwrap_or_default();
            if class.contains("Capsule") {
                continue;
            }
            let mesh = self.meshes.get(&shape).and_then(|m| m.2.clone()).or_else(|| self.convex.get(&shape).and_then(|c| c.1.clone()));
            let aabb = mesh.as_ref().zip(self.transform(i as u32)).map(|(m, (t, q))| {
                let v = m.tris().iter().flatten().map(|v| q * *v + t);
                (v.clone().fold(Vec3::splat(f32::MAX), |a, b| a.min(b)), v.fold(Vec3::splat(f32::MIN), |a, b| a.max(b)))
            });
            // decoded: its box reaches within `range`; not decoded: origin within 40 m
            let near = match aabb {
                Some((lo, hi)) => (p.clamp(lo, hi) - p).length() <= range,
                None => t.distance(p) <= 40.0,
            };
            if !near {
                continue;
            }
            let bounds = aabb.map(|(lo, hi)| format!("x {:.2}..{:.2} y {:.2}..{:.2} z {:.2}..{:.2}", lo.x, hi.x, lo.y, hi.y, lo.z, hi.z));
            let raw = Quat::from_xyzw(f32_at(body + 0x80), f32_at(body + 0x84), f32_at(body + 0x88), f32_at(body + 0x8c)).conjugate();
            let used = self.transform(i as u32).map(|(_, q)| q);
            out.push(format!(
                "  body #{i} L{:#x} {class} flags {:#x} bp {:#x} at {t:.2?} +0x80 rot {raw:.3?} used {used:.3?}: {} tris, used {}, {}",
                u32_at(body + 0x6c), u32_at(body + 0x68), u32_at(body + 0x78),
                mesh.as_ref().map_or(0, |m| m.tris().len()), self.last_bodies.contains(&(i as u32)),
                bounds.unwrap_or_else(|| "not decoded".into())
            ));
            // an undecoded compressed mesh: what's cached for it, and which header pointer decodes
            if mesh.is_none() && class.contains("CompressedMeshShape") && readable(shape, 0xa0) {
                out.push(format!("    cache {:?}", self.meshes.get(&shape).map(|(md, n, m)| (format!("{md:#x}"), *n, m.is_some()))));
                let list = read_u64(shape + 0x70).unwrap_or(0) as usize;
                let n = (u32_at(shape + 0x78) as usize).min(8);
                let kinds: Vec<String> = (0..n).filter(|_| readable(list, n * 0x70)).map(|k| {
                    let piece = unsafe { *((list + k * 0x70 + 0x50) as *const usize) };
                    format!("{:?}", readable(piece, 0x50).then(|| class_of(piece)).flatten())
                }).collect();
                out.push(format!("    pieces +0x70: count {} first kinds {kinds:?}, decoded {:?}", u32_at(shape + 0x78), decode_custom_pieces(shape).map(|t| t.len())));
                // raw words, to find where the custom pieces live (first such body only: it's long)
                static DUMPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                if DUMPED.swap(shape, std::sync::atomic::Ordering::Relaxed) != shape && decode_custom_pieces(shape).is_none() {
                    let words = |a: usize, len: usize| -> String {
                        (0..len).step_by(8).filter(|o| readable(a + o, 8)).map(|o| format!("{:016x}", unsafe { *((a + o) as *const u64) })).collect::<Vec<_>>().join(" ")
                    };
                    out.push(format!("    shape {shape:#x} [0..0xc0]: {}", words(shape, 0xc0)));
                    for off in [0x50usize, 0x70] {
                        let a = read_u64(shape + off).unwrap_or(0) as usize;
                        if a > 0x10000 && readable(a, 0x80) {
                            out.push(format!("    +{off:#x} -> {a:#x} [0..0x80]: {}", words(a, 0x80)));
                        }
                    }
                    let md = unsafe { *((shape + 0x48) as *const usize) };
                    if readable(md, 0x180) {
                        out.push(format!("    data {md:#x} [0..0x180]: {}", words(md, 0x180)));
                        for (name, off, elem) in [("sections", 0x60usize, 0x60usize), ("prims", 0x70, 4), ("shared idx", 0x80, 2), ("packed", 0x90, 4), ("shared", 0xa0, 8), ("+0xb0", 0xb0, 8), ("+0xc0", 0xc0, 8)] {
                            let a = read_u64(md + off).unwrap_or(0) as usize;
                            let n = u32_at(md + off + 8) as usize;
                            if a > 0x10000 && n > 0 && n < 4096 && readable(a, (n * elem).min(0x100)) {
                                out.push(format!("    {name} [{n}] at {a:#x}: {}", words(a, (n * elem).min(0x100).max(8))));
                            }
                        }
                    }
                }
                let md = unsafe { *((shape + 0x48) as *const usize) };
                if readable(md, 0xb0) {
                    let n = u32_at(md + 0x78) as usize;
                    let prims = read_u64(md + 0x70).unwrap_or(0) as usize;
                    let custom = (prims != 0 && readable(prims, n * 4)).then(|| (0..n).filter(|k| unsafe { *((prims + k * 4) as *const u8) == *((prims + k * 4 + 1) as *const u8) }).count());
                    out.push(format!(
                        "    data: {n} prims ({custom:?} custom), {} sections, box {:.2?}..{:.2?}",
                        u32_at(md + 0x68), vec3_at(md + 0x30), vec3_at(md + 0x40)
                    ));
                }
                for off in (0x08..0xa0).step_by(8) {
                    let md = unsafe { *((shape + off) as *const usize) };
                    if md < 0x10000 || !readable(md, 0xb0) {
                        continue;
                    }
                    out.push(format!(
                        "    shape +{off:#x} -> {md:#x}: decode {:?}, pieces box {:?}",
                        decode(md).map(|t| t.len()), custom_pieces_box(md).map(|t| t.len())
                    ));
                }
            }
        }
        out
    }


    /// World-space (Havok) triangles near `center` from all allowed bodies.
    pub fn query(&mut self, center: Vec3) -> Option<Vec<(Tri, u32, u32)>> {
        let out = self.scan(center, true)?;
        // debug: now and then the same query the long way, every body looked at in full, to
        // show the slots change nothing
        if crate::debug() && self.queries % 20 == 3 {
            let bodies = std::mem::take(&mut self.last_bodies);
            let t = std::time::Instant::now();
            let full = self.scan(center, false)?;
            let key = |v: &[(Tri, u32, u32)]| {
                let mut k: Vec<(u32, [u32; 9])> = v.iter().map(|(t, _, i)| (*i, [t[0].x, t[0].y, t[0].z, t[1].x, t[1].y, t[1].z, t[2].x, t[2].y, t[2].z].map(f32::to_bits))).collect();
                k.sort_unstable();
                k
            };
            let same = key(&out) == key(&full);
            crate::dlog(format!(
                "havok query check: {} ({} triangles by slots, {} the long way in {:.1} ms)",
                if same { "same" } else { "DIFFERENT" },
                out.len(),
                full.len(),
                t.elapsed().as_secs_f32() * 1000.0
            ));
            self.last_bodies = bodies;
        }
        Some(out)
    }

    /// `query`'s work. `by_slots`: pass far bodies over by their slot (false: look at all).
    fn scan(&mut self, center: Vec3, by_slots: bool) -> Option<Vec<(Tri, u32, u32)>> {
        let havok = unsafe { CSHavokMan::instance() }.ok()?;
        let base = havok as *const CSHavokMan as usize;
        let pw = read_u64(base + 0x98)? as usize;
        let world = read_u64(pw + 0x8)? as usize;
        let bodies = read_u64(world + 0x28)? as usize;
        let count = (u32_at(world + 0x30) as usize).min(262_144);
        if !readable(bodies, count * 0xb0) {
            return None;
        }
        self.bodies = bodies;
        self.body_count = count;
        let t_start = std::time::Instant::now();
        let (mut n_layer_ok, mut n_decoded, mut n_near, mut n_picked, mut n_passed, mut n_waiting) = (0u32, 0u32, 0u32, 0usize, 0u32, 0u32);
        if self.slots.len() != count {
            self.slots = vec![Slot { shape: 0, at: Vec3::ZERO, reach: 0.0 }; count];
        }
        let phase = self.queries as usize % RECHECK;
        let mut out = Vec::new();
        let mut seen_layers: HashMap<u32, u32> = HashMap::new();
        // (this loop goes over every body in the world, tens of thousands, for the hundred or so
        // near Mario: the cheap tests come first, and a body's rotation is only worked out
        // once it's known to be near)
        let count_layers = !self.logged_layers;
        let rotation = |body: usize| {
            body_rotation(body)
                .unwrap_or_else(|| Quat::from_xyzw(f32_at(body + 0x80), f32_at(body + 0x84), f32_at(body + 0x88), f32_at(body + 0x8c)).conjugate())
        };
        for i in 0..count {
            let body = bodies + i * 0xb0;
            let shape = unsafe { *((body + 0x60) as *const usize) };
            if shape == 0 {
                continue;
            }
            let layer = u32_at(body + 0x6c);
            // skip layers we don't want before touching the shape
            if !self.layers.is_empty() && !self.layers.contains(&(layer & 0xff)) {
                if count_layers {
                    *seen_layers.entry(layer).or_default() += 1;
                }
                continue;
            }
            // taken out of the physics world (+0x78 broadphase id -1): an opened door's blocker,
            // a broken crate... the body stays in the list but nothing collides with it
            if u32_at(body + 0x78) == u32::MAX {
                self.not_in_world += 1;
                continue;
            }
            let t = vec3_at(body + 0x30);
            n_layer_ok += 1;
            let slot = self.slots[i];
            if by_slots && i % RECHECK != phase && slot.shape == shape && slot.at == t && (slot.reach < 0.0 || (t - center).length() > slot.reach + RADIUS + BELOW) {
                n_passed += 1;
                continue;
            }
            // (after that: a set lookup per body, for one or two bodies in it)
            if self.exclude.contains(&(i as u32)) {
                continue;
            }
            let none = Slot { shape, at: t, reach: -1.0 };
            // shapes get freed and re-allocated as the world streams: validate the cache entry
            // unknown shapes (and stale pointers in unused body slots) get one real memory check,
            // after which they're cached; known shapes are read directly
            if matches!(self.meshes.get(&shape), Some((0, 0, None))) {
                self.slots[i] = none;
                continue;
            }
            if !self.meshes.contains_key(&shape) && !self.convex.contains_key(&shape) && !readable(shape, 0x50) {
                self.meshes.insert(shape, (0, 0, None));
                self.slots[i] = none;
                continue;
            }
            let vtable = unsafe { *(shape as *const usize) };
            let convex = match self.convex.get(&shape) {
                Some((vt, m)) if *vt == vtable => Some(m.clone()),
                Some(_) => {
                    self.convex.remove(&shape);
                    None
                }
                None => None,
            };
            let md = unsafe { *((shape + 0x48) as *const usize) };
            // (no memory check here: it is validated on decode)
            let nprims = if convex.is_some() { 0 } else { match self.meshes.get(&shape) {
                // non-mesh shapes (boxes etc.) are cached as None: nothing to read, skip them
                Some((cmd, _, None)) if *cmd == md => {
                    self.slots[i] = none;
                    continue;
                }
                Some((cmd, _, Some(_))) if *cmd == md && md != 0 => u32_at(md + 0x78),
                _ => 0,
            } };
            let mesh = if let Some(m) = convex { m } else { match self.meshes.get(&shape) {
                Some((cmd, cn, m)) if *cmd == md && *cn == nprims && md != 0 => m.clone(),
                _ => {
                    // (one already waiting isn't asked again what it is or how big: its shape's
                    // class and its data are as they were)
                    let waited = self.waiting.get(&shape).filter(|w| (w.0, w.1) == (vtable, md)).map(|w| w.2);
                    let cls = if waited.is_some() { String::new() } else { class_of(shape).unwrap_or_default() };
                    // A mesh nowhere near waits. Every new shape used to be decoded on sight,
                    // wherever it was, to learn how far it reaches: a map tile streaming in half
                    // a kilometre off meant a couple of hundred meshes in one query, 40 to 70 ms.
                    // Its data has the box all its vertices lie in, which says as much for free.
                    // It's decoded once Mario is within AHEAD of where it could matter, so they
                    // come in one at a time as he gets near them.
                    if waited.is_some() || cls.contains("CompressedMeshShape") {
                        if let Some(bound) = waited.or_else(|| domain_bound(md)).filter(|b| (t - center).length() > b + RADIUS + BELOW + AHEAD) {
                            self.waiting.insert(shape, (vtable, md, bound));
                            self.slots[i] = Slot { shape, at: t, reach: bound + AHEAD };
                            n_waiting += 1;
                            continue;
                        }
                    }
                    let cls = if self.waiting.remove(&shape).is_some() { class_of(shape).unwrap_or_default() } else { cls };
                    n_decoded += 1;
                    if self.queries == 0 {
                        crate::dlog(format!("  decoding body {i} layer {layer:#x} shape {shape:#x} md {md:#x} class {cls:?}"));
                    }
                    if cls.contains("ConvexPolytopeShape") || cls.contains("BoxShape") || cls.contains("CylinderShape") {
                        let m = decode_convex(shape).filter(|t| !t.is_empty()).map(|t| Arc::new(Mesh::new(t)));
                        self.meshes.remove(&shape);
                        self.convex.insert(shape, (vtable, m.clone()));
                        m
                    } else {
                    let tris = if cls.contains("CompressedMeshShape") {
                        decode(md).filter(|t| !t.is_empty()).or_else(|| {
                            let pieces = decode_custom_pieces(shape);
                            if let Some(p) = &pieces {
                                crate::dlog(format!("collision: body {i} (layer {layer:#x}) is custom pieces: {} triangles from its convex pieces", p.len()));
                            }
                            pieces
                        }).or_else(|| {
                            let b = custom_pieces_box(md);
                            if b.is_some() {
                                self.boxed.insert(shape);
                                log(format!("collision: body {i} (layer {layer:#x}) is custom pieces only: using its box"));
                                if let Some(b) = b.as_ref() {
                                    let v = b.iter().flatten();
                                    let lo = v.clone().fold(Vec3::splat(f32::MAX), |a, c| a.min(*c));
                                    let hi = v.fold(Vec3::splat(f32::MIN), |a, c| a.max(*c));
                                    crate::dlog(format!(
                                        "  body {i} at {t:.2?} rot {:.3?} flags {:#x} bp {:#x}: local box {lo:.2?}..{hi:.2?} (centre {:.2?})",
                                        rotation(body), u32_at(body + 0x68), u32_at(body + 0x78), (lo + hi) / 2.0
                                    ));
                                }
                            }
                            b
                        })
                    } else if cls.contains("CompoundShape") {
                        decode_compound(shape)
                    } else {
                        None
                    };
                    let mut m = tris.filter(|t| !t.is_empty()).map(|t| Arc::new(Mesh::new(t)));
                    // a lift pillar: grown out so Mario can't stand over the gap around it
                    let lift = matches!(u32_at(body + 0x68) & 0xff, 0x02 | 0x03);
                    if let Some(grown) = m.as_ref().filter(|_| lift).and_then(|m| grow_pillar(m)) {
                        crate::dlog(format!("collision: body {i} (layer {layer:#x}) is a lift pillar, grown by {PILLAR_GROW} m"));
                        self.boxed.insert(shape);
                        m = Some(Arc::new(grown));
                    }
                    let n = if md != 0 && readable(md + 0x70, 16) { u32_at(md + 0x78) } else { 0 };
                    self.meshes.insert(shape, (md, n, m.clone()));
                    m
                    }
                }
            } };
            let Some(mesh) = mesh else {
                self.slots[i] = none;
                continue;
            };
            self.slots[i] = Slot { shape, at: t, reach: mesh.radius };
            if (t - center).length() > mesh.radius + RADIUS + BELOW {
                continue;
            }
            let q = rotation(body);
            if !q.is_finite() || q.length_squared() < 0.5 {
                continue;
            }
            let q = q.normalize();
            if count_layers {
                *seen_layers.entry(layer).or_default() += 1;
            }
            // query boxes (world AABBs): the main box around Mario and a narrow deep column under him
            let boxes = [
                (center - Vec3::new(RADIUS, BOX_DOWN, RADIUS), center + Vec3::new(RADIUS, ABOVE, RADIUS)),
                (center - Vec3::new(COLUMN, BELOW, COLUMN), center + Vec3::new(COLUMN, 0.0, COLUMN)),
            ];
            let inv = q.inverse();
            let c = |v: f32| (v / CELL).floor() as i32;
            let mut picked = std::collections::HashSet::new();
            for (wlo, whi) in boxes {
                let (mut llo, mut lhi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for k in 0..8 {
                    let p = Vec3::new(
                        if k & 1 == 0 { wlo.x } else { whi.x },
                        if k & 2 == 0 { wlo.y } else { whi.y },
                        if k & 4 == 0 { wlo.z } else { whi.z },
                    );
                    let l = inv * (p - t);
                    llo = llo.min(l);
                    lhi = lhi.max(l);
                }
                for x in c(llo.x)..=c(lhi.x) {
                    for y in c(llo.y)..=c(lhi.y) {
                        for z in c(llo.z)..=c(lhi.z) {
                            if let Some(list) = mesh.cells.get(&(x, y, z)) {
                                picked.extend(list.iter().copied());
                            }
                        }
                    }
                }
            }
            n_near += 1;
            n_picked += picked.len();
            for ti in picked {
                let w = mesh.tris[ti as usize].map(|v| q * v + t);
                let lo = w[0].min(w[1]).min(w[2]);
                let hi = w[0].max(w[1]).max(w[2]);
                let hits = |r: f32, down: f32, up: f32| {
                    lo.x < center.x + r && hi.x > center.x - r && lo.z < center.z + r && hi.z > center.z - r
                        && hi.y > center.y - down && lo.y < center.y + up
                };
                if hits(RADIUS, BOX_DOWN, ABOVE) || hits(COLUMN, BELOW, 0.0) {
                    out.push((w, layer & 0xff, i as u32));
                }
            }
        }
        self.last_bodies = out.iter().map(|t| t.2).collect();
        self.queries += 1;
        if self.queries % 10 == 1 {
            crate::dlog(format!(
                "havok query: {:.1} ms, bodies {count}, layer ok {n_layer_ok}, passed by slot {n_passed}, waiting {n_waiting}, decoded {n_decoded}, near {n_near}, picked {n_picked}, out {}",
                t_start.elapsed().as_secs_f32() * 1000.0, out.len()
            ));
        }
        if !self.logged_layers {
            self.logged_layers = true;
            log(format!("havok collision: layers near Mario (layer -> bodies): {seen_layers:x?}"));
        }
        if out.len() > MAX_TRIS {
            out.sort_by(|a, b| {
                // distance to the triangle's box, not its centre: a big floor tri under Mario
                // has its centre metres away and got cut first
                let d = |t: &(Tri, u32, u32)| {
                    let lo = t.0[0].min(t.0[1]).min(t.0[2]);
                    let hi = t.0[0].max(t.0[1]).max(t.0[2]);
                    (center.clamp(lo, hi) - center).length_squared()
                };
                d(a).total_cmp(&d(b))
            });
            out.truncate(MAX_TRIS);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_body_reads_reject_removed_and_reused_slots() {
        // Synthetic live body allocation, laid out just like the reader's 0xb0-byte record.
        let mut words = vec![0u64; 0xb0 / 8];
        let base = words.as_mut_ptr() as usize;
        unsafe {
            *((base + 0x60) as *mut usize) = 0x12340;
            *((base + 0x8c) as *mut f32) = 1.0;
        }
        let h = HavokCollision { bodies: base, body_count: 1, ..Default::default() };
        assert_eq!(h.shape_of(0), 0x12340);
        assert!(h.transform(0).is_some());
        assert_eq!(h.shape_of(1), 0);
        assert!(h.transform(1).is_none());
        unsafe { *((base + 0x78) as *mut u32) = u32::MAX; }
        assert_eq!(h.shape_of(0), 0);
        assert!(h.transform(0).is_none());
        unsafe {
            *((base + 0x78) as *mut u32) = 0;
            *((base + 0x60) as *mut usize) = 0x56780;
        }
        assert_ne!(h.shape_of(0), 0x12340);
        unsafe { *((base + 0x30) as *mut f32) = f32::NAN; }
        assert!(h.transform(0).is_none());
        unsafe {
            *((base + 0x30) as *mut f32) = 0.0;
            *((base + 0x8c) as *mut f32) = 0.0;
        }
        assert!(h.transform(0).is_none()); // never normalize an invalid/zero quaternion
    }
}
