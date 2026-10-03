//! In-place FLVER2 editing: Mario's mesh goes into an armour FLVER's existing vertex and index
//! buffers (the game drops meshes whose data was appended or re-serialised), each SM64 body part
//! bound 100% to its own bone with an identity bind, so the mod can write the part matrices
//! straight into those bones.

use std::collections::HashMap;

use super::model::{MarioModel, Tri};

/// SM64 part -> FLVER bone (bones the chest piece skins natively); same list as engine_mario.rs.
pub const PART_BONES: [&str; 22] = [
    "", "Pelvis_Mantle", "Spine2", "Neck", "L_ShoulderArmor", "L_Pectoral", "Collar", "R_Shoulder", "R_Pectoral",
    "Spine2_Mantle", "L_Hip", "SpineArmor1", "Spine_Mantle", "R_Hip", "SpineArmor2", "L_Shoulder",
    // eye variants (open, half, closed, dead)
    "L_UpArmTwist", "L_Elbow", "L_ForeArmTwist", "L_ForeArmTwist1",
    // the peace-sign right hand (star dance)
    "R_Elbow",
    // Independently hidden skateboard on a verified unused leaf skin bone.
    "R_Calf",
];
/// SM64 units -> metres, times Mario's 0.25 model scale
const UNIT: f64 = 0.01 * 0.25;

pub const TEX: f64 = 2048.0;
/// SM64's atlas: eleven 64x64 textures side by side
pub const CELLS: usize = 11;
/// each texture gets its own tile in the albedo...
pub const TILE: usize = 2048 / CELLS;
/// ...with a border of the plain colour (UVs poke slightly outside their texture)
pub const PAD: usize = 18;
pub const INNER: usize = TILE - 2 * PAD;
const EYE_CELLS: [usize; 4] = [5, 6, 7, 8];
const EYE_PARTS: [usize; 4] = [16, 17, 18, 19];
const PEACE_PART: usize = 20;

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn i32_at(d: &[u8], o: usize) -> i32 {
    u32_at(d, o) as i32
}

fn put_f32s(d: &mut [u8], o: usize, v: &[f32]) {
    for (i, x) in v.iter().enumerate() {
        d[o + i * 4..o + i * 4 + 4].copy_from_slice(&x.to_le_bytes());
    }
}

pub struct Flver {
    pub d: Vec<u8>,
    data_off: usize,
    bone_off: usize,
    mesh_off: usize,
    faceset_off: usize,
    vbuf_off: usize,
    layout_off: usize,
    bones: usize,
    meshes: usize,
    facesets: usize,
}

struct Faceset {
    header: usize,
    count: usize,
    offset: usize,
    length: usize,
}

struct VertexBuffer {
    layout: usize,
    vsize: usize,
    vcount: usize,
    offset: usize,
    length: usize,
}

struct Mesh {
    facesets: Vec<usize>,
    vbufs: Vec<usize>,
}

impl Flver {
    pub fn new(data: Vec<u8>) -> Result<Self, String> {
        if !data.starts_with(b"FLVER\0") || data.len() < 0x80 {
            return Err("not a FLVER".into());
        }
        let d = &data;
        let c = |o: usize| u32_at(d, o) as usize;
        let (data_off, dummies, materials, bones, meshes, _vbufs) = (c(0x0C), c(0x14), c(0x18), c(0x1C), c(0x20), c(0x24));
        let facesets = c(0x50);
        let mut p = 0x80 + dummies * 0x40 + materials * 0x20;
        let bone_off = p;
        p += bones * 0x80;
        let mesh_off = p;
        p += meshes * 0x30;
        let faceset_off = p;
        p += facesets * 0x20;
        let vbuf_off = p;
        p += c(0x24) * 0x20;
        let layout_off = p;
        Ok(Self { d: data, data_off, bone_off, mesh_off, faceset_off, vbuf_off, layout_off, bones, meshes, facesets })
    }

    fn bone_names(&self) -> Vec<String> {
        (0..self.bones)
            .map(|i| super::bnd4::wstr(&self.d, i32_at(&self.d, self.bone_off + i * 0x80 + 12) as usize).unwrap_or_default())
            .collect()
    }

    fn facesets(&self) -> Vec<Faceset> {
        (0..self.facesets)
            .map(|i| {
                let a = self.faceset_off + i * 0x20;
                Faceset {
                    header: a,
                    count: i32_at(&self.d, a + 8) as usize,
                    offset: self.data_off + i32_at(&self.d, a + 12) as usize,
                    length: i32_at(&self.d, a + 16) as usize,
                }
            })
            .collect()
    }

    fn vertex_buffers(&self, i: usize) -> VertexBuffer {
        let a = self.vbuf_off + i * 0x20;
        let v = |k: usize| i32_at(&self.d, a + k * 4) as usize;
        VertexBuffer { layout: v(1), vsize: v(2), vcount: v(3), length: v(6), offset: self.data_off + v(7) }
    }

