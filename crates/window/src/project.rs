//! A project is the map path, the categories, and the light objects.
//! An untouched template emits the same patch as [`lamps::Lamp`].

use std::path::{Path, PathBuf};

use glam::{Mat3, Vec3};
use lamps::{Kind, Lamp};
use serde::{Deserialize, Serialize};
use solve::{Area, Disk, Omni, Rectangle, Volume};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Plane,
    Solid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Square,
    Circle,
    Cube,
    Sphere,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LightObject {
    pub name: String,
    pub category: String,
    pub space: Space,
    pub size: Vec3,
    pub position: Vec3,
    pub rotation: Vec3,
    pub corners: f32,
    pub intensity: f32,
    pub color: Vec3,
    /// 0 hides the light on the mesh inside the figure, 100 is the brightest glass.
    pub inside: f32,
    /// 0 cuts the interior light at the hull, 100 fades it well past the hull.
    pub rim: f32,
    /// 0 keeps the bright glow at the center, 100 carries it out to the hull.
    pub spread: f32,
    /// 0 leaves the metal cage dark, 100 lights it as brightly as the glass.
    pub mesh: f32,
    pub template: Option<Kind>,
    pub custom: bool,
}

fn base_inside() -> f32 {
    crate::base::live().inside
}

fn base_rim() -> f32 {
    crate::base::live().rim
}

fn base_spread() -> f32 {
    crate::base::live().spread
}

fn base_mesh() -> f32 {
    crate::base::live().mesh
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorState {
    Closed,
    Open,
    Gone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoorChoice {
    pub id: String,
    pub state: DoorState,
}

pub fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Fluorescent => "Люминесцент",
        Kind::Bulb => "Колба",
        Kind::Sconce => "Бра",
        Kind::Camera => "Камера",
        Kind::Emergency => "Аварийная",
    }
}

/// The fixture mesh that belongs to a lamp. `axis_*` are half-extents in world space.
#[derive(Clone, Copy, Debug)]
pub struct FigureGlow {
    pub center: Vec3,
    pub axis_x: Vec3,
    pub axis_y: Vec3,
    pub axis_z: Vec3,
    pub color: Vec3,
    pub intensity: f32,
    /// 0..1, how strongly the mesh inside the figure is lit.
    pub inside: f32,
    /// 0..1, how far that light fades past the hull.
    pub rim: f32,
    /// 0..1, how far the bright center reaches toward the hull.
    pub spread: f32,
    /// 0..1, how much of that glow the metal cage keeps.
    pub mesh: f32,
}

impl FigureGlow {
    pub fn contains(&self, point: Vec3) -> bool {
        let offset = point - self.center;
        let inside = |axis: Vec3| {
            let len2 = axis.length_squared();
            len2 > 1.0e-8 && offset.dot(axis).abs() / len2 <= 1.0
        };
        inside(self.axis_x) && inside(self.axis_y) && inside(self.axis_z)
    }
}

/// Built-in group. The lights below are meant to be seated on the fixture.
pub const RELAPSE: &str = "relapse";

/// One light from the Relapse set. Sizes are the model hull, in Source units.
pub struct Fixture {
    pub name: &'static str,
    pub space: Space,
    pub size: Vec3,
    pub corners: f32,
    pub intensity: f32,
    pub color: Vec3,
    /// Added to the camera target so the sun and moon start in the sky.
    pub lift: Vec3,
}

/// Cool fluorescent from the Relapse `lamp_lumi` spots: 210 225 255.
const LUMI: Vec3 = Vec3::new(0.824, 0.882, 1.0);
/// Warm bulb from the Relapse `lamp_colba` spots: 255 236 210.
const COLBA: Vec3 = Vec3::new(1.0, 0.925, 0.824);
const FIXTURE: f32 = 18_000.0;

