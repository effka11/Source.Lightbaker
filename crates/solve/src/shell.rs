//! A lamp inside a closed shell lights nothing outside that shell.
//! The room around a lamp is not such a shell: nothing sits outside it.

use std::collections::{HashMap, HashSet};

use glam::Vec3;

use crate::embree::Scene;
use crate::geom::{Area, Triangle};

const TRACE: f32 = 1.0e6;
const NUDGE: f32 = 1.0e-3;

const DIRECTIONS: [Vec3; 3] = [
    Vec3::new(0.820, 0.431, 0.377),
    Vec3::new(-0.512, 0.724, 0.462),
    Vec3::new(0.247, -0.538, 0.806),
];

pub(crate) fn sealed_areas(triangles: &[Triangle], areas: &[Area]) -> Vec<usize> {
    if triangles.is_empty() || areas.is_empty() {
        return Vec::new();
    }

    let edges = edge_map(triangles);
    let groups = components(triangles.len(), &edges);
    let mut sealed = Vec::new();
    for members in groups {
        if !is_closed(&members, &edges) {
            continue;
        }
        let subset: Vec<Triangle> = members.iter().map(|&index| triangles[index]).collect();
        let shell = Scene::build(&subset);
        if !has_geometry_outside(&shell, &members, triangles) {
            continue;
        }
        for (index, area) in areas.iter().enumerate() {
            if sealed.contains(&index) {
                continue;
            }
            if inside(&shell, area.center()) {
                sealed.push(index);
            }
        }
    }
    sealed.sort_unstable();
    sealed
}

fn edge_map(triangles: &[Triangle]) -> HashMap<([i32; 3], [i32; 3]), Vec<usize>> {
    let mut edges: HashMap<([i32; 3], [i32; 3]), Vec<usize>> = HashMap::new();
    for (index, triangle) in triangles.iter().enumerate() {
        let keys = [
            vertex_key(triangle.vertices[0]),
            vertex_key(triangle.vertices[1]),
            vertex_key(triangle.vertices[2]),
        ];
        for edge in 0..3 {
            let mut ends = [keys[edge], keys[(edge + 1) % 3]];
            if ends[0] == ends[1] {
                continue;
            }
            if ends[0] > ends[1] {
                ends.swap(0, 1);
            }
            edges.entry((ends[0], ends[1])).or_default().push(index);
        }
    }
    edges
}

fn vertex_key(point: Vec3) -> [i32; 3] {
    [
        (point.x * 1000.0).round() as i32,
        (point.y * 1000.0).round() as i32,
        (point.z * 1000.0).round() as i32,
    ]
}

fn components(
    count: usize,
    edges: &HashMap<([i32; 3], [i32; 3]), Vec<usize>>,
) -> Vec<HashSet<usize>> {
    let mut parent: Vec<usize> = (0..count).collect();
    for shared in edges.values() {
        if shared.len() < 2 {
            continue;
        }
        for other in &shared[1..] {
            unite(&mut parent, shared[0], *other);
        }
    }

    let mut groups: HashMap<usize, HashSet<usize>> = HashMap::new();
    for index in 0..count {
        groups
            .entry(find(&mut parent, index))
            .or_default()
            .insert(index);
    }
    groups.into_values().collect()
}

fn find(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

fn unite(parent: &mut [usize], left: usize, right: usize) {
    let left = find(parent, left);
    let right = find(parent, right);
    if left != right {
        parent[right] = left;
    }
}

fn is_closed(members: &HashSet<usize>, edges: &HashMap<([i32; 3], [i32; 3]), Vec<usize>>) -> bool {
    if members.len() < 4 {
        return false;
    }
    for shared in edges.values() {
        if !shared.iter().any(|index| members.contains(index)) {
            continue;
        }
        if shared.len() != 2 || shared.iter().any(|index| !members.contains(index)) {
            return false;
        }
    }
    true
}

fn has_geometry_outside(shell: &Scene, members: &HashSet<usize>, triangles: &[Triangle]) -> bool {
    triangles.iter().enumerate().any(|(index, triangle)| {
        if members.contains(&index) {
            return false;
        }
        let centroid = (triangle.vertices[0] + triangle.vertices[1] + triangle.vertices[2]) / 3.0;
        !inside(shell, centroid)
    })
}

fn inside(shell: &Scene, point: Vec3) -> bool {
    DIRECTIONS
        .iter()
        .all(|direction| crossings(shell, point, *direction) % 2 == 1)
}

fn crossings(shell: &Scene, origin: Vec3, direction: Vec3) -> u32 {
    let direction = direction.normalize_or_zero();
    if direction == Vec3::ZERO {
        return 0;
    }
    let mut cursor = origin;
    let mut count = 0u32;
    let mut previous = usize::MAX;
    for _ in 0..48 {
        let Some(hit) = shell.hit(cursor, direction, TRACE) else {
            break;
        };
        if hit.distance < NUDGE || hit.prim == previous {
            cursor += direction * NUDGE;
            continue;
        }
        count += 1;
        previous = hit.prim;
        cursor = hit.point + direction * NUDGE;
    }
    count
}