    /// (offset, type, semantic) of a layout's members
    fn layout_members(&self, layout: usize) -> Vec<(i32, i32, i32)> {
        let a = self.layout_off + layout * 0x10;
        let (n, members) = (i32_at(&self.d, a) as usize, i32_at(&self.d, a + 12) as usize);
        (0..n)
            .map(|k| {
                let m = members + k * 0x14;
                (i32_at(&self.d, m + 4), i32_at(&self.d, m + 8), i32_at(&self.d, m + 12))
            })
            .collect()
    }

    fn meshes(&self) -> Vec<Mesh> {
        (0..self.meshes)
            .map(|i| {
                let a = self.mesh_off + i * 0x30;
                let list = |n: usize, off: usize| (0..i32_at(&self.d, a + n) as usize).map(|k| i32_at(&self.d, i32_at(&self.d, a + off) as usize + k * 4) as usize).collect();
                Mesh { facesets: list(0x20, 0x24), vbufs: list(0x28, 0x2C) }
            })
            .collect()
    }

    fn clear_faceset(&mut self, fs: &Faceset) {
        self.d[fs.offset..fs.offset + fs.length].fill(0);
    }

    /// Every mesh emptied (no triangles).
    pub fn empty(mut self) -> Vec<u8> {
        for fs in self.facesets() {
            self.clear_faceset(&fs);
        }
        self.d
    }
}

// ---- Mario's vertices ------------------------------------------------------------------------

fn round_even(x: f64) -> f64 {
    x.round_ties_even()
}

fn color_key(c: [f32; 3]) -> [i32; 3] {
    c.map(|v| (v * 255.0).round_ties_even() as i32)
}

/// The distinct SM64 colours (first vertex of each triangle), sorted: one albedo band each.
pub fn mario_colors(model: &MarioModel) -> Vec<[i32; 3]> {
    let mut set: Vec<[i32; 3]> = model.tris.iter().map(|t| color_key(t.color[0])).collect();
    set.sort();
    set.dedup();
    set
}

fn mario_uv(colors: &[[i32; 3]], color: [f32; 3], uv: [f32; 2], textured: bool, cell: usize) -> [f64; 2] {
    let k = colors.iter().position(|c| *c == color_key(color)).unwrap_or(0) as f64;
    if !textured {
        return [(k * 340.0 + 168.0) / TEX, 1824.0 / TEX];
    }
    let lim = PAD as f64 / INNER as f64;
    let u = (uv[0] as f64 * CELLS as f64 - cell as f64).clamp(-lim, 1.0 + lim);
    let v = (uv[1] as f64).clamp(-lim, 1.0 + lim);
    [
        ((cell * TILE + PAD) as f64 + u * INNER as f64) / TEX,
        (k * 256.0 + PAD as f64 + v * INNER as f64) / TEX,
    ]
}

pub struct Vertex {
    pub pos: [f64; 3],
    pub normal: [f64; 3],
    pub uv: [f64; 2],
    pub part: usize,
}

pub fn textured(t: &Tri) -> bool {
    (0..2).any(|c| {
        let vals = t.uv.map(|uv| uv[c]);
        let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        hi - lo > 1e-4
    })
}

pub fn mean_u(t: &Tri) -> f32 {
    (t.uv[0][0] + t.uv[1][0] + t.uv[2][0]) / 3.0
}

/// Mario's mesh as indexed vertices: every eye triangle once per eye texture (each on its own
/// bone, the mod shows the one SM64 draws), the peace hand on its own bone.
pub fn mario_vertices(model: &MarioModel) -> (Vec<Vertex>, Vec<[u16; 3]>) {
    let colors = mario_colors(model);
    let all = model.tris.iter().map(|t| (t, t.part as usize)).chain(model.peace.iter().map(|t| (t, PEACE_PART)));
    let mut verts: Vec<Vertex> = Vec::new();
    let mut index: HashMap<(usize, [i64; 3], [i64; 2]), usize> = HashMap::new();
    let mut tris = Vec::new();
    for (t, part) in all {
        let tex = textured(t);
        let mu = mean_u(t);
        let cells = CELLS as f32;
        // the export caught Mario mid-blink: its eye triangles use the closed-eyes texture (cell 7)
        let eye = tex && part == 3 && 5.5 / cells <= mu && mu < 8.0 / cells;
        let cell = if eye { 7 } else { (mu * cells).clamp(0.0, cells - 1.0) as usize };
        let copies: Vec<(usize, Option<usize>)> =
            if eye { EYE_PARTS.iter().zip(EYE_CELLS).map(|(&p, c)| (p, Some(c))).collect() } else { vec![(part, None)] };
        for (owner, eye_cell) in copies {
            let mut tri = [0u16; 3];
            for k in 0..3 {
                let l = t.local[k];
                // SM64 is mirrored on X
                let p = [-(l[0] as f64) * UNIT, l[1] as f64 * UNIT, l[2] as f64 * UNIT];
                let mut uv = mario_uv(&colors, t.color[0], t.uv[k], tex, cell);
                if let Some(c) = eye_cell {
                    uv[0] += (c as f64 - 7.0) * TILE as f64 / TEX;
                }
                let key = (owner, p.map(|v| round_even(v * 1e5) as i64), uv.map(|v| round_even(v * 1e4) as i64));
                let i = *index.entry(key).or_insert_with(|| {
                    verts.push(Vertex { pos: p, normal: [0.0; 3], uv, part: owner });
                    verts.len() - 1
                });
                let n = t.normal[k];
                let v = &mut verts[i].normal;
                v[0] -= n[0] as f64;
                v[1] += n[1] as f64;
                v[2] += n[2] as f64;
                tri[k] = i as u16;
            }
            tris.push(tri);
        }
    }
    super::skateboard::append(&mut verts, &mut tris, &colors);
    (verts, tris)
}

