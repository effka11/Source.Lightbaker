//! Nearest wall luxel for a bounce hit.
//!
//! The mock room is small enough that a full scan is the reference. A map has
//! hundreds of thousands of walls and tens of millions of hits, two thirds of
//! which land where no coplanar wall luxel exists at all. The same predicate is
//! answered from a tree. Walls are first dealt into six piles by the axis
//! their normal leans toward, so a hit on a floor or a ceiling never opens the
//! piles of vertical walls. Each pile is a tree over positions whose nodes
//! also carry the box of their normals: a node is pruned when no normal in it
//! can be aligned with the hit, when no plane through its box can pass near
//! the hit point, or when its box is farther than the best luxel so far. A
//! miss then costs a walk down one small tree, not a scan of the
//! neighbourhood. Ties keep the smaller receiver index.

use glam::Vec3;

use crate::geom::{Receiver, Role};

const SEARCH: f32 = 768.0;
/// A candidate's normal must lie within 60° of the hit normal.
const ALIGN: f32 = 0.5;
const PLANE_EPS: f32 = 1.5;
/// Rounding slack on the plane offset bound, in world units.
const PLANE_SLACK: f32 = 0.1;
/// Rounding slack on the alignment bound.
const ALIGN_SLACK: f32 = 1.0e-4;
const LEAF: usize = 8;
const NO_CHILD: u32 = u32::MAX;

pub(crate) struct WallIndex {
    /// Wall receivers in leaf order.
    entries: Vec<Entry>,
    nodes: Vec<Node>,
    /// Subtree roots: one per pile of normals that lean the same way.
    roots: Vec<u32>,
    /// Root of each pile by its number, `NO_CHILD` for an empty pile.
    pile: [u32; 6],
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
    /// Box holding every normal below.
    normal_min: Vec3,
    normal_max: Vec3,
    /// Leaf: `[first, first + count)` in `entries`. Inner: children.
    first: u32,
    count: u32,
    left: u32,
    right: u32,
}

impl WallIndex {
    pub(crate) fn build(receivers: &[Receiver]) -> Self {
        let entries: Vec<Entry> = receivers
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
            pile: [NO_CHILD; 6],
        };
        if entries.is_empty() {
            return index;
        }

        // One pile per leaning. The pile the hit itself leans toward is searched
        // first; a pile whose normals cannot face the hit closes at its root.
        let mut piles: Vec<Vec<Entry>> = vec![Vec::new(); 6];
        for entry in entries {
            piles[leaning(entry.normal)].push(entry);
        }
        let mut ordered = Vec::new();
        for (number, pile) in piles.into_iter().enumerate() {
            if pile.is_empty() {
                continue;
            }
            let first = ordered.len();
            ordered.extend_from_slice(&pile);
            let root = index.split(&mut ordered, first, pile.len());
            index.roots.push(root);
            index.pile[number] = root;
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
        // The pile the hit normal itself leans toward holds the coplanar
        // luxels. Searching it first shrinks the bound before the other piles
        // are opened, so most of them close on distance alone.
        let own = self.pile[leaning(hit_normal)];
        if own != NO_CHILD {
            self.descend(own, point, hit_normal, gather as u32, &mut best, &mut bound);
        }
        for &root in &self.roots {
            if root == own {
                continue;
            }
            self.descend(
                root,
                point,
                hit_normal,
                gather as u32,
                &mut best,
                &mut bound,
            );
        }
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
        if box_distance_squared(node.min, node.max, point) > *bound {
            return;
        }
        if !node.may_align(hit_normal) {
            return;
        }
        if !node.may_be_coplanar(point) {
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
        let mut normal_min = Vec3::splat(f32::MAX);
        let mut normal_max = Vec3::splat(f32::MIN);
        for entry in entries {
            min = min.min(entry.position);
            max = max.max(entry.position);
            normal_min = normal_min.min(entry.normal);
            normal_max = normal_max.max(entry.normal);
        }
        Self {
            min,
            max,
            normal_min,
            normal_max,
            first,
            count,
            left: NO_CHILD,
            right: NO_CHILD,
        }
    }

    /// Some normal in the box may lie within 60° of the hit normal. The largest
    /// dot product over a box is reached at one of its corners, one axis at a
    /// time, so this is the same arithmetic as the entry test, only on bounds.
    fn may_align(&self, hit_normal: Vec3) -> bool {
        let most = (self.normal_min.x * hit_normal.x).max(self.normal_max.x * hit_normal.x)
            + (self.normal_min.y * hit_normal.y).max(self.normal_max.y * hit_normal.y)
            + (self.normal_min.z * hit_normal.z).max(self.normal_max.z * hit_normal.z);
        most >= ALIGN - ALIGN_SLACK
    }

    /// Some plane through a position in the box, with a normal from the normal
    /// box, may pass within `PLANE_EPS` of the point. The offset
    /// `(point - position) · normal` is a sum of per-axis products, and each
    /// product ranges over the corners of its own rectangle of bounds.
    fn may_be_coplanar(&self, point: Vec3) -> bool {
        let mut lo = 0.0f32;
        let mut hi = 0.0f32;
        for axis in 0..3 {
            let near = point[axis] - self.max[axis];
            let far = point[axis] - self.min[axis];
            let n_lo = self.normal_min[axis];
            let n_hi = self.normal_max[axis];
            let corners = [near * n_lo, near * n_hi, far * n_lo, far * n_hi];
            lo += corners.iter().copied().fold(f32::MAX, f32::min);
            hi += corners.iter().copied().fold(f32::MIN, f32::max);
        }
        lo <= PLANE_EPS + PLANE_SLACK && hi >= -(PLANE_EPS + PLANE_SLACK)
    }
}

/// Which axis a normal leans toward, as a pile number. The first axis whose
/// component reaches `ALIGN` wins, so every unit normal lands in some pile.
fn leaning(normal: Vec3) -> usize {
    if normal.z >= ALIGN {
        0
    } else if normal.z <= -ALIGN {
        1
    } else if normal.x >= ALIGN {
        2
    } else if normal.x <= -ALIGN {
        3
    } else if normal.y >= ALIGN {
        4
    } else {
        5
    }
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
