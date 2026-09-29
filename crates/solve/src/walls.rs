//! Nearest wall luxel for a bounce hit.
//!
//! The mock room is small enough that a full scan is the reference. A map has
//! hundreds of thousands of walls, so the same predicate is answered from a
//! grid: direction first (a floor hit does not walk vertical walls), then the
//! cells around the hit. Empty cells are a bounds check, not a search. Ties
//! keep the smaller receiver index.

use glam::Vec3;

use crate::geom::{Receiver, Role};

const CELL: f32 = 256.0;
const SEARCH: f32 = 768.0;
const ALIGN: f32 = 0.4;
const PLANE_EPS: f32 = 1.5;

const AXES: [Vec3; 6] = [
    Vec3::X,
    Vec3::NEG_X,
    Vec3::Y,
    Vec3::NEG_Y,
    Vec3::Z,
    Vec3::NEG_Z,
];

pub(crate) struct WallIndex {
    dirs: [Dir; 6],
}

impl WallIndex {
    pub(crate) fn build(receivers: &[Receiver]) -> Self {
        let mut buckets: [Vec<usize>; 6] = Default::default();
        for (index, receiver) in receivers.iter().enumerate() {
            if receiver.role != Role::Wall {
                continue;
            }
            let normal = receiver.normal.normalize_or_zero();
            if normal == Vec3::ZERO || !receiver.position.is_finite() {
                continue;
            }
            for (slot, axis) in AXES.iter().enumerate() {
                if normal.dot(*axis) >= ALIGN {
                    buckets[slot].push(index);
                }
            }
        }
        Self {
            dirs: std::array::from_fn(|slot| Dir::build(AXES[slot], receivers, &buckets[slot])),
        }
    }

    pub(crate) fn any(&self) -> bool {
        self.dirs.iter().any(|dir| !dir.indices.is_empty())
    }

    pub(crate) fn nearest(
        &self,
        receivers: &[Receiver],
        point: Vec3,
        hit_normal: Vec3,
        gather: usize,
    ) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for dir in &self.dirs {
            if dir.axis.dot(hit_normal) < ALIGN {
                continue;
            }
            dir.search(receivers, point, hit_normal, gather, &mut best);
        }
        best.map(|(index, _)| index)
    }
}

struct Dir {
    axis: Vec3,
    origin: Vec3,
    cell: f32,
    dims: [i32; 3],
    offsets: Vec<u32>,
    indices: Vec<u32>,
}

impl Dir {
    fn build(axis: Vec3, receivers: &[Receiver], indices: &[usize]) -> Self {
        if indices.is_empty() {
            return Self::empty(axis);
        }
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for &index in indices {
            let point = receivers[index].position;
            min = min.min(point);
            max = max.max(point);
        }
        let extent = max - min;
        let mut cell = CELL;
        let (dims, cells) = loop {
            let dims = [
                (extent.x / cell).floor() as i32 + 1,
                (extent.y / cell).floor() as i32 + 1,
                (extent.z / cell).floor() as i32 + 1,
            ];
            let count = (dims[0] as usize)
                .checked_mul(dims[1] as usize)
                .and_then(|value| value.checked_mul(dims[2] as usize))
                .filter(|count| *count <= 3_000_000);
            if let Some(count) = count {
                break (dims, count);
            }
            cell *= 2.0;
        };

        let mut counts = vec![0u32; cells];
        for &index in indices {
            let flat = flat_clamped(min, cell, dims, receivers[index].position);
            counts[flat] += 1;
        }
        let mut offsets = Vec::with_capacity(cells + 1);
        offsets.push(0u32);
        for count in &counts {
            offsets.push(offsets.last().copied().unwrap_or(0) + count);
        }
        let mut packed = vec![0u32; *offsets.last().unwrap_or(&0) as usize];
        let mut cursor = offsets.clone();
        for &index in indices {
            let flat = flat_clamped(min, cell, dims, receivers[index].position);
            let slot = cursor[flat] as usize;
            packed[slot] = index as u32;
            cursor[flat] += 1;
        }
        Self {
            axis,
            origin: min,
            cell,
            dims,
            offsets,
            indices: packed,
        }
    }

    fn empty(axis: Vec3) -> Self {
        Self {
            axis,
            origin: Vec3::ZERO,
            cell: CELL,
            dims: [0, 0, 0],
            offsets: Vec::new(),
            indices: Vec::new(),
        }
    }

