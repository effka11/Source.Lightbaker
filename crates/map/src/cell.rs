//! One luxel's share of its face, in lightmap coordinates.
//!
//! `Outline::cell` takes the texel center. That center is half a luxel past
//! the integer grid point, and the luxel owns the unit square around it — the
//! square the engine draws for that texel. Light is gathered where the square
//! meets the face, so a square cut by a wall never samples inside the wall. A
//! square that misses the face borrows the nearest point inside it and has no
//! outline to draw.

use glam::Vec2;

const HALF: f32 = 0.5;
/// Half the cell diagonal: past this distance from an edge the cell is whole.
const REACH: f32 = std::f32::consts::SQRT_2 * HALF;
const AREA_EPS: f32 = 1.0e-4;
/// How far a borrowed sample steps in from the edge, in luxels.
const INSET: f32 = 0.1;

/// Convex face outline in lightmap coordinates.
pub struct Outline {
    points: Vec<Vec2>,
    /// Inward unit normal and offset per edge: inside when `n · p + d >= 0`.
    edges: Vec<(Vec2, f32)>,
    centroid: Vec2,
}

/// Where one luxel gathers light and what part of the face it covers.
pub struct Cell {
    pub sample: Vec2,
    /// Convex piece of the face inside the cell; empty when they do not meet.
    pub piece: Vec<Vec2>,
}

impl Outline {
    /// `None` for a degenerate outline.
    pub fn new(points: &[Vec2]) -> Option<Self> {
        if points.len() < 3 {
            return None;
        }
        let signed = signed_area(points);
        if signed.abs() <= AREA_EPS || !signed.is_finite() {
            return None;
        }
        let mut points = points.to_vec();
        if signed < 0.0 {
            points.reverse();
        }
        let mut edges = Vec::with_capacity(points.len());
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            let along = b - a;
            let length = along.length();
            if length <= 1.0e-6 {
                continue;
            }
            let normal = Vec2::new(-along.y, along.x) / length;
            edges.push((normal, -normal.dot(a)));
        }
        let centroid = centroid(&points)?;
        Some(Self {
            points,
            edges,
            centroid,
        })
    }

    pub fn cell(&self, center: Vec2) -> Cell {
        let mut depth = f32::MAX;
        for (normal, offset) in &self.edges {
            depth = depth.min(normal.dot(center) + offset);
        }
        if depth >= REACH {
            return Cell {
                sample: center,
                piece: square(center),
            };
        }
        if depth > -REACH {
            let piece = self.clipped(center);
            if let Some(sample) = centroid(&piece) {
                return Cell { sample, piece };
            }
        }
        Cell {
            sample: self.borrowed(center),
            piece: Vec::new(),
        }
    }

    fn clipped(&self, center: Vec2) -> Vec<Vec2> {
        let planes = [
            (Vec2::X, -(center.x - HALF)),
            (Vec2::NEG_X, center.x + HALF),
            (Vec2::Y, -(center.y - HALF)),
            (Vec2::NEG_Y, center.y + HALF),
        ];
        let mut piece = self.points.clone();
        for (normal, offset) in planes {
            piece = clip(&piece, normal, offset);
            if piece.len() < 3 {
                return Vec::new();
            }
        }
        piece
    }

    /// Nearest point of the face, stepped in toward its centroid.
    fn borrowed(&self, point: Vec2) -> Vec2 {
        let mut best = self.centroid;
        let mut best_distance = f32::MAX;
        for index in 0..self.points.len() {
            let a = self.points[index];
            let b = self.points[(index + 1) % self.points.len()];
            let candidate = nearest_on_segment(a, b, point);
            let distance = candidate.distance_squared(point);
            if distance < best_distance {
                best_distance = distance;
                best = candidate;
            }
        }
        let inward = self.centroid - best;
        let length = inward.length();
        if length <= 1.0e-6 {
            return best;
        }
        best + inward / length * INSET.min(length * 0.5)
    }
}

fn square(center: Vec2) -> Vec<Vec2> {
    vec![
        center + Vec2::new(-HALF, -HALF),
        center + Vec2::new(HALF, -HALF),
        center + Vec2::new(HALF, HALF),
        center + Vec2::new(-HALF, HALF),
    ]
}

/// Sutherland–Hodgman against one half-plane: keeps `n · p + d >= 0`.
fn clip(points: &[Vec2], normal: Vec2, offset: f32) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(points.len() + 2);
    let mut push = |point: Vec2| {
        if out
            .last()
            .is_none_or(|last| last.distance_squared(point) > 1.0e-10)
        {
            out.push(point);
        }
    };
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        let da = normal.dot(a) + offset;
        let db = normal.dot(b) + offset;
        if da >= 0.0 {
            push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let along = da / (da - db);
            push(a + (b - a) * along);
        }
    }
    if out.len() > 1 && out[0].distance_squared(out[out.len() - 1]) <= 1.0e-10 {
        out.pop();
    }
    out
}

