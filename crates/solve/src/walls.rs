//! Nearest wall luxel for a bounce hit.
//!
//! The mock room is small enough that a full scan is the reference. A map has
//! hundreds of thousands of walls and tens of millions of hits, two thirds of
//! which land where no coplanar wall luxel exists at all. The same predicate is
//! answered from a tree: luxels that share one exact normal form a subtree that
//! is pruned by the offset of their plane, everything else is pruned by a cone
//! of normals, and every node is pruned by the distance to its box. A miss then
//! costs a walk down the tree, not a scan of the neighbourhood. Ties keep the
//! smaller receiver index.

use glam::Vec3;

use crate::geom::{Receiver, Role};

const SEARCH: f32 = 768.0;
/// A candidate's normal must lie within 60° of the hit normal.
const ALIGN: f32 = 0.5;
const PLANE_EPS: f32 = 1.5;
/// Rounding slack on the plane offset bound, in world units.
const PLANE_SLACK: f32 = 0.1;
const LEAF: usize = 8;
/// Luxels sharing one exact normal form their own subtree from this many on.
const MIN_GROUP: usize = 16;
const NO_CHILD: u32 = u32::MAX;

pub(crate) struct WallIndex {
    /// Wall receivers in leaf order.
    entries: Vec<Entry>,
    nodes: Vec<Node>,
    /// Subtree roots: one per exact-normal group, then one for the rest.
    roots: Vec<u32>,
}

#[derive(Clone, Copy)]
struct Entry {
    position: Vec3,
    normal: Vec3,
    index: u32,
}

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Axis of the cone holding every normal below.
    cone: Vec3,
    /// Prune when `cone · hit_normal` is below this: no normal below can be aligned.
    prune_cos: f32,
    /// `Some` when every entry below has bitwise the same normal.
    plane: Option<Plane>,
    /// Leaf: `[first, first + count)` in `entries`. Inner: children.
    first: u32,
    count: u32,
    left: u32,
    right: u32,
}

#[derive(Clone, Copy)]
struct Plane {
    normal: Vec3,
    lo: f32,
    hi: f32,
}

impl WallIndex {
    pub(crate) fn build(receivers: &[Receiver]) -> Self {
        let mut entries: Vec<Entry> = receivers
            .iter()
            .enumerate()
            .filter(|(_, receiver)| receiver.role == Role::Wall)
            .filter_map(|(index, receiver)| {
                let normal = receiver.normal.normalize_or_zero();
                if normal == Vec3::ZERO || !receiver.position.is_finite() {
                    return None;
                }
                Some(Entry {
                    position: receiver.position,
                    normal,
                    index: index as u32,
                })
            })
            .collect();
        let mut index = Self {
            entries: Vec::new(),
            nodes: Vec::new(),
            roots: Vec::new(),
        };
        if entries.is_empty() {
            return index;
        }

        // Runs of one exact normal, largest groups first; the small ones share a tree.
        entries.sort_unstable_by_key(|entry| normal_key(entry.normal));
        let mut ordered = Vec::with_capacity(entries.len());
        let mut rest = Vec::new();
        let mut start = 0;
        while start < entries.len() {
            let key = normal_key(entries[start].normal);
            let mut end = start;
            while end < entries.len() && normal_key(entries[end].normal) == key {
                end += 1;
            }
            if end - start >= MIN_GROUP {
                let first = ordered.len();
                ordered.extend_from_slice(&entries[start..end]);
                let root = index.split(&mut ordered, first, end - start);
                index.roots.push(root);
            } else {
                rest.extend_from_slice(&entries[start..end]);
            }
            start = end;
        }
        if !rest.is_empty() {
            let first = ordered.len();
            ordered.extend_from_slice(&rest);
            let root = index.split(&mut ordered, first, rest.len());
            index.roots.push(root);
        }
        index.entries = ordered;
        index
    }

    pub(crate) fn any(&self) -> bool {
        !self.entries.is_empty()
    }

    pub(crate) fn nearest(
        &self,
        receivers: &[Receiver],
        point: Vec3,
        hit_normal: Vec3,
        gather: usize,
    ) -> Option<usize> {
        let _ = receivers;
        if self.entries.is_empty() {
            return None;
        }
        let mut best: Option<(u32, f32)> = None;
        let mut bound = SEARCH * SEARCH;
        for &root in &self.roots {
            self.descend(
                root,
                point,
                hit_normal,
                gather as u32,
                &mut best,
                &mut bound,
            );
        }
        stats::lookup(best.map(|(_, distance)| distance.sqrt()));
        best.map(|(index, _)| index as usize)
    }

