use glam::Vec3;

use crate::geom::{Area, Receiver, Rectangle, Role, Triangle};

#[cfg(test)]
use crate::light::{solve, stratum_point, unoccluded_intensity, RAY_LIFT};

const ROOM_SIZE: f32 = 256.0;
const ROOM_HEIGHT: f32 = 128.0;
const LUXEL: f32 = 16.0;
const PARTITION_X: f32 = 208.0;
const PARTITION_TOP: f32 = 96.0;
const GRATE_X: f32 = 150.0;
const GRATE_TOP: f32 = 96.0;
const SHELL_MIN: Vec3 = Vec3::new(16.0, 16.0, 0.0);
const SHELL_MAX: Vec3 = Vec3::new(64.0, 64.0, 48.0);
const WALL_ALBEDO: Vec3 = Vec3::new(0.66, 0.6, 0.52);
const FLOOR_ALBEDO: Vec3 = Vec3::new(0.3, 0.28, 0.26);
const FACE_BIAS: f32 = 0.35;

#[derive(Clone, Copy, Debug)]
pub struct Luxel {
    pub receiver: Receiver,
    pub corners: [Vec3; 4],
}

pub struct Room {
    pub triangles: Vec<Triangle>,
    pub luxels: Vec<Luxel>,
    pub areas: Vec<Area>,
    pub grate: Vec<Triangle>,
    pub shell: Vec<Triangle>,
}

pub fn room() -> Room {
    let mut triangles = Vec::new();
    let mut luxels = Vec::new();

    push_quad(
        &mut triangles,
        Vec3::ZERO,
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, ROOM_SIZE, 0.0),
    );
    push_luxels(
        &mut luxels,
        Vec3::ZERO,
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        Vec3::Z,
        Role::Floor,
        FLOOR_ALBEDO,
        0.0,
    );
    push_quad(
        &mut triangles,
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, ROOM_SIZE, 0.0),
    );
    push_wall(
        &mut triangles,
        &mut luxels,
        Vec3::ZERO,
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        Vec3::Y,
    );
    push_wall(
        &mut triangles,
        &mut luxels,
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        -Vec3::Y,
    );
    push_wall(
        &mut triangles,
        &mut luxels,
        Vec3::ZERO,
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        Vec3::X,
    );
    push_wall(
        &mut triangles,
        &mut luxels,
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        -Vec3::X,
    );

    let partition = Vec3::new(PARTITION_X, 0.0, 0.0);
    let along = Vec3::new(0.0, ROOM_SIZE, 0.0);
    let up = Vec3::new(0.0, 0.0, PARTITION_TOP);
    push_quad(&mut triangles, partition, along, up);
    push_luxels(
        &mut luxels,
        partition,
        along,
        up,
        -Vec3::X,
        Role::Wall,
        WALL_ALBEDO,
        FACE_BIAS,
    );
    push_luxels(
        &mut luxels,
        partition,
        along,
        up,
        Vec3::X,
        Role::Wall,
        WALL_ALBEDO,
        FACE_BIAS,
    );

    let grate = grate();
    let shell = closed_box(SHELL_MIN, SHELL_MAX);
    triangles.extend(grate.iter().copied());
    triangles.extend(shell.iter().copied());

    Room {
        triangles,
        luxels,
        areas: vec![
            Area::Rectangle(Rectangle {
                center: Vec3::new(80.0, 128.0, 118.0),
                half_u: Vec3::new(36.0, 0.0, 0.0),
                half_v: Vec3::new(0.0, 8.0, 0.0),
                normal: -Vec3::Z,
                intensity: 18_000.0,
                color: Vec3::ONE,
            }),
            Area::Rectangle(Rectangle {
                center: Vec3::new(38.0, 42.0, 34.0),
                half_u: Vec3::new(8.0, 0.0, 0.0),
                half_v: Vec3::new(0.0, 6.0, 0.0),
                normal: -Vec3::Z,
                intensity: 18_000.0,
                color: Vec3::ONE,
            }),
        ],
        grate,
        shell,
    }
}

fn grate() -> Vec<Triangle> {
    let mut bars = Vec::new();
    for (y0, y1) in [(40.0, 72.0), (112.0, 148.0), (168.0, 204.0), (220.0, 248.0)] {
        bars.extend(vertical_strip(GRATE_X, y0, y1, 0.0, GRATE_TOP));
    }
    bars
}

fn vertical_strip(x: f32, y0: f32, y1: f32, z0: f32, z1: f32) -> [Triangle; 2] {
    let origin = Vec3::new(x, y0, z0);
    let along = Vec3::new(0.0, y1 - y0, 0.0);
    let up = Vec3::new(0.0, 0.0, z1 - z0);
    [
        Triangle {
            vertices: [origin, origin + along, origin + along + up],
        },
        Triangle {
            vertices: [origin, origin + along + up, origin + up],
        },
    ]
}

