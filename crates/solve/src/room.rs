use glam::Vec3;

use crate::geom::{Receiver, Rectangle, Triangle};

#[cfg(test)]
use crate::light::{solve, unoccluded_intensity};

const ROOM_SIZE: f32 = 256.0;
const ROOM_HEIGHT: f32 = 128.0;
const LUXEL: f32 = 16.0;

#[derive(Clone, Copy, Debug)]
pub struct Luxel {
    pub receiver: Receiver,
    pub corners: [Vec3; 4],
}

pub struct Room {
    pub triangles: Vec<Triangle>,
    pub luxels: Vec<Luxel>,
    pub area: Rectangle,
    pub panel: Vec<Triangle>,
}

pub fn room() -> Room {
    let mut triangles = Vec::new();
    let mut luxels = Vec::new();

    add_face(
        &mut triangles,
        &mut luxels,
        Vec3::ZERO,
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        Vec3::Z,
        true,
    );
    add_face(
        &mut triangles,
        &mut luxels,
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        -Vec3::Z,
        false,
    );
    add_face(
        &mut triangles,
        &mut luxels,
        Vec3::ZERO,
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        Vec3::Y,
        true,
    );
    add_face(
        &mut triangles,
        &mut luxels,
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        -Vec3::Y,
        true,
    );
    add_face(
        &mut triangles,
        &mut luxels,
        Vec3::ZERO,
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        Vec3::X,
        true,
    );
    add_face(
        &mut triangles,
        &mut luxels,
        Vec3::new(ROOM_SIZE, 0.0, 0.0),
        Vec3::new(0.0, ROOM_SIZE, 0.0),
        Vec3::new(0.0, 0.0, ROOM_HEIGHT),
        -Vec3::X,
        true,
    );

    let panel = panel();
    triangles.extend(panel.iter().copied());

    Room {
        triangles,
        luxels,
        area: Rectangle {
            center: Vec3::new(80.0, 128.0, 118.0),
            half_u: Vec3::new(36.0, 0.0, 0.0),
            half_v: Vec3::new(0.0, 8.0, 0.0),
            normal: -Vec3::Z,
            intensity: 18_000.0,
        },
        panel,
    }
}

fn panel() -> Vec<Triangle> {
    let a = Vec3::new(168.0, 88.0, 0.0);
    let b = Vec3::new(168.0, 168.0, 0.0);
    let c = Vec3::new(168.0, 168.0, 48.0);
    let d = Vec3::new(168.0, 88.0, 48.0);
    vec![
        Triangle {
            vertices: [a, b, c],
        },
        Triangle {
            vertices: [a, c, d],
        },
    ]
}

fn add_face(
    triangles: &mut Vec<Triangle>,
    luxels: &mut Vec<Luxel>,
    origin: Vec3,
    axis_u: Vec3,
    axis_v: Vec3,
    normal: Vec3,
    emit_luxels: bool,
) {
    triangles.push(Triangle {
        vertices: [origin, origin + axis_u, origin + axis_u + axis_v],
    });
    triangles.push(Triangle {
        vertices: [origin, origin + axis_u + axis_v, origin + axis_v],
    });
    if !emit_luxels {
        return;
    }

    let count_u = (axis_u.length() / LUXEL).round().max(1.0) as i32;
    let count_v = (axis_v.length() / LUXEL).round().max(1.0) as i32;
    let step_u = axis_u / count_u as f32;
    let step_v = axis_v / count_v as f32;
    for j in 0..count_v {
        for i in 0..count_u {
            let corner = origin + step_u * i as f32 + step_v * j as f32;
            luxels.push(Luxel {
                receiver: Receiver {
                    position: corner + step_u * 0.5 + step_v * 0.5,
                    normal,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_has_open_shadow_and_penumbra() {
        let room = room();
        let floor: Vec<_> = room
            .luxels
            .iter()
            .filter(|luxel| luxel.receiver.normal.z > 0.9)
            .copied()
            .collect();
        let receivers: Vec<_> = floor.iter().map(|luxel| luxel.receiver).collect();
        for rays in [16, 64] {
            let colors = solve(&room.triangles, &receivers, &room.area, rays);
            let mut open = false;
            let mut closed = false;
            let mut partial = false;
            for (luxel, color) in floor.iter().zip(colors) {
                let expected = unoccluded_intensity(&room.area, luxel.receiver.position);
                let visibility = color[0] / expected;
                if visibility > 0.97 {
                    open = true;
                } else if visibility < 0.05 {
                    closed = true;
                } else if (0.15..0.85).contains(&visibility) {
                    partial = true;
                }
            }
            assert!(open, "no fully lit floor luxel at {rays} rays");
            assert!(closed, "no fully shadowed floor luxel at {rays} rays");
            assert!(partial, "no penumbra on the floor at {rays} rays");
        }
    }
}