    fn search(
        &self,
        receivers: &[Receiver],
        point: Vec3,
        hit_normal: Vec3,
        gather: usize,
        best: &mut Option<(usize, f32)>,
    ) {
        if self.indices.is_empty() {
            return;
        }
        let origin = self.cell(point);
        let max_ring = (SEARCH / self.cell).ceil() as i32 + 1;
        let limit2 = SEARCH * SEARCH;
        for ring in 0..=max_ring {
            if let Some((_, dist2)) = *best {
                let min_dist = (ring as f32 - 1.0) * self.cell;
                if min_dist > 0.0 && min_dist * min_dist > dist2 {
                    break;
                }
            }
            for_shell(ring, |x, y, z| {
                for &index in self.range(origin[0] + x, origin[1] + y, origin[2] + z) {
                    consider(
                        receivers,
                        index as usize,
                        point,
                        hit_normal,
                        gather,
                        limit2,
                        best,
                    );
                }
            });
        }
    }

    fn cell(&self, point: Vec3) -> [i32; 3] {
        let local = point - self.origin;
        [
            (local.x / self.cell).floor() as i32,
            (local.y / self.cell).floor() as i32,
            (local.z / self.cell).floor() as i32,
        ]
    }

    fn range(&self, x: i32, y: i32, z: i32) -> &[u32] {
        let Some(flat) = self.flat(x, y, z) else {
            return &[];
        };
        let start = self.offsets[flat] as usize;
        let end = self.offsets[flat + 1] as usize;
        &self.indices[start..end]
    }

    fn flat(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        if x < 0 || y < 0 || z < 0 || x >= self.dims[0] || y >= self.dims[1] || z >= self.dims[2] {
            return None;
        }
        Some(
            ((z as usize * self.dims[1] as usize) + y as usize) * self.dims[0] as usize
                + x as usize,
        )
    }
}

fn flat_clamped(origin: Vec3, cell_size: f32, dims: [i32; 3], point: Vec3) -> usize {
    let local = point - origin;
    let mut cell = [
        (local.x / cell_size).floor() as i32,
        (local.y / cell_size).floor() as i32,
        (local.z / cell_size).floor() as i32,
    ];
    for slot in 0..3 {
        if cell[slot] < 0 {
            cell[slot] = 0;
        }
        if cell[slot] >= dims[slot] {
            cell[slot] = dims[slot] - 1;
        }
    }
    ((cell[2] as usize * dims[1] as usize) + cell[1] as usize) * dims[0] as usize + cell[0] as usize
}

fn for_shell(ring: i32, mut visit: impl FnMut(i32, i32, i32)) {
    if ring == 0 {
        visit(0, 0, 0);
        return;
    }
    for z in -ring..=ring {
        for y in -ring..=ring {
            for x in -ring..=ring {
                if x.abs() == ring || y.abs() == ring || z.abs() == ring {
                    visit(x, y, z);
                }
            }
        }
    }
}

fn consider(
    receivers: &[Receiver],
    index: usize,
    point: Vec3,
    hit_normal: Vec3,
    gather: usize,
    limit2: f32,
    best: &mut Option<(usize, f32)>,
) {
    if index == gather {
        return;
    }
    let wall = &receivers[index];
    let normal = wall.normal.normalize_or_zero();
    if normal.dot(hit_normal) < 0.5 {
        return;
    }
    if (point - wall.position).dot(normal).abs() > PLANE_EPS {
        return;
    }
    let distance = point.distance_squared(wall.position);
    if distance > limit2 {
        return;
    }
    let replace = match *best {
        Some((best_index, best_distance)) => {
            distance < best_distance || (distance == best_distance && index < best_index)
        }
        None => true,
    };
    if replace {
        *best = Some((index, distance));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::room;

    fn scan(receivers: &[Receiver], point: Vec3, hit_normal: Vec3, gather: usize) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for index in 0..receivers.len() {
            if receivers[index].role != Role::Wall {
                continue;
            }
            consider(
                receivers,
                index,
                point,
                hit_normal,
                gather,
                f32::MAX,
                &mut best,
            );
        }
        best.map(|(index, _)| index)
    }

    #[test]
    fn index_matches_the_linear_scan() {
        let fixture = room();
        let receivers: Vec<_> = fixture.luxels.iter().map(|luxel| luxel.receiver).collect();
        let index = WallIndex::build(&receivers);
        for (gather, receiver) in receivers.iter().enumerate() {
            let point = receiver.position + receiver.normal.normalize_or_zero() * 0.25;
            let normal = receiver.normal.normalize_or_zero();
            assert_eq!(
                index.nearest(&receivers, point, normal, gather),
                scan(&receivers, point, normal, gather),
                "gather {gather} at {}",
                receiver.position
            );
        }
    }
}