const RELAPSE_FIXTURES: &[Fixture] = &[
    // Overcast sun from `light_environment` on relapse_jail: 175 230 239.
    // A disk this strong, 4096 above the floor, lands near a dim cloudy day.
    Fixture {
        name: "Солнце",
        space: Space::Plane,
        size: Vec3::new(1024.0, 1024.0, 1.0),
        corners: 0.0,
        intensity: 8_000_000.0,
        color: Vec3::new(0.686, 0.902, 0.937),
        lift: Vec3::new(0.0, 0.0, 4096.0),
    },
    // Dimmer and bluer than the sun, still a disk in the sky.
    Fixture {
        name: "Луна",
        space: Space::Plane,
        size: Vec3::new(768.0, 768.0, 1.0),
        corners: 0.0,
        intensity: 1_200_000.0,
        color: Vec3::new(0.55, 0.70, 1.0),
        lift: Vec3::new(1400.0, 0.0, 3200.0),
    },
    // models/props_c17/light_domelight02_on.mdl — 18 × 18.7 × 7.6
    Fixture {
        name: "Купол",
        space: Space::Solid,
        size: Vec3::new(18.0, 18.7, 7.6),
        corners: 0.0,
        intensity: FIXTURE,
        color: LUMI,
        lift: Vec3::ZERO,
    },
    // models/props_lab/lab_flourescentlight001a.mdl — 12.4 × 66.4 × 65.9
    Fixture {
        name: "Панель",
        space: Space::Solid,
        size: Vec3::new(12.4, 66.4, 65.9),
        corners: 90.0,
        intensity: FIXTURE,
        color: LUMI,
        lift: Vec3::ZERO,
    },
    // models/props_wasteland/prison_flourescentlight002b.mdl — 6.2 × 63 × 8.6
    Fixture {
        name: "Люминесцент",
        space: Space::Solid,
        size: Vec3::new(6.2, 63.0, 8.6),
        corners: 55.0,
        intensity: FIXTURE,
        color: LUMI,
        lift: Vec3::ZERO,
    },
    // models/props_wasteland/prison_cagedlight001a.mdl — 4.1 × 7.8 × 13.1
    Fixture {
        name: "Клетка",
        space: Space::Solid,
        size: Vec3::new(4.1, 7.8, 13.1),
        corners: 25.0,
        intensity: FIXTURE,
        color: COLBA,
        lift: Vec3::ZERO,
    },
    // models/props_wasteland/prison_lamp001a.mdl — 22.1 × 22.1 × 25.7
    Fixture {
        name: "Плафон",
        space: Space::Solid,
        size: Vec3::new(22.1, 22.1, 25.7),
        corners: 20.0,
        intensity: FIXTURE,
        color: COLBA,
        lift: Vec3::ZERO,
    },
    // models/props/de_nuke/wall_light.mdl — 5.9 × 38.7 × 3.4
    Fixture {
        name: "Настенный",
        space: Space::Solid,
        size: Vec3::new(5.9, 38.7, 3.4),
        corners: 70.0,
        intensity: FIXTURE,
        color: COLBA,
        lift: Vec3::ZERO,
    },
    // models/props/cs_office/light_security.mdl — 9.6 × 19.5 × 15.9
    Fixture {
        name: "Охрана",
        space: Space::Solid,
        size: Vec3::new(9.6, 19.5, 15.9),
        corners: 40.0,
        intensity: 20_000.0,
        color: Vec3::new(0.90, 0.95, 1.0),
        lift: Vec3::ZERO,
    },
    // models/props_wasteland/light_spotlight01_lamp.mdl — 22.2 × 19.9 × 19.3
    Fixture {
        name: "Прожектор",
        space: Space::Solid,
        size: Vec3::new(22.2, 19.9, 19.3),
        corners: 30.0,
        intensity: 28_000.0,
        color: Vec3::new(0.96, 0.98, 1.0),
        lift: Vec3::ZERO,
    },
];

pub fn relapse_fixtures() -> &'static [Fixture] {
    RELAPSE_FIXTURES
}

pub fn base_label(base: Base) -> &'static str {
    match base {
        Base::Square => "Квадрат",
        Base::Circle => "Круг",
        Base::Cube => "Куб",
        Base::Sphere => "Шар",
    }
}

impl LightObject {
    pub fn fluorescent(position: Vec3) -> Self {
        Self::from_template(Kind::Fluorescent, position)
    }

    pub fn from_template(kind: Kind, position: Vec3) -> Self {
        let area = Lamp {
            kind,
            position,
            forward: Vec3::X,
        }
        .area();
        let (size, corners) = match area {
            Area::Rectangle(rectangle) => (
                Vec3::new(
                    rectangle.half_u.length() * 2.0,
                    rectangle.half_v.length() * 2.0,
                    1.0,
                ),
                100.0,
            ),
            Area::Disk(disk) => (Vec3::new(disk.radius * 2.0, disk.radius * 2.0, 1.0), 0.0),
            Area::Volume(_) | Area::Omni(_) => (Vec3::splat(48.0), 0.0),
        };
        let mut object = Self {
            name: kind_label(kind).to_owned(),
            category: String::new(),
            space: Space::Plane,
            size,
            position,
            rotation: Vec3::new(0.0, template_pitch(kind), 0.0),
            corners,
            intensity: area.intensity(),
            color: area.color(),
            inside: base_inside(),
            rim: base_rim(),
            spread: base_spread(),
            mesh: base_mesh(),
            template: Some(kind),
            custom: false,
        };
        apply_base(&mut object, &crate::base::live());
        object
    }

    pub fn from_fixture(fixture: &Fixture, position: Vec3) -> Self {
        let mut object = Self {
            name: fixture.name.to_owned(),
            category: RELAPSE.to_owned(),
            space: fixture.space,
            size: fixture.size,
            position: position + fixture.lift,
            rotation: Vec3::ZERO,
            corners: fixture.corners,
            intensity: fixture.intensity,
            color: fixture.color,
            inside: base_inside(),
            rim: base_rim(),
            spread: base_spread(),
            mesh: base_mesh(),
            template: None,
            custom: true,
        };
        apply_base(&mut object, &crate::base::live());
        object
    }

