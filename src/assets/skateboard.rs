//! Original32vertex deck and four wheels, using Mario's existing atlas.
use super::flver::Vertex;
pub const BOARD: usize = 21;
/// A hidden deck must never scale a different body part through its ancestry.
pub fn safe_attachment(parents: &[i16], parts: &[usize]) -> bool {
    if parts.len() != BOARD + 1
        || parts
            .iter()
            .any(|&b| b >= parents.len() || b > u8::MAX as usize)
    {
        return false;
    }
    for &bone in &parts[1..] {
        let mut p = parents[bone];
        let mut steps = 0;
        while p >= 0 {
            let i = p as usize;
            if i >= parents.len() || i == parts[BOARD] || steps >= parents.len() {
                return false;
            }
            p = parents[i];
            steps += 1;
        }
    }
    true
}
pub fn append(verts: &mut Vec<Vertex>, tris: &mut Vec<[u16; 3]>, colors: &[[i32; 3]]) {
    let brown = colors
        .iter()
        .enumerate()
        .min_by_key(|(_, c)| {
            c.iter()
                .zip([115, 59, 24])
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<i32>()
        })
        .map_or(0, |(i, _)| i);
    let deck_uv = [(brown as f64 * 340.0 + 168.0) / 2048.0, 1824.0 / 2048.0];
    let base = u16::try_from(verts.len()).expect("board vertex budget");
    for x in [-0.035, 0.0] {
        for y in [-0.32, 0.32] {
            for z in [-0.13, 0.13] {
                let n: [f64; 3] = [if x < 0.0 { -1.0 } else { 1.0 }, y / 0.32, z / 0.13];
                verts.push(Vertex {
                    pos: [x, y, z],
                    normal: n.map(|v| v / 3.0f64.sqrt()),
                    uv: deck_uv,
                    part: BOARD,
                });
            }
        }
    }
    for t in [
        [0, 1, 3],
        [0, 3, 2],
        [4, 6, 7],
        [4, 7, 5],
        [0, 4, 5],
        [0, 5, 1],
        [2, 3, 7],
        [2, 7, 6],
        [0, 2, 6],
        [0, 6, 4],
        [1, 5, 7],
        [1, 7, 3],
    ] {
        tris.push(t.map(|i| base + i));
    }
    for y in [-0.20, 0.20] {
        for z in [-0.14, 0.14] {
            let base = u16::try_from(verts.len()).expect("board wheel budget");
            for n in [
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, 0.0, -1.0],
                [0.0, 0.0, 1.0],
            ] {
                verts.push(Vertex {
                    pos: [0.05 + n[0] * 0.05, y + n[1] * 0.05, z + n[2] * 0.025],
                    normal: n,
                    uv: [2040.0 / 2048.0, 1536.0 / 2048.0],
                    part: BOARD,
                });
            }
            for i in 0..4 {
                tris.extend([
                    [base + 4, base + i, base + (i + 1) % 4],
                    [base + 5, base + (i + 1) % 4, base + i],
                ]);
            }
        }
    }
}
