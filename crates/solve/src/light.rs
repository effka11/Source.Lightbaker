use glam::Vec3;
use rayon::prelude::*;

use crate::embree::Scene;
use crate::geom::{Area, Receiver, Role, Triangle};
use crate::shell::sealed_areas;

const MIN_DISTANCE: f32 = 1.0e-2;
pub(crate) const RAY_LIFT: f32 = 0.5;
const HIT_SLOP: f32 = 1.0e-3;
const PLANE_EPS: f32 = 1.5;
const BOUNCES: u32 = 2;
const TRACE: f32 = 1.0e6;
/// Floors dimmer than this fraction of the bounce already in the room are lifted.
const LIFT_DARK: f32 = 0.1;
const LIFT_MIX: f32 = 0.7;

#[derive(Clone, Debug, PartialEq)]
pub struct Solved {
    pub light: Vec<[f32; 3]>,
    pub sealed: Vec<usize>,
}

pub fn solve(triangles: &[Triangle], receivers: &[Receiver], areas: &[Area], rays: u32) -> Solved {
    let sealed = sealed_areas(triangles, areas);
    if receivers.is_empty() {
        return Solved {
            light: Vec::new(),
            sealed,
        };
    }

    let open: Vec<&Area> = areas
        .iter()
        .enumerate()
        .filter(|(index, _)| !sealed.contains(index))
        .map(|(_, area)| area)
        .collect();
    if rays == 0 || open.is_empty() {
        return Solved {
            light: vec![[0.0, 0.0, 0.0]; receivers.len()],
            sealed,
        };
    }

    let scene = Scene::build(triangles);
    let direct = direct_light(&scene, receivers, &open, rays);
    let mut light = direct.clone();
    let mut arriving = direct.clone();
    for _ in 0..BOUNCES {
        let bounced = diffuse_bounce(&scene, receivers, &arriving, rays);
        for (slot, add) in light.iter_mut().zip(&bounced) {
            add_color(slot, *add);
        }
        arriving = bounced;
    }
    // Almost-black floors move toward bounce that already landed. No point in the room.
    lift_floors(&mut light, &direct, receivers);
    Solved { light, sealed }
}

pub(crate) fn unoccluded_intensity(area: &Area, position: Vec3) -> f32 {
    let nearest = nearest_point(area, position);
    let distance = (nearest - position).length().max(MIN_DISTANCE);
    area.intensity() / (distance * distance)
}

fn direct_light(
    scene: &Scene,
    receivers: &[Receiver],
    areas: &[&Area],
    rays: u32,
) -> Vec<[f32; 3]> {
    receivers
        .par_iter()
        .map(|receiver| {
            let mut sum = [0.0, 0.0, 0.0];
            for area in areas {
                add_color(&mut sum, direct(scene, receiver, area, rays));
            }
            sum
        })
        .collect()
}

fn direct(scene: &Scene, receiver: &Receiver, area: &Area, rays: u32) -> [f32; 3] {
    let normal = receiver.normal.normalize_or_zero();
    let scale = unoccluded_intensity(area, receiver.position);
    if scale == 0.0 || normal == Vec3::ZERO {
        return [0.0, 0.0, 0.0];
    }

    let origin = receiver.position + normal * RAY_LIFT;
    let mut hits = 0u32;
    for index in 0..rays {
        let sample = stratum_point(area, index, rays);
        if arrives(scene, origin, sample, normal, area.normal()) {
            hits += 1;
        }
    }
    let value = scale * hits as f32 / rays as f32;
    let color = area.color();
    [value * color.x, value * color.y, value * color.z]
}

fn diffuse_bounce(
    scene: &Scene,
    receivers: &[Receiver],
    arriving: &[[f32; 3]],
    rays: u32,
) -> Vec<[f32; 3]> {
    let walls: Vec<usize> = receivers
        .iter()
        .enumerate()
        .filter(|(_, receiver)| receiver.role == Role::Wall)
        .map(|(index, _)| index)
        .collect();
    if walls.is_empty() {
        return vec![[0.0, 0.0, 0.0]; receivers.len()];
    }
    receivers
        .par_iter()
        .enumerate()
        .map(|(index, receiver)| {
            bounce_one(scene, receivers, &walls, arriving, index, receiver, rays)
        })
        .collect()
}