    pub fn from_base(base: Base, position: Vec3) -> Self {
        let solid = matches!(base, Base::Cube | Base::Sphere);
        let round = matches!(base, Base::Circle | Base::Sphere);
        let lamp = Lamp {
            kind: Kind::Fluorescent,
            position: Vec3::ZERO,
            forward: Vec3::X,
        }
        .area();
        let mut object = Self {
            name: base_label(base).to_owned(),
            category: String::new(),
            space: if solid { Space::Solid } else { Space::Plane },
            size: if solid {
                Vec3::splat(48.0)
            } else {
                Vec3::new(48.0, 48.0, 1.0)
            },
            position,
            rotation: Vec3::ZERO,
            corners: if round { 0.0 } else { 100.0 },
            intensity: lamp.intensity(),
            color: Vec3::ONE,
            inside: base_inside(),
            rim: base_rim(),
            spread: base_spread(),
            mesh: base_mesh(),
            template: None,
            custom: true,
        };
        apply_base(&mut object, &crate::base::live());
        object
    }

    pub fn areas(&self) -> Vec<Area> {
        if let Some(kind) = self.template {
            if !self.custom {
                return vec![Lamp {
                    kind,
                    position: self.position,
                    forward: Vec3::X,
                }
                .area()];
            }
        }
        self.parametric_areas()
    }

    /// Axis-aligned bounds of the oriented box, used as the hitbox.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (x, y, z) = self.axes();
        let mut half = self.size * 0.5;
        if self.space == Space::Plane {
            half.z = half.z.max(0.5);
        }
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    let point = self.position + x * half.x * sx + y * half.y * sy + z * half.z * sz;
                    min = min.min(point);
                    max = max.max(point);
                }
            }
        }
        (min, max)
    }

    pub fn shape_vertices(&self) -> Vec<[f32; 3]> {
        let mut vertices = Vec::new();
        let (x, y, z) = self.axes();
        let place =
            |point: Vec3| (self.position + x * point.x + y * point.y + z * point.z).to_array();
        let exp = 2.0 / roundness(self.corners);
        if self.space == Space::Plane {
            let hx = self.size.x.max(1.0) * 0.5;
            let hy = self.size.y.max(1.0) * 0.5;
            let steps = 40u32;
            let mut ring = Vec::with_capacity(steps as usize);
            for step in 0..steps {
                let angle = step as f32 / steps as f32 * std::f32::consts::TAU;
                ring.push(Vec3::new(
                    signed_pow(angle.cos(), exp) * hx,
                    signed_pow(angle.sin(), exp) * hy,
                    0.0,
                ));
            }
            let center = Vec3::ZERO;
            for index in 0..steps as usize {
                let next = (index + 1) % steps as usize;
                push_tri(&mut vertices, place, center, ring[index], ring[next]);
                push_tri(&mut vertices, place, center, ring[next], ring[index]);
            }
        } else {
            let hx = self.size.x.max(1.0) * 0.5;
            let hy = self.size.y.max(1.0) * 0.5;
            let hz = self.size.z.max(1.0) * 0.5;
            let stacks = 12u32;
            let slices = 18u32;
            let point = |theta: f32, phi: f32| {
                let (ct, st) = (theta.cos(), theta.sin());
                let (cp, sp) = (phi.cos(), phi.sin());
                Vec3::new(
                    hx * signed_pow(ct, exp) * signed_pow(cp, exp),
                    hy * signed_pow(ct, exp) * signed_pow(sp, exp),
                    hz * signed_pow(st, exp),
                )
            };
            for stack in 0..stacks {
                let t0 = -std::f32::consts::FRAC_PI_2
                    + stack as f32 / stacks as f32 * std::f32::consts::PI;
                let t1 = -std::f32::consts::FRAC_PI_2
                    + (stack + 1) as f32 / stacks as f32 * std::f32::consts::PI;
                for slice in 0..slices {
                    let p0 = slice as f32 / slices as f32 * std::f32::consts::TAU;
                    let p1 = (slice + 1) as f32 / slices as f32 * std::f32::consts::TAU;
                    let a = point(t0, p0);
                    let b = point(t0, p1);
                    let c = point(t1, p1);
                    let d = point(t1, p0);
                    push_tri(&mut vertices, place, a, d, c);
                    push_tri(&mut vertices, place, a, c, b);
                }
            }
        }
        vertices
    }

    /// Which mesh is this lamp's glass. The draw shades that shell; it does not
    /// paint the hull a flat color.
    pub fn figure_glow(&self) -> Option<FigureGlow> {
        if self.intensity <= 0.0 {
            return None;
        }
        let (x, y, z) = self.axes();
        let mut half = self.size.max(Vec3::splat(1.0)) * 0.5;
        if self.space == Space::Plane {
            half.z = half.z.max(1.0);
        }
        // The mesh sits on the hull. A little slack keeps that shell inside.
        let slack = 2.0;
        Some(FigureGlow {
            center: self.position,
            axis_x: x * (half.x + slack),
            axis_y: y * (half.y + slack),
            axis_z: z * (half.z + slack),
            color: self.color.clamp(Vec3::ZERO, Vec3::ONE),
            intensity: self.intensity,
            inside: (self.inside / 100.0).clamp(0.0, 1.0),
            rim: (self.rim / 100.0).clamp(0.0, 1.0),
            spread: (self.spread / 100.0).clamp(0.0, 1.0),
            mesh: (self.mesh / 100.0).clamp(0.0, 1.0),
        })
    }

    fn axes(&self) -> (Vec3, Vec3, Vec3) {
        let yaw = self.rotation.x.to_radians();
        let pitch = self.rotation.y.to_radians();
        let roll = self.rotation.z.to_radians();
        let basis =
            Mat3::from_rotation_z(yaw) * Mat3::from_rotation_y(pitch) * Mat3::from_rotation_x(roll);
        (basis.x_axis, basis.y_axis, basis.z_axis)
    }

    fn parametric_areas(&self) -> Vec<Area> {
        let (x, y, z) = self.axes();
        let blend = (self.corners / 100.0).clamp(0.0, 1.0);
        let mut areas = Vec::new();
        if self.space == Space::Plane {
            push_patch(
                &mut areas,
                self.position,
                -z,
                x * self.size.x.max(1.0) * 0.5,
                y * self.size.y.max(1.0) * 0.5,
                blend,
                self.intensity,
                self.color,
            );
        } else {
            let hx = self.size.x.max(1.0) * 0.5;
            let hy = self.size.y.max(1.0) * 0.5;
            let hz = self.size.z.max(1.0) * 0.5;
            let share = self.intensity / 6.0;
            let cavity = (self.inside / 100.0).clamp(0.0, 1.0);
            // The cavity is the glass. The hull's faces are not lights: their
            // planes cut a shadow through the room wherever the figure sits.
            areas.push(Area::Volume(Volume {
                center: self.position,
                axis_x: x * hx,
                axis_y: y * hy,
                axis_z: z * hz,
                intensity: share * cavity,
                color: self.color,
            }));
            areas.push(Area::Omni(Omni {
                center: self.position,
                axis_x: x * hx,
                axis_y: y * hy,
                axis_z: z * hz,
                intensity: self.intensity / 3.0,
                color: self.color,
            }));
        }
        areas
    }
}