    /// Builds the subtree over `entries[first..first + count]`, reordering that
    /// slice in place, and returns the node index.
    fn split(&mut self, entries: &mut [Entry], first: usize, count: usize) -> u32 {
        let slice = &mut entries[first..first + count];
        let mut node = Node::over(slice, first as u32, count as u32);
        let slot = self.nodes.len() as u32;
        self.nodes.push(node);
        if count <= LEAF {
            return slot;
        }
        let extent = node.max - node.min;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let mid = count / 2;
        slice.select_nth_unstable_by(mid, |left, right| {
            left.position[axis].total_cmp(&right.position[axis])
        });
        node.left = self.split(entries, first, mid);
        node.right = self.split(entries, first + mid, count - mid);
        node.count = 0;
        self.nodes[slot as usize] = node;
        slot
    }

    fn descend(
        &self,
        slot: u32,
        point: Vec3,
        hit_normal: Vec3,
        gather: u32,
        best: &mut Option<(u32, f32)>,
        bound: &mut f32,
    ) {
        let node = &self.nodes[slot as usize];
        stats::node();
        if node.cone.dot(hit_normal) < node.prune_cos {
            return;
        }
        if let Some(plane) = node.plane {
            if plane.normal.dot(hit_normal) < ALIGN {
                return;
            }
            let offset = point.dot(plane.normal);
            if offset < plane.lo - PLANE_EPS - PLANE_SLACK
                || offset > plane.hi + PLANE_EPS + PLANE_SLACK
            {
                return;
            }
        }
        if box_distance_squared(node.min, node.max, point) > *bound {
            return;
        }
        if node.count > 0 {
            let start = node.first as usize;
            for entry in &self.entries[start..start + node.count as usize] {
                consider_entry(entry, point, hit_normal, gather, best, bound);
            }
            return;
        }
        let left = &self.nodes[node.left as usize];
        let right = &self.nodes[node.right as usize];
        let left_distance = box_distance_squared(left.min, left.max, point);
        let right_distance = box_distance_squared(right.min, right.max, point);
        let (near, far) = if left_distance <= right_distance {
            (node.left, node.right)
        } else {
            (node.right, node.left)
        };
        self.descend(near, point, hit_normal, gather, best, bound);
        self.descend(far, point, hit_normal, gather, best, bound);
    }
}

impl Node {
    fn over(entries: &[Entry], first: u32, count: u32) -> Self {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut sum = Vec3::ZERO;
        let key = normal_key(entries[0].normal);
        let mut uniform = true;
        for entry in entries {
            min = min.min(entry.position);
            max = max.max(entry.position);
            sum += entry.normal;
            uniform &= normal_key(entry.normal) == key;
        }
        let cone = sum.normalize_or_zero();
        let cone = if cone == Vec3::ZERO {
            entries[0].normal
        } else {
            cone
        };
        let widest = entries
            .iter()
            .map(|entry| entry.normal.dot(cone))
            .fold(1.0f32, f32::min)
            .clamp(-1.0, 1.0);
        // Every normal lies within `half` of the cone axis. A hit normal farther
        // than `half + 60°` from the axis is beyond 60° of all of them.
        let half = widest.acos();
        let reach = half + ALIGN.acos();
        let prune_cos = if reach >= std::f32::consts::PI {
            -2.0
        } else {
            // `acos` is steep near 1, so the bound is loosened by a little more
            // than the rounding of `widest` can move it.
            reach.cos() - 1.0e-3
        };
        let plane = uniform.then(|| {
            let normal = entries[0].normal;
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for entry in entries {
                let offset = entry.position.dot(normal);
                lo = lo.min(offset);
                hi = hi.max(offset);
            }
            Plane { normal, lo, hi }
        });
        Self {
            min,
            max,
            cone,
            prune_cos,
            plane,
            first,
            count,
            left: NO_CHILD,
            right: NO_CHILD,
        }
    }
}

impl Drop for WallIndex {
    fn drop(&mut self) {
        stats::report();
    }
}

/// Temporary counters for the gm_construct measurement. Removed before finishing.
pub(crate) mod stats {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    // lookups, misses, nodes, found <=32, <=64, <=128, <=256, <=768
    const SLOTS: usize = 8;
    static TOTALS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

    thread_local! {
        static LOCAL: [Cell<u64>; SLOTS] = [const { Cell::new(0) }; SLOTS];
    }

    fn bump(slot: usize, by: u64) {
        LOCAL.with(|local| local[slot].set(local[slot].get() + by));
    }

    pub fn node() {
        bump(2, 1);
    }

    pub fn lookup(found: Option<f32>) {
        bump(0, 1);
        match found {
            None => bump(1, 1),
            Some(distance) => {
                let slot = if distance <= 32.0 {
                    3
                } else if distance <= 64.0 {
                    4
                } else if distance <= 128.0 {
                    5
                } else if distance <= 256.0 {
                    6
                } else {
                    7
                };
                bump(slot, 1);
            }
        }
        LOCAL.with(|local| {
            if local[0].get() % 4096 == 0 {
                for (cell, total) in local.iter().zip(TOTALS.iter()) {
                    total.fetch_add(cell.replace(0), Relaxed);
                }
            }
        });
    }

