//! Winding for Havok triangles reflected into SM64 space.
//! Walls never pick their side from Mario's position: crossing a plane or refreshing the
//! query must not turn the exterior of a rock into its interior.

/// Only decoded convex hulls and synthetic boxes have a trustworthy interior point.
/// A jump must not flip their sloped faces; open terrain and merely closed meshes retain
/// the height heuristic because their vertex average need not be inside the solid.
pub fn body_surface_vertices(v: [[i32; 3]; 3], center: Option<[f32; 3]>, mario: [f32; 3], convex_or_box: bool) -> Option<[[i32; 3]; 3]> {
    let height_hint = if convex_or_box && center.is_some() { None } else { Some(mario) };
    surface_vertices(v, center, height_hint)
}

pub fn surface_vertices(mut v: [[i32; 3]; 3], convex_center: Option<[f32; 3]>, mario: Option<[f32; 3]>) -> Option<[[i32; 3]; 3]> {
    // er_to_sm reflects X, which reverses handedness.
    v.swap(1, 2);
    let a = v[0].map(|x| x as f64);
    let b = v[1].map(|x| x as f64);
    let c = v[2].map(|x| x as f64);
    let u: [f64; 3] = std::array::from_fn(|i| b[i] - a[i]);
    let w: [f64; 3] = std::array::from_fn(|i| c[i] - a[i]);
    let n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
    if n.iter().map(|x| x * x).sum::<f64>() < 1.0 {
        return None;
    }
    let len = n.iter().map(|x| x * x).sum::<f64>().sqrt();
    if let Some(m) = mario.filter(|_| n[1].abs() > 0.2 * len) {
        // floors and ceilings get faced by Mario's height like before: some map meshes are wound
        // the other way round as a whole, and some convex centres come out wrong, either way
        // the ground turns into a ceiling. judged where Mario is, clamped for long ramps.
        let lo = a[1].min(b[1]).min(c[1]);
        let hi = a[1].max(b[1]).max(c[1]);
        let y = (a[1] - (n[0] * (m[0] as f64 - a[0]) + n[2] * (m[2] as f64 - a[2])) / n[1]).clamp(lo, hi);
        if (y < m[1] as f64 + 120.0) != (n[1] > 0.0) {
            v.swap(1, 2);
        }
    } else if let Some(mid) = convex_center {
        // Convex face indices and synthetic boxes need an explicit outward orientation.
        let dot: f64 = (0..3).map(|i| n[i] * ((a[i] + b[i] + c[i]) / 3.0 - mid[i] as f64)).sum();
        if dot < 0.0 {
            v.swap(1, 2);
        }
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal(v: [[i32; 3]; 3]) -> [i32; 3] {
        let a: [i32; 3] = std::array::from_fn(|i| v[1][i] - v[0][i]);
        let b: [i32; 3] = std::array::from_fn(|i| v[2][i] - v[0][i]);
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    }

    #[test]
    fn reflected_mesh_preserves_floor_ceiling_and_rock_exterior() {
        // A Havok upward floor after reflecting X must still be a floor.
        let floor = [[0, 0, 0], [0, 0, 100], [-100, 0, 0]];
        assert!(normal(surface_vertices(floor, None, None).unwrap())[1] > 0);
        let ceiling = [floor[0], floor[2], floor[1]];
        assert!(normal(surface_vertices(ceiling, None, None).unwrap())[1] < 0);
        // A leaning rock side keeps its outward normal across every query, including when
        // Mario's feet are below its plane or have penetrated it slightly.
        let rock = [[0, 0, 0], [0, 100, 100], [-20, 100, 0]];
        let result = surface_vertices(rock, None, None).unwrap();
        assert!(normal(result)[0] > 0);
        assert_eq!(surface_vertices(rock, None, None), Some(result));
    }

    #[test]
    fn flipped_ground_mesh_is_still_a_floor_under_mario() {
        let ceiling = [[0, 0, 0], [-100, 0, 0], [0, 0, 100]];
        assert!(normal(surface_vertices(ceiling, None, None).unwrap())[1] < 0);
        assert!(normal(surface_vertices(ceiling, None, Some([-20.0, 5.0, 20.0])).unwrap())[1] > 0);
        assert!(normal(surface_vertices(ceiling, None, Some([-20.0, -300.0, 20.0])).unwrap())[1] < 0);
    }

    #[test]
    fn convex_faces_are_outward_even_with_inconsistent_input_winding() {
        let top = [[-100, 100, -100], [100, 100, -100], [100, 100, 100]];
        for input in [top, [top[0], top[2], top[1]]] {
            assert!(normal(surface_vertices(input, Some([0.0; 3]), None).unwrap())[1] > 0);
        }
        let side = [[100, -100, -100], [100, 100, -100], [100, 100, 100]];
        assert!(normal(surface_vertices(side, Some([0.0; 3]), None).unwrap())[0] > 0);
        let bottom = top.map(|p| [p[0], -100, p[2]]);
        assert!(normal(surface_vertices(bottom, Some([0.0; 3]), None).unwrap())[1] < 0);
    }

    #[test]
    fn quantized_degenerate_triangles_are_rejected() {
        assert_eq!(surface_vertices([[0; 3]; 3], None, None), None);
        assert_eq!(surface_vertices([[0; 3], [1; 3], [2; 3]], None, None), None);
    }

    #[test]
    fn solid_sloped_face_keeps_its_exterior_through_a_jump() {
        // A convex rock's inclined side (positive X and Y exterior) used to change
        // facing when Mario crossed its height band during a collision refresh.
        let face = [[100, -100, -100], [-100, 100, -100], [-100, 100, 100]];
        let center = Some([0.0, -100.0, 0.0]);
        let expected = body_surface_vertices(face, center, [200.0, -300.0, 0.0], true).unwrap();
        assert!(normal(expected)[0] > 0 && normal(expected)[1] > 0);
        for height in [-300.0, -100.0, 0.0, 200.0, 500.0] {
            for input in [face, [face[0], face[2], face[1]]] {
                let result = body_surface_vertices(input, center, [200.0, height, 0.0], true).unwrap();
                assert!(normal(result)[0] > 0 && normal(result)[1] > 0);
            }
        }
        // Open terrain and unproven closed meshes still distinguish overhead ground.
        assert!(normal(body_surface_vertices(face, center, [200.0, -300.0, 0.0], false).unwrap())[1] < 0);
        assert!(normal(body_surface_vertices(face, center, [200.0, 500.0, 0.0], false).unwrap())[1] > 0);
    }
}
