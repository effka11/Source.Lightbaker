//! Doors as light blockers. The BSP keeps every door entity; only the ray
//! mesh changes. A door starts closed. Opening it swings or slides the leaf.
//! Removing it drops the leaf from the ray mesh.

use glam::Vec3;
use solve::Triangle;

use crate::Surface;

/// One door the light can close, open, or ignore. The BSP entity stays.
#[derive(Clone, Debug)]
pub struct Door {
    pub id: String,
    pub name: String,
    pub closed: DoorPose,
    pub open: DoorPose,
}

/// Triangles that block rays, and the surfaces the window draws.
#[derive(Clone, Debug)]
pub struct DoorPose {
    pub triangles: Vec<Triangle>,
    pub surface: Vec<Surface>,
}

const BACKWARDS: i32 = 2;
const ROTATE_Z: i32 = 64;
const ROTATE_X: i32 = 128;

pub(crate) struct ModelRec {
    pub mins: Vec3,
    pub maxs: Vec3,
    pub origin: Vec3,
    pub first: i32,
    pub count: i32,
}

pub(crate) struct BrushJob {
    pub id: String,
    pub name: String,
    pub first: usize,
    pub count: usize,
    pub motion: Motion,
}

pub(crate) enum Motion {
    Slide(Vec3),
    /// `pivot` is missing when the entity has no origin. The caller uses the leaf center.
    Spin {
        pivot: Option<Vec3>,
        angles: Vec3,
    },
}

pub(crate) struct PropLeaf {
    pub id: String,
    pub name: String,
    pub origin: Vec3,
    pub angles: Vec3,
    pub open_angles: Vec3,
    pub model: String,
    pub skin: i32,
}

pub(crate) fn is_prop_door(class: &str) -> bool {
    class.eq_ignore_ascii_case("prop_door_rotating")
}

pub(crate) fn brush_jobs(entities: &str, models: &[ModelRec]) -> Vec<BrushJob> {
    let mut jobs = Vec::new();
    for body in blocks(entities) {
        let Some(class) = value(body, "classname") else {
            continue;
        };
        let rotating = class.eq_ignore_ascii_case("func_door_rotating");
        if !rotating && !class.eq_ignore_ascii_case("func_door") {
            continue;
        }
        let Some(index) = value(body, "model")
            .and_then(|model| model.strip_prefix('*'))
            .and_then(|rest| rest.parse::<usize>().ok())
        else {
            continue;
        };
        // Model 0 is the world. A door never owns it.
        let Some(model) = models.get(index).filter(|_| index > 0) else {
            continue;
        };
        if model.count <= 0 || model.first < 0 {
            continue;
        }
        let origin = value(body, "origin").and_then(parse_vec);
        let id = ident(class, body, origin.unwrap_or(model.origin), index);
        let name = value(body, "targetname").unwrap_or("").to_string();
        let motion = if rotating {
            // The brush is stored where Hammer left it, which is the closed pose.
            // Spawnflags that start the door open are ignored: light starts closed.
            Motion::Spin {
                pivot: origin,
                angles: swing(Vec3::ZERO, flags_of(body), distance_of(body)),
            }
        } else {
            Motion::Slide(slide_delta(
                movedir_of(body),
                model.maxs - model.mins,
                lip_of(body),
            ))
        };
        jobs.push(BrushJob {
            id,
            name,
            first: model.first as usize,
            count: model.count as usize,
            motion,
        });
    }
    jobs
}

pub(crate) fn prop_leaves(entities: &str) -> Vec<PropLeaf> {
    let mut leaves = Vec::new();
    for body in blocks(entities) {
        let Some(class) = value(body, "classname") else {
            continue;
        };
        if !is_prop_door(class) {
            continue;
        }
        let Some(model) = value(body, "model") else {
            continue;
        };
        if !model.to_ascii_lowercase().ends_with(".mdl") {
            continue;
        }
        let origin = value(body, "origin")
            .and_then(parse_vec)
            .unwrap_or(Vec3::ZERO);
        let angles = value(body, "angles")
            .or_else(|| value(body, "angle"))
            .and_then(parse_vec)
            .unwrap_or(Vec3::ZERO);
        leaves.push(PropLeaf {
            id: ident(class, body, origin, leaves.len()),
            name: value(body, "targetname").unwrap_or("").to_string(),
            origin,
            angles,
            open_angles: swing(angles, flags_of(body), distance_of(body)),
            model: model.to_string(),
            skin: value(body, "skin")
                .and_then(|text| text.parse().ok())
                .unwrap_or(0),
        });
    }
    leaves
}