fn push_patch(
    areas: &mut Vec<Area>,
    center: Vec3,
    normal: Vec3,
    half_u: Vec3,
    half_v: Vec3,
    blend: f32,
    intensity: f32,
    color: Vec3,
) {
    let normal = normal.normalize_or_zero();
    if normal == Vec3::ZERO || intensity <= 0.0 {
        return;
    }
    if blend > 0.001 {
        areas.push(Area::Rectangle(Rectangle {
            center,
            half_u,
            half_v,
            normal,
            intensity: intensity * blend,
            color,
        }));
    }
    if blend < 0.999 {
        let radius = half_u.length().min(half_v.length());
        if radius > 0.0 {
            areas.push(disk(
                center,
                radius,
                normal,
                intensity * (1.0 - blend),
                color,
            ));
        }
    }
}

fn disk(center: Vec3, radius: f32, normal: Vec3, intensity: f32, color: Vec3) -> Area {
    let helper = if normal.z.abs() > 0.9 {
        Vec3::X
    } else {
        Vec3::Z
    };
    Area::Disk(Disk {
        center,
        radius,
        normal,
        axis: helper.cross(normal).normalize_or_zero(),
        intensity,
        color,
    })
}

fn template_pitch(kind: Kind) -> f32 {
    let area = Lamp {
        kind,
        position: Vec3::ZERO,
        forward: Vec3::X,
    }
    .area();
    let Area::Disk(disk) = area else {
        return 0.0;
    };
    let tilt = disk
        .normal
        .normalize()
        .dot(-Vec3::Z)
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    if disk.normal.x >= 0.0 {
        -tilt
    } else {
        tilt
    }
}

fn roundness(corners: f32) -> f32 {
    let t = (corners / 100.0).clamp(0.0, 1.0);
    2.0_f32.powf(1.0 + t * 5.0)
}

fn signed_pow(value: f32, exp: f32) -> f32 {
    if value == 0.0 {
        0.0
    } else {
        value.signum() * value.abs().powf(exp)
    }
}

fn push_tri(
    vertices: &mut Vec<[f32; 3]>,
    place: impl Fn(Vec3) -> [f32; 3],
    a: Vec3,
    b: Vec3,
    c: Vec3,
) {
    vertices.push(place(a));
    vertices.push(place(b));
    vertices.push(place(c));
}

#[derive(Serialize, Deserialize)]
struct SavedProject {
    map: String,
    categories: Vec<String>,
    objects: Vec<SavedObject>,
    #[serde(default)]
    doors: Vec<SavedDoor>,
}

#[derive(Serialize, Deserialize)]
struct SavedDoor {
    id: String,
    state: String,
}

#[derive(Serialize, Deserialize)]
struct SavedObject {
    name: String,
    category: String,
    space: String,
    size: [f32; 3],
    position: [f32; 3],
    rotation: [f32; 3],
    corners: f32,
    intensity: f32,
    color: [f32; 3],
    #[serde(default = "default_inside")]
    inside: f32,
    #[serde(default = "default_rim")]
    rim: f32,
    #[serde(default = "default_spread")]
    spread: f32,
    #[serde(default = "default_mesh")]
    mesh: f32,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    custom: bool,
}

