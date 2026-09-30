//! Displacement grids. Triangle order matches Source `CPowerInfo`, so the
//! lightmap sample-position lump indexes the same triangles.

use glam::Vec3;

const OFFSETS: [(i32, i32); 9] = [
    (1, -1),
    (0, -1),
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
];

const CHILDREN: [(i32, i32); 4] = [(1, 1), (-1, 1), (-1, -1), (1, -1)];

pub struct DispVert {
    pub vector: Vec3,
    pub dist: f32,
}

pub fn orient(corners: [Vec3; 4], start: Vec3) -> [Vec3; 4] {
    let mut best = 0usize;
    let mut best_d = f32::MAX;
    for (index, corner) in corners.iter().enumerate() {
        let distance = (*corner - start).length_squared();
        if distance < best_d {
            best_d = distance;
            best = index;
        }
    }
    // Vertex 0 sits on the start corner. The grid's first edge runs against
    // the face winding, which is how neighboring displacements were sewn.
    [
        corners[best],
        corners[(best + 3) % 4],
        corners[(best + 2) % 4],
        corners[(best + 1) % 4],
    ]
}

pub fn triangulation(power: i32) -> Vec<[u32; 3]> {
    let mut tris = Vec::new();
    if !(2..=4).contains(&power) {
        return tris;
    }
    let side = (1 << power) + 1;
    let root = side / 2;
    emit(power, side, (root, root), 0, &mut tris);
    tris
}

fn emit(power: i32, side: i32, node: (i32, i32), level: i32, tris: &mut Vec<[u32; 3]>) {
    let index = |x: i32, y: i32| (y * side + x) as u32;
    if level + 1 < power {
        let node_inc = 1 << (power - level - 2);
        for (mx, my) in CHILDREN {
            emit(
                power,
                side,
                (node.0 + mx * node_inc, node.1 + my * node_inc),
                level + 1,
                tris,
            );
        }
        return;
    }

    let step = 1 << (power - level - 1);
    let center = index(node.0, node.1);
    let mut previous: Option<u32> = None;
    for (ox, oy) in OFFSETS {
        let side_index = index(node.0 + ox * step, node.1 + oy * step);
        if let Some(prev) = previous {
            tris.push([prev, side_index, center]);
        }
        previous = Some(side_index);
    }
}

pub fn surface(corners: [Vec3; 4], verts: &[DispVert], power: i32) -> Option<Vec<Vec3>> {
    if !(2..=4).contains(&power) {
        return None;
    }
    let side = (1 << power) + 1;
    let size = 1 << power;
    let count = (side * side) as usize;
    if verts.len() < count {
        return None;
    }
    let mut points = Vec::with_capacity(count);
    for y in 0..side {
        for x in 0..side {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let flat = bilinear(corners, u, v);
            let vert = &verts[(y * side + x) as usize];
            points.push(flat + vert.vector * vert.dist);
        }
    }
    Some(points)
}

/// Lightmap cell `(s, t)` covers one fraction of the displacement and meets its neighbors.
pub fn cell(
    points: &[Vec3],
    power: i32,
    s: i32,
    t: i32,
    width: i32,
    height: i32,
) -> Option<[Vec3; 4]> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let w = width as f32;
    let h = height as f32;
    let u0 = s as f32 / w;
    let v0 = t as f32 / h;
    Some([
        at(points, power, u0, v0)?,
        at(points, power, u0 + 1.0 / w, v0)?,
        at(points, power, u0 + 1.0 / w, v0 + 1.0 / h)?,
        at(points, power, u0, v0 + 1.0 / h)?,
    ])
}

pub fn at(points: &[Vec3], power: i32, u: f32, v: f32) -> Option<Vec3> {
    if !(2..=4).contains(&power) {
        return None;
    }
    let size = 1 << power;
    let side = size + 1;
    if points.len() < (side * side) as usize {
        return None;
    }
    let u = u.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let x = u * size as f32;
    let y = v * size as f32;
    let x0 = (x.floor() as i32).clamp(0, size);
    let y0 = (y.floor() as i32).clamp(0, size);
    let x1 = (x0 + 1).min(size);
    let y1 = (y0 + 1).min(size);
    let fx = if x0 == x1 { 0.0 } else { x - x0 as f32 };
    let fy = if y0 == y1 { 0.0 } else { y - y0 as f32 };
    let point = |x: i32, y: i32| points[(y * side + x) as usize];
    let a = point(x0, y0);
    let b = point(x1, y0);
    let c = point(x1, y1);
    let d = point(x0, y1);
    Some(
        a * ((1.0 - fx) * (1.0 - fy))
            + b * (fx * (1.0 - fy))
            + c * (fx * fy)
            + d * ((1.0 - fx) * fy),
    )
}

pub fn bilinear(corners: [Vec3; 4], u: f32, v: f32) -> Vec3 {
    let a = 1.0 - u;
    let b = 1.0 - v;
    corners[0] * (a * b) + corners[1] * (u * b) + corners[2] * (u * v) + corners[3] * (a * v)
}

/// One lightmap sample: triangle index into [`triangulation`] and barycentric weights.
pub fn read_samples(
    data: &[u8],
    start: usize,
    count: usize,
    tri_count: usize,
) -> Vec<Option<(usize, [f32; 3])>> {
    let mut out = Vec::with_capacity(count);
    let mut cursor = start;
    for _ in 0..count {
        if cursor >= data.len() {
            out.push(None);
            continue;
        }
        let mut tri = data[cursor] as usize;
        cursor += 1;
        if tri == 255 {
            if cursor >= data.len() {
                out.push(None);
                continue;
            }
            tri = data[cursor] as usize + 255;
            cursor += 1;
        }
        if cursor + 2 >= data.len() {
            out.push(None);
            continue;
        }
        let bary = [
            data[cursor] as f32 / 255.9,
            data[cursor + 1] as f32 / 255.9,
            data[cursor + 2] as f32 / 255.9,
        ];
        cursor += 3;
        if tri >= tri_count {
            out.push(None);
        } else {
            out.push(Some((tri, bary)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_counts_match_source() {
        assert_eq!(triangulation(2).len(), 32);
        assert_eq!(triangulation(3).len(), 128);
        assert_eq!(triangulation(4).len(), 512);
        let side = (1 << 4) + 1;
        let max = triangulation(4).into_iter().flatten().max().unwrap();
        assert!(max < (side * side) as u32);
    }

    #[test]
    fn lightmap_cells_meet_and_follow_the_hill() {
        let corners = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(64.0, 0.0, 0.0),
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(0.0, 64.0, 0.0),
        ];
        let power = 2;
        let side = (1 << power) + 1;
        let mut verts = Vec::new();
        for _ in 0..(side * side) {
            verts.push(DispVert {
                vector: Vec3::Z,
                dist: 0.0,
            });
        }
        verts[(2 * side + 2) as usize].dist = 32.0;
        let points = surface(corners, &verts, power).unwrap();
        let left = cell(&points, power, 0, 0, 2, 2).unwrap();
        let right = cell(&points, power, 1, 0, 2, 2).unwrap();
        let far = cell(&points, power, 0, 1, 2, 2).unwrap();
        assert!((left[1] - right[0]).length() < 1.0e-3);
        assert!((left[2] - right[3]).length() < 1.0e-3);
        assert!((left[3] - far[0]).length() < 1.0e-3);
        assert!((left[0].z).abs() < 1.0e-3);
        assert!((left[2].z - 32.0).abs() < 1.0e-3);
    }
}