pub(crate) fn translate_pose(pose: &DoorPose, delta: Vec3) -> DoorPose {
    map_pose(pose, |point| point + delta, |normal| normal)
}

pub(crate) fn spin_pose(pose: &DoorPose, origin: Vec3, angles: Vec3) -> DoorPose {
    let matrix = angle_matrix(angles, Vec3::ZERO);
    map_pose(
        pose,
        |point| origin + rotate(matrix, point - origin),
        |normal| rotate(matrix, normal),
    )
}

pub(crate) fn pose_center(pose: &DoorPose) -> Vec3 {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let mut any = false;
    for triangle in &pose.triangles {
        for point in triangle.vertices {
            if !point.is_finite() {
                continue;
            }
            min = min.min(point);
            max = max.max(point);
            any = true;
        }
    }
    if any {
        (min + max) * 0.5
    } else {
        Vec3::ZERO
    }
}

pub(crate) fn angle_matrix(angles: Vec3, origin: Vec3) -> [[f32; 4]; 3] {
    let deg = std::f32::consts::PI / 180.0;
    let (sy, cy) = (angles.y * deg).sin_cos();
    let (sp, cp) = (angles.x * deg).sin_cos();
    let (sr, cr) = (angles.z * deg).sin_cos();
    let crcy = cr * cy;
    let crsy = cr * sy;
    let srcy = sr * cy;
    let srsy = sr * sy;
    [
        [cp * cy, sp * srcy - crsy, sp * crcy + srsy, origin.x],
        [cp * sy, sp * srsy + crcy, sp * crsy - srcy, origin.y],
        [-sp, sr * cp, cr * cp, origin.z],
    ]
}

fn map_pose(pose: &DoorPose, point: impl Fn(Vec3) -> Vec3, dir: impl Fn(Vec3) -> Vec3) -> DoorPose {
    DoorPose {
        triangles: pose
            .triangles
            .iter()
            .map(|triangle| Triangle {
                vertices: triangle.vertices.map(&point),
            })
            .collect(),
        surface: pose
            .surface
            .iter()
            .map(|face| {
                let mut next = *face;
                next.vertices = face.vertices.map(&point);
                next.normal = face.normal.map(|normal| {
                    let turned = dir(normal);
                    if turned.length_squared() > 1.0e-8 {
                        turned.normalize()
                    } else {
                        normal
                    }
                });
                next
            })
            .collect(),
    }
}

fn rotate(matrix: [[f32; 4]; 3], point: Vec3) -> Vec3 {
    Vec3::new(
        point.x * matrix[0][0] + point.y * matrix[0][1] + point.z * matrix[0][2],
        point.x * matrix[1][0] + point.y * matrix[1][1] + point.z * matrix[1][2],
        point.x * matrix[2][0] + point.y * matrix[2][1] + point.z * matrix[2][2],
    )
}

fn slide_delta(dir: Vec3, size: Vec3, lip: f32) -> Vec3 {
    let dir = dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return Vec3::ZERO;
    }
    let span = dir.dot(size).abs() - lip;
    dir * span.max(0.0)
}

/// Closed angles plus the open swing. `flags` pick yaw, pitch, or roll.
fn swing(closed: Vec3, flags: i32, distance: f32) -> Vec3 {
    let mut axis = if flags & ROTATE_Z != 0 {
        Vec3::Z
    } else if flags & ROTATE_X != 0 {
        Vec3::X
    } else {
        Vec3::Y
    };
    if flags & BACKWARDS != 0 {
        axis = -axis;
    }
    let distance = if distance.is_finite() { distance } else { 90.0 };
    closed + axis * distance
}

fn flags_of(body: &str) -> i32 {
    value(body, "spawnflags")
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

fn distance_of(body: &str) -> f32 {
    value(body, "distance")
        .and_then(|text| text.parse().ok())
        .filter(|value: &f32| value.is_finite())
        .unwrap_or(90.0)
}

fn lip_of(body: &str) -> f32 {
    value(body, "lip")
        .and_then(|text| text.parse().ok())
        .filter(|value: &f32| value.is_finite())
        .unwrap_or(0.0)
}

fn movedir_of(body: &str) -> Vec3 {
    value(body, "movedir")
        .and_then(parse_vec)
        .filter(|dir| dir.length_squared() > 1.0e-8)
        .unwrap_or(Vec3::X)
}

fn ident(class: &str, body: &str, origin: Vec3, index: usize) -> String {
    if let Some(id) = value(body, "hammerid").filter(|id| !id.is_empty()) {
        return format!("{class}#{id}");
    }
    format!(
        "{class}|{index}|{:.0}_{:.0}_{:.0}",
        origin.x, origin.y, origin.z
    )
}

fn blocks(text: &str) -> impl Iterator<Item = &str> {
    text.split('{')
        .skip(1)
        .map(|block| block.split('}').next().unwrap_or(""))
}

fn value<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let mut quotes = body.match_indices('"').map(|(at, _)| at);
    while let Some(start) = quotes.next() {
        let end = quotes.next()?;
        let name = &body[start + 1..end];
        let value_start = quotes.next()?;
        let value_end = quotes.next()?;
        if name.eq_ignore_ascii_case(key) {
            return Some(&body[value_start + 1..value_end]);
        }
    }
    None
}