fn default_inside() -> f32 {
    base_inside()
}

fn default_rim() -> f32 {
    base_rim()
}

fn default_spread() -> f32 {
    base_spread()
}

fn default_mesh() -> f32 {
    base_mesh()
}

/// Copy the saved base onto a light. Name, category, and position stay.
pub fn apply_base(object: &mut LightObject, base: &crate::base::Params) {
    object.inside = base.inside.clamp(0.0, 100.0);
    object.rim = base.rim.clamp(0.0, 100.0);
    object.spread = base.spread.clamp(0.0, 100.0);
    object.mesh = base.mesh.clamp(0.0, 100.0);
    if !base.complete {
        return;
    }
    object.space = if base.solid {
        Space::Solid
    } else {
        Space::Plane
    };
    object.size = Vec3::from_array(base.size).max(Vec3::splat(1.0));
    object.rotation = Vec3::from_array(base.rotation);
    object.corners = base.corners.clamp(0.0, 100.0);
    object.intensity = base.intensity.max(0.0);
    object.color = Vec3::from_array(base.color).clamp(Vec3::ZERO, Vec3::ONE);
    object.custom = true;
}

pub fn capture_base(object: &LightObject) -> crate::base::Params {
    crate::base::Params {
        complete: true,
        solid: object.space == Space::Solid,
        size: object.size.to_array(),
        rotation: object.rotation.to_array(),
        corners: object.corners,
        intensity: object.intensity,
        color: object.color.to_array(),
        inside: object.inside,
        rim: object.rim,
        spread: object.spread,
        mesh: object.mesh,
    }
}

pub fn write_project(
    path: &Path,
    map: &Path,
    categories: &[String],
    objects: &[LightObject],
    doors: &[DoorChoice],
) -> Result<(), String> {
    let saved = SavedProject {
        map: map.display().to_string(),
        categories: categories.to_vec(),
        objects: objects.iter().map(SavedObject::from).collect(),
        doors: doors
            .iter()
            .filter(|door| door.state != DoorState::Closed)
            .map(|door| SavedDoor {
                id: door.id.clone(),
                state: door_tag(door.state).to_owned(),
            })
            .collect(),
    };
    let text = serde_json::to_string_pretty(&saved).map_err(|err| err.to_string())?;
    std::fs::write(path, text).map_err(|err| err.to_string())
}

/// Lights only. The open map stays; these objects are placed at their saved coordinates.
#[derive(Serialize, Deserialize)]
struct SavedLights {
    categories: Vec<String>,
    objects: Vec<SavedObject>,
}

pub fn write_lights(
    path: &Path,
    categories: &[String],
    objects: &[LightObject],
) -> Result<(), String> {
    let saved = SavedLights {
        categories: categories.to_vec(),
        objects: objects.iter().map(SavedObject::from).collect(),
    };
    let text = serde_json::to_string_pretty(&saved).map_err(|err| err.to_string())?;
    std::fs::write(path, text).map_err(|err| err.to_string())
}

pub fn read_lights(path: &Path) -> Result<(Vec<String>, Vec<LightObject>), String> {
    let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let saved: SavedLights = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    let objects = saved
        .objects
        .into_iter()
        .map(SavedObject::into_object)
        .collect();
    Ok((saved.categories, objects))
}

pub fn read_project(
    path: &Path,
) -> Result<(PathBuf, Vec<String>, Vec<LightObject>, Vec<DoorChoice>), String> {
    let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let saved: SavedProject = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    let objects = saved
        .objects
        .into_iter()
        .map(SavedObject::into_object)
        .collect();
    let doors = saved
        .doors
        .into_iter()
        .map(|door| DoorChoice {
            id: door.id,
            state: door_state(&door.state),
        })
        .collect();
    Ok((PathBuf::from(saved.map), saved.categories, objects, doors))
}

fn door_tag(state: DoorState) -> &'static str {
    match state {
        DoorState::Closed => "closed",
        DoorState::Open => "open",
        DoorState::Gone => "gone",
    }
}

fn door_state(tag: &str) -> DoorState {
    match tag {
        "open" => DoorState::Open,
        "gone" | "deleted" => DoorState::Gone,
        _ => DoorState::Closed,
    }
}

impl From<&LightObject> for SavedObject {
    fn from(object: &LightObject) -> Self {
        Self {
            name: object.name.clone(),
            category: object.category.clone(),
            space: match object.space {
                Space::Plane => "2d",
                Space::Solid => "3d",
            }
            .to_owned(),
            size: object.size.to_array(),
            position: object.position.to_array(),
            rotation: object.rotation.to_array(),
            corners: object.corners,
            intensity: object.intensity,
            color: object.color.to_array(),
            inside: object.inside.clamp(0.0, 100.0),
            rim: object.rim.clamp(0.0, 100.0),
            spread: object.spread.clamp(0.0, 100.0),
            mesh: object.mesh.clamp(0.0, 100.0),
            template: object.template.map(kind_tag).map(str::to_owned),
            custom: object.custom,
        }
    }
}