fn snorm8(v: f64) -> u8 {
    round_even(v * 127.0 + 127.0).clamp(0.0, 255.0) as u8
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let l = if l == 0.0 { 1.0 } else { l };
    v.map(|x| x / l)
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// +1 if the game's front faces wind so that (b-a)x(c-a) points along the vertex normals, -1 if
/// against: measured on the armour's own triangles (triangle lists of u16 indices).
fn front_winding(f: &Flver, vb: &VertexBuffer, fs: &Faceset) -> Option<f64> {
    if f.d[fs.header + 4] != 0 {
        return None; // triangle strip
    }
    let vert = |i: usize| -> Option<([f64; 3], [f64; 3])> {
        let o = vb.offset + i * vb.vsize;
        (i < vb.vcount).then(|| {
            let p = [0, 1, 2].map(|k| f32::from_le_bytes(f.d[o + k * 4..o + k * 4 + 4].try_into().unwrap()) as f64);
            let n = [0, 1, 2].map(|k| (f.d[o + 12 + k] as f64 - 127.0) / 127.0);
            (p, n)
        })
    };
    let idx: Vec<usize> = f.d[fs.offset..fs.offset + fs.count * 2].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as usize).collect();
    let (mut along, mut against) = (0, 0);
    for t in idx.chunks(3).filter(|t| t.len() == 3) {
        let (Some(a), Some(b), Some(c)) = (vert(t[0]), vert(t[1]), vert(t[2])) else { continue };
        let face = cross(sub(b.0, a.0), sub(c.0, a.0));
        let n = [0, 1, 2].map(|k| a.1[k] + b.1[k] + c.1[k]);
        let d = dot(face, n);
        if d > 0.0 {
            along += 1;
        } else if d < 0.0 {
            against += 1;
        }
    }
    (along + against > 0).then(|| if along >= against { 1.0 } else { -1.0 })
}