fn closed_box(min: Vec3, max: Vec3) -> Vec<Triangle> {
    let p = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    let quads = [
        [0, 1, 2, 3],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [3, 2, 6, 7],
        [0, 3, 7, 4],
        [1, 2, 6, 5],
    ];
    let mut triangles = Vec::with_capacity(12);
    for quad in quads {
        triangles.push(Triangle {
            vertices: [p[quad[0]], p[quad[1]], p[quad[2]]],
        });
        triangles.push(Triangle {
            vertices: [p[quad[0]], p[quad[2]], p[quad[3]]],
        });
    }
    triangles
}

fn push_wall(
    triangles: &mut Vec<Triangle>,
    luxels: &mut Vec<Luxel>,
    origin: Vec3,
    axis_u: Vec3,
    axis_v: Vec3,
    normal: Vec3,
) {
    push_quad(triangles, origin, axis_u, axis_v);
    push_luxels(
        luxels,
        origin,
        axis_u,
        axis_v,
        normal,
        Role::Wall,
        WALL_ALBEDO,
        0.0,
    );
}

fn push_quad(triangles: &mut Vec<Triangle>, origin: Vec3, axis_u: Vec3, axis_v: Vec3) {
    triangles.push(Triangle {
        vertices: [origin, origin + axis_u, origin + axis_u + axis_v],
    });
    triangles.push(Triangle {
        vertices: [origin, origin + axis_u + axis_v, origin + axis_v],
    });
}

fn push_luxels(
    luxels: &mut Vec<Luxel>,
    origin: Vec3,
    axis_u: Vec3,
    axis_v: Vec3,
    normal: Vec3,
    role: Role,
    albedo: Vec3,
    bias: f32,
) {
    let count_u = (axis_u.length() / LUXEL).round().max(1.0) as i32;
    let count_v = (axis_v.length() / LUXEL).round().max(1.0) as i32;
    let step_u = axis_u / count_u as f32;
    let step_v = axis_v / count_v as f32;
    let shift = normal.normalize_or_zero() * bias;
    for j in 0..count_v {
        for i in 0..count_u {
            let corner = origin + step_u * i as f32 + step_v * j as f32 + shift;
            let position = corner + step_u * 0.5 + step_v * 0.5;
            if role == Role::Floor && in_shell_footprint(position) {
                continue;
            }
            luxels.push(Luxel {
                receiver: Receiver {
                    position,
                    normal,
                    albedo,
                    role,
                },
                corners: [
                    corner,
                    corner + step_u,
                    corner + step_u + step_v,
                    corner + step_v,
                ],
            });
        }
    }
}