impl SavedObject {
    fn into_object(self) -> LightObject {
        LightObject {
            name: self.name,
            category: self.category,
            space: if self.space == "3d" {
                Space::Solid
            } else {
                Space::Plane
            },
            size: Vec3::from_array(self.size),
            position: Vec3::from_array(self.position),
            rotation: Vec3::from_array(self.rotation),
            corners: self.corners.clamp(0.0, 100.0),
            intensity: self.intensity.max(0.0),
            color: Vec3::from_array(self.color),
            inside: self.inside.clamp(0.0, 100.0),
            rim: self.rim.clamp(0.0, 100.0),
            spread: self.spread.clamp(0.0, 100.0),
            mesh: self.mesh.clamp(0.0, 100.0),
            template: self.template.as_deref().and_then(kind_from),
            custom: self.custom,
        }
    }
}

fn kind_tag(kind: Kind) -> &'static str {
    match kind {
        Kind::Fluorescent => "fluorescent",
        Kind::Bulb => "bulb",
        Kind::Sconce => "sconce",
        Kind::Camera => "camera",
        Kind::Emergency => "emergency",
    }
}

fn kind_from(tag: &str) -> Option<Kind> {
    Some(match tag {
        "fluorescent" => Kind::Fluorescent,
        "bulb" => Kind::Bulb,
        "sconce" => Kind::Sconce,
        "camera" => Kind::Camera,
        "emergency" => Kind::Emergency,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(left: Vec3, right: Vec3) -> bool {
        (left - right).length() < 1.0e-3
    }

    #[test]
    fn an_untouched_fluorescent_matches_the_lamp() {
        let position = Vec3::new(1.0, 2.0, 3.0);
        let object = LightObject::fluorescent(position);
        if crate::base::live().complete {
            let base = crate::base::live();
            assert!(object.custom);
            assert!((object.intensity - base.intensity).abs() < 1.0e-2);
            assert!((object.inside - base.inside).abs() < 1.0e-3);
            return;
        }
        let Area::Rectangle(lamp) = Lamp {
            kind: Kind::Fluorescent,
            position,
            forward: Vec3::X,
        }
        .area() else {
            panic!("fluorescent is a rectangle");
        };
        let areas = object.areas();
        let Area::Rectangle(got) = areas[0] else {
            panic!("one rectangle");
        };
        assert_eq!(areas.len(), 1);
        assert!(close(got.center, lamp.center));
        assert!(close(got.half_u, lamp.half_u));
        assert!(close(got.half_v, lamp.half_v));
        assert!(close(got.normal, lamp.normal));
        assert_eq!(got.intensity, lamp.intensity);
        assert_eq!(got.color, lamp.color);
    }

    #[test]
    fn a_custom_fluorescent_keeps_the_same_patch() {
        let mut object = LightObject::fluorescent(Vec3::new(4.0, 5.0, 6.0));
        if crate::base::live().complete {
            assert!((object.intensity - crate::base::live().intensity).abs() < 1.0e-2);
            return;
        }
        object.custom = true;
        let Area::Rectangle(lamp) = Lamp {
            kind: Kind::Fluorescent,
            position: object.position,
            forward: Vec3::X,
        }
        .area() else {
            panic!("fluorescent is a rectangle");
        };
        let areas = object.areas();
        assert_eq!(areas.len(), 1);
        let Area::Rectangle(got) = areas[0] else {
            panic!("rectangle");
        };
        assert!(close(got.center, lamp.center));
        assert!(close(got.half_u, lamp.half_u));
        assert!(close(got.half_v, lamp.half_v));
        assert!(close(got.normal, lamp.normal));
        assert!((got.intensity - lamp.intensity).abs() < 1.0e-2);
    }

    #[test]
    fn a_custom_bulb_keeps_the_tilted_disk() {
        let mut object = LightObject::from_template(Kind::Bulb, Vec3::new(8.0, 9.0, 10.0));
        if crate::base::live().complete {
            assert!((object.intensity - crate::base::live().intensity).abs() < 1.0e-2);
            return;
        }
        object.custom = true;
        let Area::Disk(lamp) = Lamp {
            kind: Kind::Bulb,
            position: object.position,
            forward: Vec3::X,
        }
        .area() else {
            panic!("bulb is a disk");
        };
        let areas = object.areas();
        assert_eq!(areas.len(), 1);
        let Area::Disk(got) = areas[0] else {
            panic!("disk");
        };
        assert!(close(got.center, lamp.center));
        assert!((got.radius - lamp.radius).abs() < 1.0e-3);
        assert!(close(got.normal, lamp.normal));
        assert!((got.intensity - lamp.intensity).abs() < 1.0e-2);
        assert!(close(got.color, lamp.color));
    }

    #[test]
    fn corners_blend_from_a_disk_to_a_rectangle() {
        let mut object = LightObject::from_base(Base::Square, Vec3::ZERO);
        object.space = Space::Plane;
        object.corners = 0.0;
        assert!(matches!(object.areas()[0], Area::Disk(_)));
        object.corners = 100.0;
        assert!(matches!(object.areas()[0], Area::Rectangle(_)));
        object.corners = 50.0;
        assert_eq!(object.areas().len(), 2);
    }

    #[test]
    fn a_project_roundtrip_keeps_categories_and_objects() {
        let object = LightObject::fluorescent(Vec3::new(1.0, 2.0, 3.0));
        let categories = vec!["Зал".to_owned()];
        let map = PathBuf::from("maps/room.bsp");
        let path = std::env::temp_dir().join("lightbaker-project-roundtrip.lbr");
        write_project(&path, &map, &categories, &[object.clone()], &[]).unwrap();
        let (read_map, read_categories, read_objects, read_doors) = read_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_map, map);
        assert_eq!(read_categories, categories);
        assert_eq!(read_objects.len(), 1);
        assert_eq!(read_objects[0].name, object.name);
        assert_eq!(read_objects[0].template, Some(Kind::Fluorescent));
        assert_eq!(read_objects[0].custom, object.custom);
        assert!(close(read_objects[0].position, object.position));
        assert!(read_doors.is_empty());
    }

    #[test]
    fn a_project_keeps_an_open_door_and_forgets_a_closed_one() {
        let path = std::env::temp_dir().join("lightbaker-project-doors.lbr");
        let doors = [
            DoorChoice {
                id: "prop_door_rotating#1".to_owned(),
                state: DoorState::Open,
            },
            DoorChoice {
                id: "func_door#2".to_owned(),
                state: DoorState::Closed,
            },
            DoorChoice {
                id: "func_door#3".to_owned(),
                state: DoorState::Gone,
            },
        ];
        write_project(&path, Path::new("maps/room.bsp"), &[], &[], &doors).unwrap();
        let (_, _, _, read) = read_project(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            read,
            vec![
                DoorChoice {
                    id: "prop_door_rotating#1".to_owned(),
                    state: DoorState::Open,
                },
                DoorChoice {
                    id: "func_door#3".to_owned(),
                    state: DoorState::Gone,
                },
            ]
        );
    }

    #[test]
    fn a_lights_file_keeps_the_objects_and_not_the_map() {
        let mut object = LightObject::fluorescent(Vec3::new(1.0, 2.0, 3.0));
        object.name = "Клетка".to_owned();
        object.category = "Зал".to_owned();
        object.rotation = Vec3::new(4.0, 5.0, 6.0);
        object.size = Vec3::new(7.0, 8.0, 9.0);
        object.corners = 25.0;
        object.intensity = 18_000.0;
        object.color = Vec3::new(1.0, 0.5, 0.25);
        object.inside = 40.0;
        object.rim = 70.0;
        object.spread = 60.0;
        object.mesh = 35.0;
        object.custom = true;
        let categories = vec!["Зал".to_owned()];
        let path = std::env::temp_dir().join("lightbaker-lights-roundtrip.lbrl");
        write_lights(&path, &categories, &[object.clone()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let (read_categories, read_objects) = read_lights(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(!text.contains("\"map\""));
        assert_eq!(read_categories, categories);
        assert_eq!(read_objects.len(), 1);
        let got = &read_objects[0];
        assert_eq!(got.name, object.name);
        assert_eq!(got.category, object.category);
        assert_eq!(got.template, object.template);
        assert!(got.custom);
        assert!(close(got.position, object.position));
        assert!(close(got.rotation, object.rotation));
        assert!(close(got.size, object.size));
        assert!((got.corners - object.corners).abs() < 1.0e-3);
        assert!((got.intensity - object.intensity).abs() < 1.0e-3);
        assert!(close(got.color, object.color));
        assert!((got.inside - object.inside).abs() < 1.0e-3);
        assert!((got.rim - object.rim).abs() < 1.0e-3);
        assert!((got.spread - object.spread).abs() < 1.0e-3);
        assert!((got.mesh - object.mesh).abs() < 1.0e-3);
    }

    #[test]
    fn relapse_lights_wear_the_fixture_and_shine_both_ways() {
        let fixtures = relapse_fixtures();
        assert!(fixtures.iter().any(|fixture| fixture.name == "Солнце"));
        assert!(fixtures.iter().any(|fixture| fixture.name == "Луна"));
        let at = Vec3::new(10.0, 20.0, 30.0);
        for fixture in fixtures {
            let object = LightObject::from_fixture(fixture, at);
            assert_eq!(object.category, RELAPSE);
            assert!(object.custom);
            assert!(object.template.is_none());
            assert!(close(object.position, at + fixture.lift));
            assert!(!object.areas().is_empty());
        }
        let sun = LightObject::from_fixture(&fixtures[0], at);
        let moon = LightObject::from_fixture(&fixtures[1], at);
        let base = crate::base::live();
        if base.complete {
            assert!((sun.intensity - base.intensity).abs() < 1.0);
            assert!((moon.intensity - base.intensity).abs() < 1.0);
            assert_eq!(
                sun.space,
                if base.solid {
                    Space::Solid
                } else {
                    Space::Plane
                }
            );
        } else {
            assert!(sun.intensity > moon.intensity * 4.0);
            assert!(sun.color.z > sun.color.x);
            assert!(moon.color.z > moon.color.x);
            assert!(moon.color.z > sun.color.z);
            assert_eq!(sun.space, Space::Plane);
        }
        let dome = LightObject::from_fixture(
            fixtures
                .iter()
                .find(|fixture| fixture.name == "Купол")
                .unwrap(),
            Vec3::ZERO,
        );
        let areas = dome.areas();
        if !base.complete || base.solid {
            assert!(areas.iter().any(|area| matches!(area, Area::Volume(_))));
            assert!(areas.iter().any(|area| matches!(area, Area::Omni(_))));
            assert!(
                !areas
                    .iter()
                    .any(|area| matches!(area, Area::Rectangle(_) | Area::Disk(_))),
                "the hull faces would cut the room"
            );
        }
        let tube = fixtures
            .iter()
            .find(|fixture| fixture.name == "Люминесцент")
            .unwrap();
        assert!((tube.size.y - 63.0).abs() < 0.1);
        assert!(tube.size.y > tube.size.x * 5.0);
    }

    #[test]
    fn a_cage_glows_on_its_own_shell() {
        let fixture = relapse_fixtures()
            .iter()
            .find(|fixture| fixture.name == "Клетка")
            .expect("cage");
        let cage = LightObject::from_fixture(fixture, Vec3::ZERO);
        let glow = cage.figure_glow().expect("lamp");
        let base = crate::base::live();
        let expect_color = if base.complete {
            Vec3::from_array(base.color)
        } else {
            fixture.color
        };
        assert!(glow.contains(cage.position));
        assert!(glow.contains(cage.position + Vec3::X * (cage.size.x * 0.5)));
        assert!(!glow.contains(cage.position + Vec3::Z * (cage.size.z + 80.0)));
        assert!(close(glow.color, expect_color));
        assert!((glow.inside - base.inside / 100.0).abs() < 1.0e-4);
        assert!((glow.rim - base.rim / 100.0).abs() < 1.0e-4);
        assert!((glow.spread - base.spread / 100.0).abs() < 1.0e-4);
        assert!((glow.mesh - base.mesh / 100.0).abs() < 1.0e-4);
        let mut dark = cage.clone();
        dark.intensity = 0.0;
        assert!(dark.figure_glow().is_none());
        let volume = |object: &LightObject| {
            object
                .areas()
                .into_iter()
                .find_map(|area| match area {
                    Area::Volume(volume) => Some(volume.intensity),
                    _ => None,
                })
                .expect("volume")
        };
        let full = volume(&cage);
        dark.intensity = cage.intensity;
        dark.inside = 0.0;
        assert_eq!(volume(&dark), 0.0);
        dark.inside = 100.0;
        if cage.inside > 0.0 {
            assert!((volume(&dark) - full * (100.0 / cage.inside)).abs() < 1.0e-2);
        }
    }

    #[test]
    fn a_saved_base_replaces_the_light_and_keeps_its_place() {
        let mut object = LightObject::from_base(Base::Cube, Vec3::new(3.0, 4.0, 5.0));
        let place = object.position;
        let name = object.name.clone();
        let category = object.category.clone();
        apply_base(
            &mut object,
            &crate::base::Params {
                complete: true,
                solid: false,
                size: [9.0, 8.0, 7.0],
                rotation: [1.0, 2.0, 3.0],
                corners: 15.0,
                intensity: 500.0,
                color: [0.2, 0.3, 0.4],
                inside: 10.0,
                rim: 20.0,
                spread: 30.0,
                mesh: 40.0,
            },
        );
        assert_eq!(object.position, place);
        assert_eq!(object.name, name);
        assert_eq!(object.category, category);
        assert_eq!(object.space, Space::Plane);
        assert!(object.custom);
        assert!((object.size.x - 9.0).abs() < 1.0e-3);
        assert!((object.rotation.y - 2.0).abs() < 1.0e-3);
        assert!((object.corners - 15.0).abs() < 1.0e-3);
        assert!((object.intensity - 500.0).abs() < 1.0e-3);
        assert!(close(object.color, Vec3::new(0.2, 0.3, 0.4)));
        assert!((object.inside - 10.0).abs() < 1.0e-3);
        assert!((object.mesh - 40.0).abs() < 1.0e-3);
    }

    #[test]
    fn an_old_project_keeps_a_modest_interior() {
        let saved: SavedProject = serde_json::from_str(
            r#"{
                "map": "x",
                "categories": [],
                "objects": [{
                    "name": "Клетка",
                    "category": "",
                    "space": "3d",
                    "size": [4.0, 8.0, 13.0],
                    "position": [0.0, 0.0, 0.0],
                    "rotation": [0.0, 0.0, 0.0],
                    "corners": 25.0,
                    "intensity": 18000.0,
                    "color": [1.0, 1.0, 1.0]
                }]
            }"#,
        )
        .expect("old project");
        let object = saved.objects.into_iter().next().unwrap().into_object();
        let base = crate::base::live();
        assert_eq!(object.inside, base.inside);
        assert_eq!(object.rim, base.rim);
        assert_eq!(object.spread, base.spread);
        assert_eq!(object.mesh, base.mesh);
    }
}