fn parse_vec(text: &str) -> Option<Vec3> {
    let mut parts = text.split_whitespace();
    Some(Vec3::new(
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose_at(point: Vec3) -> DoorPose {
        let vertices = [point, point + Vec3::X, point + Vec3::Z];
        DoorPose {
            triangles: vec![Triangle { vertices }],
            surface: vec![Surface {
                vertices,
                albedo: Vec3::ONE,
                uv: [[0.0; 2]; 3],
                blend: [0.0; 3],
                material: 0,
                light_uv: [[0.0; 2]; 3],
                light_face: u32::MAX,
                luxel: [u32::MAX; 3],
                prop: true,
                normal: [Vec3::Y; 3],
            }],
        }
    }

    #[test]
    fn a_prop_door_stays_closed_and_swings_the_other_way_when_asked() {
        let text = r#"
{
"classname" "prop_door_rotating"
"origin" "10 20 30"
"angles" "0 10 0"
"model" "models/props_c17/door01_left.mdl"
"distance" "90"
"spawnflags" "2"
"spawnpos" "1"
"hammerid" "4"
"targetname" "cell"
}
{
"classname" "prop_dynamic"
"model" "models/props_c17/oildrum001.mdl"
"origin" "0 0 0"
}
"#;
        let leaves = prop_leaves(text);
        assert_eq!(leaves.len(), 1);
        let leaf = &leaves[0];
        assert_eq!(leaf.name, "cell");
        assert_eq!(leaf.id, "prop_door_rotating#4");
        assert_eq!(leaf.angles, Vec3::new(0.0, 10.0, 0.0));
        assert_eq!(leaf.open_angles, Vec3::new(0.0, -80.0, 0.0));
    }

    #[test]
    fn a_sliding_door_moves_by_its_span_minus_the_lip() {
        let text = r#"
{
"classname" "func_door"
"model" "*1"
"movedir" "1 0 0"
"lip" "4"
"origin" "16 1 48"
"targetname" "gate"
}
"#;
        let models = [
            ModelRec {
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                origin: Vec3::ZERO,
                first: 0,
                count: 1,
            },
            ModelRec {
                mins: Vec3::ZERO,
                maxs: Vec3::new(32.0, 2.0, 96.0),
                origin: Vec3::new(16.0, 1.0, 48.0),
                first: 3,
                count: 2,
            },
        ];
        let jobs = brush_jobs(text, &models);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "gate");
        assert_eq!(jobs[0].first, 3);
        let Motion::Slide(delta) = jobs[0].motion else {
            panic!("slide");
        };
        assert!((delta - Vec3::new(28.0, 0.0, 0.0)).length() < 1.0e-3);
    }

    #[test]
    fn spinning_a_leaf_keeps_the_hinge_and_turns_the_slab() {
        let pose = pose_at(Vec3::new(16.0, 0.0, 0.0));
        let open = spin_pose(&pose, Vec3::ZERO, Vec3::new(0.0, 90.0, 0.0));
        let hinge = open.triangles[0].vertices[0];
        assert!(
            (hinge - Vec3::new(0.0, 16.0, 0.0)).length() < 1.0e-3,
            "{hinge:?}"
        );
        assert!((open.surface[0].normal[0] - Vec3::new(-1.0, 0.0, 0.0)).length() < 1.0e-3);
    }

    #[test]
    fn the_world_model_is_not_a_door() {
        let models = [ModelRec {
            mins: Vec3::ZERO,
            maxs: Vec3::ONE,
            origin: Vec3::ZERO,
            first: 0,
            count: 4,
        }];
        let text = r#"
{
"classname" "func_door"
"model" "*0"
"movedir" "0 0 1"
}
"#;
        assert!(brush_jobs(text, &models).is_empty());
    }
}