fn in_shell_footprint(point: Vec3) -> bool {
    point.x > SHELL_MIN.x && point.x < SHELL_MAX.x && point.y > SHELL_MIN.y && point.y < SHELL_MAX.y
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAYS: u32 = 16;

    fn receivers_of(room: &Room) -> Vec<Receiver> {
        room.luxels.iter().map(|luxel| luxel.receiver).collect()
    }

    fn without_wall_albedo(room: &Room) -> Vec<Receiver> {
        receivers_of(room)
            .into_iter()
            .map(|mut receiver| {
                if receiver.role == Role::Wall {
                    receiver.albedo = Vec3::ZERO;
                }
                receiver
            })
            .collect()
    }

    fn luma(color: [f32; 3]) -> f32 {
        color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722
    }

    fn segment_hits(from: Vec3, to: Vec3, triangle: &Triangle) -> bool {
        let edge = to - from;
        let length = edge.length();
        if length <= HIT_MARGIN {
            return false;
        }
        let direction = edge / length;
        let v0 = triangle.vertices[0];
        let edge_u = triangle.vertices[1] - v0;
        let edge_v = triangle.vertices[2] - v0;
        let p = direction.cross(edge_v);
        let det = edge_u.dot(p);
        if det.abs() < 1.0e-5 {
            return false;
        }
        let inv = 1.0 / det;
        let offset = from - v0;
        let u = offset.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return false;
        }
        let q = offset.cross(edge_u);
        let v = direction.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return false;
        }
        let t = edge_v.dot(q) * inv;
        t > HIT_MARGIN && t < length - HIT_MARGIN
    }

    const HIT_MARGIN: f32 = 1.0e-3;

    fn grate_classes(room: &Room) -> (Vec<usize>, Vec<usize>) {
        let mut gaps = Vec::new();
        let mut bars = Vec::new();
        let area = &room.areas[0];
        for (index, luxel) in room.luxels.iter().enumerate() {
            if luxel.receiver.role != Role::Floor {
                continue;
            }
            let point = luxel.receiver.position;
            if point.x <= GRATE_X + 4.0 || point.x >= PARTITION_X - 4.0 {
                continue;
            }
            let origin = point + luxel.receiver.normal.normalize_or_zero() * RAY_LIFT;
            let mut clear = 0u32;
            for sample in 0..RAYS {
                let target = stratum_point(area, sample, RAYS);
                let blocked = room
                    .grate
                    .iter()
                    .any(|triangle| segment_hits(origin, target, triangle));
                if !blocked {
                    clear += 1;
                }
            }
            let visibility = clear as f32 / RAYS as f32;
            if visibility > 0.95 {
                gaps.push(index);
            } else if visibility < 0.05 {
                bars.push(index);
            }
        }
        (gaps, bars)
    }

    #[test]
    fn grate_gap_is_lit_and_bars_are_dark() {
        let room = room();
        let (gaps, bars) = grate_classes(&room);
        assert!(
            !gaps.is_empty() && !bars.is_empty(),
            "gap luxels {}, bar luxels {}",
            gaps.len(),
            bars.len()
        );

        let dead = solve(
            &room.triangles,
            &without_wall_albedo(&room),
            &room.areas,
            RAYS,
        );
        for index in &bars {
            assert_eq!(dead.light[*index], [0.0, 0.0, 0.0], "bar luxel {index}");
        }
        for index in &gaps {
            let expected =
                unoccluded_intensity(&room.areas[0], room.luxels[*index].receiver.position);
            let visibility = dead.light[*index][0] / expected;
            assert!(
                visibility > 0.95,
                "gap luxel {index} visibility {visibility}"
            );
        }

        let live = solve(&room.triangles, &receivers_of(&room), &room.areas, RAYS);
        let gap_luma = gaps
            .iter()
            .map(|index| luma(live.light[*index]))
            .sum::<f32>()
            / gaps.len() as f32;
        let bar_luma = bars
            .iter()
            .map(|index| luma(live.light[*index]))
            .sum::<f32>()
            / bars.len() as f32;
        assert!(gap_luma > bar_luma * 2.0, "gap {gap_luma}, bars {bar_luma}");
    }

    #[test]
    fn open_floor_falls_off_with_distance() {
        let room = room();
        let dead = solve(
            &room.triangles,
            &without_wall_albedo(&room),
            &room.areas,
            RAYS,
        );
        let color_at = |x: f32, y: f32| {
            let index = room
                .luxels
                .iter()
                .position(|luxel| {
                    let point = luxel.receiver.position;
                    (point.x - x).abs() < 0.1 && (point.y - y).abs() < 0.1
                })
                .expect("luxel");
            dead.light[index][0]
        };
        let near = color_at(88.0, 136.0);
        let far = color_at(136.0, 200.0);
        assert!(near > far, "near {near}, far {far}");
    }

    #[test]
    fn floor_behind_the_wall_is_not_black() {
        let room = room();
        let behind: Vec<usize> = room
            .luxels
            .iter()
            .enumerate()
            .filter(|(_, luxel)| {
                luxel.receiver.role == Role::Floor && luxel.receiver.position.x > PARTITION_X
            })
            .map(|(index, _)| index)
            .collect();
        assert!(behind.len() > 8);

        let dead = solve(
            &room.triangles,
            &without_wall_albedo(&room),
            &room.areas,
            RAYS,
        );
        for index in &behind {
            assert_eq!(
                dead.light[*index],
                [0.0, 0.0, 0.0],
                "direct leak at {}",
                room.luxels[*index].receiver.position
            );
        }

        let live = solve(&room.triangles, &receivers_of(&room), &room.areas, RAYS);
        let behind_luma = behind
            .iter()
            .map(|index| luma(live.light[*index]))
            .sum::<f32>()
            / behind.len() as f32;
        let open = luma(
            live.light[room
                .luxels
                .iter()
                .position(|luxel| {
                    let point = luxel.receiver.position;
                    (point.x - 88.0).abs() < 0.1 && (point.y - 136.0).abs() < 0.1
                })
                .expect("open luxel")],
        );
        assert!(behind_luma > 0.02, "floor behind the wall is {behind_luma}");
        assert!(
            behind_luma < open * 0.5,
            "behind {behind_luma}, open {open}"
        );
    }

    #[test]
    fn sealed_lamp_does_not_change_luxels() {
        let room = room();
        let mut receivers = receivers_of(&room);
        let lamp = room.areas[1].center();
        receivers.push(Receiver {
            position: Vec3::new(lamp.x, lamp.y, lamp.z - 20.0),
            normal: Vec3::Z,
            albedo: Vec3::ZERO,
            role: Role::Other,
        });
        let with = solve(&room.triangles, &receivers, &room.areas, RAYS);
        let without = solve(&room.triangles, &receivers, &room.areas[..1], RAYS);
        let full = solve(&room.triangles, &receivers, &room.areas, 64);
        assert_eq!(with.sealed, vec![1]);
        assert_eq!(full.sealed, with.sealed);
        assert!(!with.sealed.contains(&0));
        assert_eq!(
            with.light[..receivers.len() - 1],
            without.light[..receivers.len() - 1]
        );
        assert_eq!(*with.light.last().expect("insider"), [0.0, 0.0, 0.0]);
        let again = solve(&room.triangles, &receivers, &room.areas, RAYS);
        assert_eq!(with, again);
    }
}