fn signed_area(points: &[Vec2]) -> f32 {
    let mut sum = 0.0;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        sum += a.x * b.y - b.x * a.y;
    }
    sum * 0.5
}

/// Area centroid; `None` when the polygon has no area.
fn centroid(points: &[Vec2]) -> Option<Vec2> {
    if points.len() < 3 {
        return None;
    }
    let mut area = 0.0;
    let mut sum = Vec2::ZERO;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        let cross = a.x * b.y - b.x * a.y;
        area += cross;
        sum += (a + b) * cross;
    }
    if area.abs() <= AREA_EPS || !area.is_finite() {
        return None;
    }
    Some(sum / (3.0 * area))
}

fn nearest_on_segment(a: Vec2, b: Vec2, point: Vec2) -> Vec2 {
    let along = b - a;
    let length_squared = along.length_squared();
    if length_squared <= 1.0e-12 {
        return a;
    }
    let factor = ((point - a).dot(along) / length_squared).clamp(0.0, 1.0);
    a + along * factor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area_of(points: &[Vec2]) -> f32 {
        signed_area(points).abs()
    }

    fn inside(outline: &[Vec2], point: Vec2, slack: f32) -> bool {
        Outline::new(outline)
            .unwrap()
            .edges
            .iter()
            .all(|(normal, offset)| normal.dot(point) + offset >= -slack)
    }

    #[test]
    fn a_whole_cell_samples_its_center() {
        let outline = Outline::new(&[
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(0.0, 4.0),
        ])
        .unwrap();
        let cell = outline.cell(Vec2::new(2.0, 2.0));
        assert_eq!(cell.sample, Vec2::new(2.0, 2.0));
        assert_eq!(cell.piece.len(), 4);
        assert!((area_of(&cell.piece) - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn a_cell_on_the_edge_keeps_only_the_face_side() {
        let outline = Outline::new(&[
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(0.0, 4.0),
        ])
        .unwrap();
        let cell = outline.cell(Vec2::new(0.0, 2.0));
        assert!((cell.sample.x - 0.25).abs() < 1.0e-5, "{}", cell.sample);
        assert!((cell.sample.y - 2.0).abs() < 1.0e-5, "{}", cell.sample);
        assert!((area_of(&cell.piece) - 0.5).abs() < 1.0e-5);
        assert!(cell.piece.iter().all(|point| point.x >= -1.0e-6));
    }

    #[test]
    fn a_diagonal_cut_leaves_a_triangle_sampled_inside() {
        // Right triangle: the hypotenuse is the wall the floor ends at.
        let outline = [
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(0.0, 4.0),
        ];
        let cell = Outline::new(&outline).unwrap().cell(Vec2::new(2.0, 2.0));
        assert!(cell.piece.len() >= 3, "{:?}", cell.piece);
        assert!((area_of(&cell.piece) - 0.5).abs() < 1.0e-5);
        assert!(cell
            .piece
            .iter()
            .all(|point| inside(&outline, *point, 1.0e-6)));
        assert!(inside(&outline, cell.sample, 0.0), "{}", cell.sample);
        assert!(cell.sample.x + cell.sample.y < 4.0 - 0.3);
    }

    #[test]
    fn a_cell_off_the_face_borrows_a_point_inside_and_draws_nothing() {
        let outline = [
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(0.0, 4.0),
        ];
        let cell = Outline::new(&outline).unwrap().cell(Vec2::new(4.0, 4.0));
        assert!(cell.piece.is_empty());
        assert!(inside(&outline, cell.sample, 0.0), "{}", cell.sample);
        assert!(
            cell.sample.distance(Vec2::new(2.0, 2.0)) < 0.2,
            "{}",
            cell.sample
        );
    }

    #[test]
    fn cells_tile_the_face_exactly_whatever_the_winding() {
        let ccw = [
            Vec2::new(0.3, 0.2),
            Vec2::new(5.1, 0.9),
            Vec2::new(3.2, 4.6),
            Vec2::new(-0.4, 2.8),
        ];
        let mut cw = ccw;
        cw.reverse();
        for outline in [ccw, cw] {
            let shape = Outline::new(&outline).unwrap();
            let mut covered = 0.0;
            for t in -1..6 {
                for s in -1..7 {
                    let cell = shape.cell(Vec2::new(s as f32, t as f32));
                    covered += area_of(&cell.piece);
                    assert!(inside(&outline, cell.sample, 1.0e-4), "{}", cell.sample);
                    for point in &cell.piece {
                        assert!(inside(&outline, *point, 1.0e-4), "{point}");
                    }
                }
            }
            assert!((covered - area_of(&outline)).abs() < 1.0e-3, "{covered}");
        }
    }

    #[test]
    fn a_flat_outline_is_refused() {
        assert!(Outline::new(&[Vec2::ZERO, Vec2::X, Vec2::new(2.0, 0.0)]).is_none());
        assert!(Outline::new(&[Vec2::ZERO, Vec2::X]).is_none());
    }
}