fn bounce_one(
    scene: &Scene,
    receivers: &[Receiver],
    walls: &[usize],
    arriving: &[[f32; 3]],
    gather: usize,
    receiver: &Receiver,
    rays: u32,
) -> [f32; 3] {
    let normal = receiver.normal.normalize_or_zero();
    if normal == Vec3::ZERO || rays == 0 {
        return [0.0, 0.0, 0.0];
    }
    let origin = receiver.position + normal * RAY_LIFT;
    let mut sum = [0.0, 0.0, 0.0];
    for index in 0..rays {
        let direction = hemisphere_dir(normal, index, rays);
        let Some(hit) = scene.hit(origin, direction, TRACE) else {
            continue;
        };
        let Some(wall) = wall_sample(receivers, walls, hit.point, hit.normal, gather) else {
            continue;
        };
        add_color(&mut sum, tint(receivers[wall].albedo, arriving[wall]));
    }
    let scale = rays as f32;
    sum.map(|channel| channel / scale)
}

fn wall_sample(
    receivers: &[Receiver],
    walls: &[usize],
    point: Vec3,
    hit_normal: Vec3,
    gather: usize,
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for &index in walls {
        if index == gather {
            continue;
        }
        let wall = &receivers[index];
        let normal = wall.normal.normalize_or_zero();
        if normal.dot(hit_normal) < 0.5 {
            continue;
        }
        if (point - wall.position).dot(normal).abs() > PLANE_EPS {
            continue;
        }
        let distance = point.distance_squared(wall.position);
        let nearer = match best {
            Some((_, best_distance)) => distance < best_distance,
            None => true,
        };
        if nearer {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

fn lift_floors(light: &mut [[f32; 3]], direct: &[[f32; 3]], receivers: &[Receiver]) {
    let Some(target) = bounce_target(light, direct, receivers) else {
        return;
    };
    let reach = luma(target) * LIFT_DARK;
    if reach <= 1.0e-6 {
        return;
    }
    for (index, receiver) in receivers.iter().enumerate() {
        if receiver.role != Role::Floor {
            continue;
        }
        let current = light[index];
        let brightness = luma(current);
        if brightness >= reach {
            continue;
        }
        let darkness = 1.0 - brightness / reach;
        light[index] = mix(current, target, darkness * LIFT_MIX);
    }
}

fn bounce_target(
    light: &[[f32; 3]],
    direct: &[[f32; 3]],
    receivers: &[Receiver],
) -> Option<[f32; 3]> {
    let mut bounced = Vec::new();
    for index in 0..receivers.len() {
        let color = sub_color(light[index], direct[index]);
        if luma(color) <= 1.0e-5 {
            continue;
        }
        bounced.push(color);
    }
    if bounced.is_empty() {
        return None;
    }
    bounced.sort_by(|left, right| luma(*left).total_cmp(&luma(*right)));
    let upper = &bounced[bounced.len() / 2..];
    let mut sum = [0.0, 0.0, 0.0];
    for color in upper {
        add_color(&mut sum, *color);
    }
    let scale = upper.len() as f32;
    Some(sum.map(|channel| channel / scale))
}

fn nearest_point(area: &Area, point: Vec3) -> Vec3 {
    match area {
        Area::Rectangle(rectangle) => {
            let u_len = rectangle.half_u.length().max(MIN_DISTANCE);
            let v_len = rectangle.half_v.length().max(MIN_DISTANCE);
            let u_axis = rectangle.half_u / u_len;
            let v_axis = rectangle.half_v / v_len;
            let offset = point - rectangle.center;
            let du = offset.dot(u_axis).clamp(-u_len, u_len);
            let dv = offset.dot(v_axis).clamp(-v_len, v_len);
            rectangle.center + u_axis * du + v_axis * dv
        }
        Area::Disk(disk) => {
            let (axis, bitangent) = disk.frame();
            let offset = point - disk.center;
            let du = offset.dot(axis);
            let dv = offset.dot(bitangent);
            let planar = (du * du + dv * dv).sqrt();
            if planar <= disk.radius.max(0.0) || planar <= MIN_DISTANCE {
                disk.center + axis * du + bitangent * dv
            } else {
                let scale = disk.radius / planar;
                disk.center + axis * du * scale + bitangent * dv * scale
            }
        }
    }
}

pub(crate) fn stratum_point(area: &Area, index: u32, rays: u32) -> Vec3 {
    let (u, v) = stratum_uv(index, rays);
    match area {
        Area::Rectangle(rectangle) => {
            rectangle.center
                + (u * 2.0 - 1.0) * rectangle.half_u
                + (v * 2.0 - 1.0) * rectangle.half_v
        }
        Area::Disk(disk) => {
            let (x, y) = concentric_disk(u, v);
            let (axis, bitangent) = disk.frame();
            disk.center + axis * (x * disk.radius) + bitangent * (y * disk.radius)
        }
    }
}

fn stratum_uv(index: u32, rays: u32) -> (f32, f32) {
    let cols = (rays as f32).sqrt().ceil() as u32;
    let cols = cols.max(1);
    let rows = (rays + cols - 1) / cols;
    let col = index % cols;
    let row = index / cols;
    (
        (col as f32 + 0.5) / cols as f32,
        (row as f32 + 0.5) / rows as f32,
    )
}

fn hemisphere_dir(normal: Vec3, index: u32, rays: u32) -> Vec3 {
    let (u, v) = stratum_uv(index, rays);
    let (x, y) = concentric_disk(u, v);
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    let (tangent, bitangent) = tangent_frame(normal);
    (tangent * x + bitangent * y + normal * z).normalize_or_zero()
}

fn concentric_disk(u: f32, v: f32) -> (f32, f32) {
    let a = 2.0 * u - 1.0;
    let b = 2.0 * v - 1.0;
    if a == 0.0 && b == 0.0 {
        return (0.0, 0.0);
    }
    let (radius, theta) = if a.abs() > b.abs() {
        (a, std::f32::consts::FRAC_PI_4 * (b / a))
    } else {
        (
            b,
            std::f32::consts::FRAC_PI_2 - std::f32::consts::FRAC_PI_4 * (a / b),
        )
    };
    (radius * theta.cos(), radius * theta.sin())
}

fn tangent_frame(normal: Vec3) -> (Vec3, Vec3) {
    let helper = if normal.x.abs() > 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let tangent = helper.cross(normal).normalize_or_zero();
    let bitangent = normal.cross(tangent);
    (tangent, bitangent)
}

fn arrives(
    scene: &Scene,
    origin: Vec3,
    sample: Vec3,
    receiver_normal: Vec3,
    area_normal: Vec3,
) -> bool {
    let delta = sample - origin;
    let distance = delta.length();
    if distance <= HIT_SLOP {
        return false;
    }
    let direction = delta / distance;
    if direction.dot(receiver_normal) <= 0.0 {
        return false;
    }
    if direction.dot(area_normal) >= 0.0 {
        return false;
    }
    !scene.occluded(origin, direction, distance - HIT_SLOP)
}

fn tint(albedo: Vec3, energy: [f32; 3]) -> [f32; 3] {
    [
        albedo.x * energy[0],
        albedo.y * energy[1],
        albedo.z * energy[2],
    ]
}

fn add_color(slot: &mut [f32; 3], add: [f32; 3]) {
    slot[0] += add[0];
    slot[1] += add[1];
    slot[2] += add[2];
}

fn sub_color(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn mix(from: [f32; 3], to: [f32; 3], factor: f32) -> [f32; 3] {
    [
        from[0] + (to[0] - from[0]) * factor,
        from[1] + (to[1] - from[1]) * factor,
        from[2] + (to[2] - from[2]) * factor,
    ]
}

fn luma(color: [f32; 3]) -> f32 {
    color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722
}