    pub fn report() {
        if std::env::var("LB_STATS").is_err() {
            return;
        }
        let totals: Vec<u64> = TOTALS.iter().map(|total| total.load(Relaxed)).collect();
        eprintln!(
            "walls: lookups {} misses {} nodes {} found<=32 {} <=64 {} <=128 {} <=256 {} <=768 {}",
            totals[0], totals[1], totals[2], totals[3], totals[4], totals[5], totals[6], totals[7]
        );
    }
}

fn normal_key(normal: Vec3) -> [u32; 3] {
    [normal.x.to_bits(), normal.y.to_bits(), normal.z.to_bits()]
}

fn box_distance_squared(min: Vec3, max: Vec3, point: Vec3) -> f32 {
    point.distance_squared(point.clamp(min, max))
}

/// The same predicate as [`consider`], on a stored entry. `bound` shrinks to the
/// best distance so far, so later boxes are pruned against it.
fn consider_entry(
    entry: &Entry,
    point: Vec3,
    hit_normal: Vec3,
    gather: u32,
    best: &mut Option<(u32, f32)>,
    bound: &mut f32,
) {
    if entry.index == gather {
        return;
    }
    if entry.normal.dot(hit_normal) < ALIGN {
        return;
    }
    if (point - entry.position).dot(entry.normal).abs() > PLANE_EPS {
        return;
    }
    let distance = point.distance_squared(entry.position);
    if distance > SEARCH * SEARCH {
        return;
    }
    let replace = match *best {
        Some((best_index, best_distance)) => {
            distance < best_distance || (distance == best_distance && entry.index < best_index)
        }
        None => true,
    };
    if replace {
        *best = Some((entry.index, distance));
        *bound = distance;
    }
}

/// Reference predicate: the nearest wall luxel, other than `gather`, whose normal
/// is within 60° of the hit normal and whose plane passes within `PLANE_EPS` of
/// the hit point, no farther than `limit2` (squared). Ties keep the smaller index.
#[cfg(test)]
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
    if normal.dot(hit_normal) < ALIGN {
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
                SEARCH * SEARCH,
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

    /// Tilted normals, scattered walls, hits far from every wall: the tree must
    /// still answer exactly what the scan answers.
    #[test]
    fn index_matches_the_scan_on_scattered_tilted_walls() {
        let mut seed = 0x9e37_79b9u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut receivers = Vec::new();
        for cluster in 0..40 {
            let center = Vec3::new(
                next() * 6000.0 - 3000.0,
                next() * 6000.0 - 3000.0,
                next() * 400.0,
            );
            let tilt = (cluster % 5) as f32 * 0.15;
            let base = Vec3::new((cluster as f32).cos(), (cluster as f32).sin(), tilt).normalize();
            for luxel in 0..30 {
                let jitter = if cluster % 3 == 0 {
                    Vec3::new(next() - 0.5, next() - 0.5, next() - 0.5) * 0.3
                } else {
                    Vec3::ZERO
                };
                let normal = (base + jitter).normalize();
                let along = Vec3::Z.cross(normal).normalize_or_zero();
                let position =
                    center + along * (luxel as f32 * 16.0) + Vec3::Z * ((luxel % 4) as f32 * 16.0);
                receivers.push(Receiver {
                    position,
                    normal,
                    albedo: Vec3::splat(0.5),
                    role: if luxel % 7 == 0 {
                        Role::Floor
                    } else {
                        Role::Wall
                    },
                });
            }
        }
        let index = WallIndex::build(&receivers);
        // Hits on the walls themselves, beside them, and in the open.
        for probe in 0..2000 {
            let (point, normal) = if probe % 2 == 0 {
                let wall = &receivers[(probe * 37) % receivers.len()];
                let shift = Vec3::new(next() - 0.5, next() - 0.5, next() - 0.5) * 3.0;
                (
                    wall.position + shift,
                    (wall.normal + shift * 0.1).normalize(),
                )
            } else {
                (
                    Vec3::new(
                        next() * 8000.0 - 4000.0,
                        next() * 8000.0 - 4000.0,
                        next() * 600.0,
                    ),
                    Vec3::new(next() - 0.5, next() - 0.5, next() - 0.5).normalize(),
                )
            };
            let gather = probe % receivers.len();
            assert_eq!(
                index.nearest(&receivers, point, normal, gather),
                scan(&receivers, point, normal, gather),
                "probe {probe} at {point}"
            );
        }
    }
}