/// Mario into mesh `target` of an armour FLVER, every other mesh emptied.
pub fn build_mario(mut f: Flver, model: &MarioModel, target: usize) -> Result<Vec<u8>, String> {
    let names = f.bone_names();
    let bone_ids = PART_BONES
        .iter()
        .map(|n| if n.is_empty() { Some(0) } else { names.iter().position(|b| b == n) })
        .collect::<Option<Vec<usize>>>()
        .ok_or("the chest piece's skeleton is not the expected one")?;
    let parents: Vec<i16> = (0..f.bones).map(|b| {
        let a = f.bone_off+b*0x80+0x1C;
        i16::from_le_bytes(f.d[a..a+2].try_into().unwrap())
    }).collect();
    if !super::skateboard::safe_attachment(&parents,&bone_ids) {
        return Err("skateboard bone must be a separate attachment with byte-sized skin indices".into());
    }
    // identity binds, so the hierarchy composes to identity for our bones
    for b in 0..f.bones {
        let a = f.bone_off + b * 0x80;
        put_f32s(&mut f.d, a, &[0.0; 3]);
        put_f32s(&mut f.d, a + 0x10, &[0.0; 3]);
        put_f32s(&mut f.d, a + 0x20, &[1.0; 3]);
    }
    for &b in &bone_ids[1..] {
        let a = f.bone_off + b * 0x80;
        if i32_at(&f.d, a + 0x3C) != 8 {
            return Err(format!("bone {} is not skinned by the chest piece", names[b]));
        }
        put_f32s(&mut f.d, a + 0x30, &[-1.0; 3]);
        put_f32s(&mut f.d, a + 0x40, &[1.0; 3]);
    }
    put_f32s(&mut f.d, 0x28, &[-1.5, -0.5, -1.5, 1.5, 2.5, 1.5]);
    let (meshes, fsets) = (f.meshes(), f.facesets());
    let mesh = meshes.get(target).ok_or("chest piece mesh missing")?;
    for (mi, m) in meshes.iter().enumerate() {
        if mi != target {
            for &fi in &m.facesets {
                f.clear_faceset(&fsets[fi]);
            }
        }
    }
    let (verts, tris) = mario_vertices(model);
    let vb = f.vertex_buffers(mesh.vbufs[0]);
    let members = f.layout_members(vb.layout);
    const EXPECTED: [(i32, i32, i32); 7] = [(0, 2, 0), (12, 17, 3), (16, 17, 6), (20, 17, 2), (24, 19, 1), (28, 19, 10), (32, 22, 5)];
    if vb.vsize != 40 || members.get(..7) != Some(&EXPECTED[..]) || verts.len() > vb.vcount {
        return Err("the chest piece's vertex layout is not the expected one".into());
    }
    // every triangle faces the way the game's front faces do (SM64's are mirrored on X), so back
    // faces can be culled like on the armour itself
    let winding = mesh.facesets.first().and_then(|&fi| front_winding(&f, &vb, &fsets[fi]));
    let mut tris = tris;
    if let Some(w) = winding {
        for t in &mut tris {
            let [a, b, c] = t.map(|i| &verts[i as usize]);
            let face = cross(sub(b.pos, a.pos), sub(c.pos, a.pos));
            let n = [0, 1, 2].map(|k| a.normal[k] + b.normal[k] + c.normal[k]);
            if dot(face, n) * w < 0.0 {
                t.swap(1, 2);
            }
        }
    }
    let mut buf = Vec::with_capacity(vb.vcount * 40);
    for v in &verts {
        let n = normalize(v.normal);
        let t = normalize(if n[1].abs() < 0.9 { cross(n, [0.0, 1.0, 0.0]) } else { cross(n, [1.0, 0.0, 0.0]) });
        for p in v.pos {
            buf.extend_from_slice(&(p as f32).to_le_bytes());
        }
        buf.extend(n.map(snorm8));
        buf.push(0);
        buf.extend(t.map(snorm8));
        buf.push(0);
        buf.extend_from_slice(&[bone_ids[v.part] as u8, 0, 0, 0, 255, 0, 0, 0]);
        // same as the armour's own vertices (the shader reads G/B as blend masks)
        buf.extend_from_slice(&[255, 0, 0, 255]);
        let (u, w) = (round_even(v.uv[0] * TEX) as i16, round_even(v.uv[1] * TEX) as i16);
        for x in [u, w, u, w] {
            buf.extend_from_slice(&x.to_le_bytes());
        }
    }
    // unused vertices repeat vertex 0
    let first: Vec<u8> = buf[..40].to_vec();
    while buf.len() < vb.vcount * 40 {
        buf.extend_from_slice(&first);
    }
    if buf.len() != vb.length {
        return Err("vertex buffer size mismatch".into());
    }
    f.d[vb.offset..vb.offset + vb.length].copy_from_slice(&buf);
    let idx: Vec<u8> = tris.iter().flatten().flat_map(|i| i.to_le_bytes()).collect();
    // Mario goes into the biggest faceset (LOD 0); the smaller LODs can't hold him, so their headers
    // point at that same index data: whichever LOD the game draws (shadows use lower ones, picked
    // by distance and angle), it is all of Mario (empty, they left him without one)
    let full = *mesh.facesets.iter().max_by_key(|&&fi| fsets[fi].count).ok_or("chest piece mesh has no facesets")?;
    let fs = &fsets[full];
    if idx.len() > fs.length {
        return Err("Mario does not fit in the chest piece's index buffer".into());
    }
    f.d[fs.offset..fs.offset + idx.len()].copy_from_slice(&idx);
    f.d[fs.offset + idx.len()..fs.offset + fs.length].fill(0);
    let (count, offset) = ((idx.len() / 2) as i32, i32_at(&f.d, fs.header + 12));
    for &fi in &mesh.facesets {
        let h = fsets[fi].header;
        f.d[h + 5] = if winding.is_some() { 1 } else { 0 }; // cull back faces (unknown winding: both sides)
        f.d[h + 8..h + 12].copy_from_slice(&count.to_le_bytes());
        f.d[h + 12..h + 16].copy_from_slice(&offset.to_le_bytes());
        f.d[h + 16..h + 20].copy_from_slice(&(fs.length as i32).to_le_bytes());
    }
    crate::log(format!("assets: Mario mesh {} vertices, {} triangles", verts.len(), tris.len()));
    Ok(f.d)
}
