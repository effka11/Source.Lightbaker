use glam::Vec3;
use rayon::prelude::*;

use crate::embree::Scene;
use crate::geom::{Receiver, Rectangle, Triangle};

const MIN_DISTANCE: f32 = 1.0e-2;
const RAY_LIFT: f32 = 0.5;
const HIT_SLOP: f32 = 1.0e-3;

pub fn solve(
    triangles: &[Triangle],
    receivers: &[Receiver],
    area: &Rectangle,
    rays: u32,
) -> Vec<[f32; 3]> {
    if receivers.is_empty() {
        return Vec::new();
    }
    if rays == 0 {
        return vec![[0.0, 0.0, 0.0]; receivers.len()];
    }

    let scene = Scene::build(triangles);
    receivers
        .par_iter()
        .map(|receiver| direct(&scene, receiver, area, rays))
        .collect()
}

pub(crate) fn unoccluded_intensity(area: &Rectangle, position: Vec3) -> f32 {
    let nearest = nearest_point(area, position);
    let distance = (nearest - position).length().max(MIN_DISTANCE);
    area.intensity / (distance * distance)
}

fn direct(scene: &Scene, receiver: &Receiver, area: &Rectangle, rays: u32) -> [f32; 3] {
    let normal = receiver.normal.normalize_or_zero();
    let scale = unoccluded_intensity(area, receiver.position);
    if scale == 0.0 || normal == Vec3::ZERO {
        return [0.0, 0.0, 0.0];
    }

    let origin = receiver.position + normal * RAY_LIFT;
    let mut hits = 0u32;
    for index in 0..rays {
        let sample = stratum_point(area, index, rays);
        if arrives(scene, origin, sample, normal, area.normal) {
            hits += 1;
        }
    }
    let visibility = hits as f32 / rays as f32;
    let value = scale * visibility;
    [value, value, value]
}

fn nearest_point(area: &Rectangle, point: Vec3) -> Vec3 {
    let u_len = area.half_u.length().max(MIN_DISTANCE);
    let v_len = area.half_v.length().max(MIN_DISTANCE);
    let u_axis = area.half_u / u_len;
    let v_axis = area.half_v / v_len;
    let offset = point - area.center;
    let du = offset.dot(u_axis).clamp(-u_len, u_len);
    let dv = offset.dot(v_axis).clamp(-v_len, v_len);
    area.center + u_axis * du + v_axis * dv
}

fn stratum_point(area: &Rectangle, index: u32, rays: u32) -> Vec3 {
    let cols = (rays as f32).sqrt().ceil() as u32;
    let cols = cols.max(1);
    let rows = (rays + cols - 1) / cols;
    let col = index % cols;
    let row = index / cols;
    let u = (col as f32 + 0.5) / cols as f32;
    let v = (row as f32 + 0.5) / rows as f32;
    area.center + (u * 2.0 - 1.0) * area.half_u + (v * 2.0 - 1.0) * area.half_v
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
