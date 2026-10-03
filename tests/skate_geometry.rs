//! Standalone production board/FLVER regressions; no ROM or game assets required.
#![allow(dead_code)]
mod model {
    #[derive(Clone, Copy)]
    pub struct Tri {
        pub part: i32,
        pub local: [[f32; 3]; 3],
        pub normal: [[f32; 3]; 3],
        pub uv: [[f32; 2]; 3],
        pub color: [[f32; 3]; 3],
        pub world: [[f32; 3]; 3],
    }
    pub struct MarioModel {
        pub tris: Vec<Tri>,
        pub peace: Vec<Tri>,
    }
}
fn log(_: impl AsRef<str>) {}
#[path = "../src/assets/bnd4.rs"]
mod bnd4;
#[path = "../src/assets/flver.rs"]
mod flver;
#[path = "../src/assets/skateboard.rs"]
mod skateboard;

#[test]
fn original_board_is_small_closed_and_outward_facing() {
    let (mut vertices, mut triangles) = (Vec::new(), Vec::new());
    skateboard::append(
        &mut vertices,
        &mut triangles,
        &[[0, 0, 0], [115, 59, 24], [255, 255, 255]],
    );
    assert_eq!(vertices.len(), 32);
    assert_eq!(triangles.len(), 44);
    assert!(vertices.iter().all(|v| {
        v.part == 21
            && v.pos
                .iter()
                .chain(v.normal.iter())
                .chain(v.uv.iter())
                .all(|x| x.is_finite())
    }));
    let mut edges = std::collections::HashMap::new();
    for t in triangles {
        let p = t.map(|i| vertices[i as usize].pos);
        let a: [f64; 3] = std::array::from_fn(|i| p[1][i] - p[0][i]);
        let b: [f64; 3] = std::array::from_fn(|i| p[2][i] - p[0][i]);
        let cross = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let normal: [f64; 3] =
            std::array::from_fn(|i| t.iter().map(|&v| vertices[v as usize].normal[i]).sum());
        assert!(cross.iter().zip(normal).map(|(a, b)| a * b).sum::<f64>() > 0.0);
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    assert!(edges.values().all(|&uses| uses == 2));
    assert!((vertices[0].uv[0] - (340.0 + 168.0) / 2048.0).abs() < 1e-12);
    assert_eq!(vertices[8].uv, [2040.0 / 2048.0, 1536.0 / 2048.0]);
}

#[test]
fn production_builder_preserves_native_geometry_and_appends_only_board() {
    let native = model::Tri {
        part: 1,
        local: [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]],
        normal: [[0.0, 0.0, 1.0]; 3],
        uv: [[0.0, 0.0]; 3],
        color: [[115.0 / 255.0, 59.0 / 255.0, 24.0 / 255.0]; 3],
        world: [[0.0; 3]; 3],
    };
    let (vertices, triangles) = flver::mario_vertices(&model::MarioModel {
        tris: vec![native],
        peace: vec![],
    });
    assert_eq!(vertices.len(), 35);
    assert_eq!(triangles.len(), 45);
    assert_eq!(triangles[0], [0, 1, 2]);
    assert!(vertices[..3].iter().all(|v| v.part == 1));
    assert!(vertices[3..].iter().all(|v| v.part == skateboard::BOARD));
    assert_eq!(vertices[1].pos, [-0.25, 0.0, 0.0]);
    assert_eq!(flver::PART_BONES.len(), 22);
    assert_eq!(flver::PART_BONES[21], "R_Calf");
}

#[test]
fn hidden_board_requires_safe_byte_sized_attachment() {
    let mut parents = vec![-1; 24];
    let parts: Vec<usize> = (0..22).collect();
    assert!(skateboard::safe_attachment(&parents, &parts));
    parents[2] = 21;
    assert!(!skateboard::safe_attachment(&parents, &parts));
    parents[2] = 2;
    assert!(!skateboard::safe_attachment(&parents, &parts));
    let mut oversized = parts;
    oversized[21] = 256;
    assert!(!skateboard::safe_attachment(&vec![-1; 257], &oversized));
}
