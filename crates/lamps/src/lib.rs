//! A lamp is a kind and a place. [`Lamp::area`] is the patch the solver sees.

use glam::Vec3;
use solve::{Area, Disk, Rectangle};

/// Fluorescent strength. Every other kind is a fraction of this.
const UNIT_INTENSITY: f32 = 18_000.0;
const WHITE: Vec3 = Vec3::ONE;
const RED: Vec3 = Vec3::new(1.0, 0.0, 0.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Fluorescent,
    Bulb,
    Sconce,
    Camera,
    Emergency,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Fluorescent,
        Kind::Bulb,
        Kind::Sconce,
        Kind::Camera,
        Kind::Emergency,
    ];

    /// Height of the patch in the hand-built room.
    pub fn height(self) -> f32 {
        match self {
            Kind::Fluorescent => 118.0,
            Kind::Bulb => 64.0,
            Kind::Sconce => 84.0,
            Kind::Camera => 108.0,
            Kind::Emergency => 96.0,
        }
    }

    fn share(self) -> f32 {
        match self {
            Kind::Fluorescent => 1.0,
            Kind::Bulb => 0.25,
            Kind::Sconce => 0.375,
            Kind::Camera => 0.125,
            Kind::Emergency => 0.1875,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Lamp {
    pub kind: Kind,
    pub position: Vec3,
    pub forward: Vec3,
}

/// Lamps stored beside the map. A missing file is an empty set.
pub fn load_beside(map: &std::path::Path) -> Vec<Lamp> {
    let path = map.with_extension("lamps");
    if !path.is_file() {
        return Vec::new();
    }
    read_lamps(&path)
}

fn read_lamps(path: &std::path::Path) -> Vec<Lamp> {
    // The bytes of that file are defined when it is written.
    let Ok(_bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    Vec::new()
}

impl Lamp {
    pub fn area(self) -> Area {
        let forward = horizontal(self.forward);
        let strength = UNIT_INTENSITY * self.kind.share();
        match self.kind {
            Kind::Fluorescent => {
                let side = Vec3::Z.cross(forward);
                Area::Rectangle(Rectangle {
                    center: self.position,
                    half_u: forward * 36.0,
                    half_v: side * 8.0,
                    normal: -Vec3::Z,
                    intensity: strength,
                    color: WHITE,
                })
            }
            Kind::Bulb => disk(self.position, 12.0, aim(forward, 37.0), strength, WHITE),
            Kind::Sconce => disk(self.position, 7.0, aim(forward, 72.0), strength, WHITE),
            Kind::Camera => disk(self.position, 3.5, aim(forward, 22.0), strength, WHITE),
            Kind::Emergency => disk(self.position, 5.0, aim(forward, 18.0), strength, RED),
        }
    }
}

fn disk(center: Vec3, radius: f32, normal: Vec3, intensity: f32, color: Vec3) -> Area {
    let normal = normal.normalize_or_zero();
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

fn horizontal(forward: Vec3) -> Vec3 {
    let flat = Vec3::new(forward.x, forward.y, 0.0);
    if flat.length_squared() <= 1.0e-8 {
        Vec3::X
    } else {
        flat.normalize()
    }
}

fn aim(forward: Vec3, tilt_degrees: f32) -> Vec3 {
    let tilt = tilt_degrees.to_radians();
    (-Vec3::Z * tilt.cos() + forward * tilt.sin()).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_lamp_file_is_an_empty_set() {
        let path = std::env::temp_dir().join("lightbaker-no-lamps.bsp");
        assert!(load_beside(&path).is_empty());
    }

    use solve::{solve, Area, Disk, Rectangle};

    fn placed(kind: Kind) -> Area {
        Lamp {
            kind,
            position: Vec3::new(80.0, 128.0, kind.height()),
            forward: Vec3::X,
        }
        .area()
    }

    fn as_rectangle(area: Area) -> Rectangle {
        match area {
            Area::Rectangle(rectangle) => rectangle,
            Area::Disk(_) | Area::Volume(_) | Area::Omni(_) => panic!("expected a rectangle"),
        }
    }

    fn as_disk(area: Area) -> Disk {
        match area {
            Area::Disk(disk) => disk,
            Area::Rectangle(_) | Area::Volume(_) | Area::Omni(_) => panic!("expected a disk"),
        }
    }

    fn tilt_from_down(normal: Vec3) -> f32 {
        normal.normalize().dot(-Vec3::Z).clamp(-1.0, 1.0).acos()
    }

    fn floor_hit(origin: Vec3, direction: Vec3) -> Vec3 {
        let direction = direction.normalize_or_zero();
        let t = -origin.z / direction.z;
        origin + direction * t
    }

    #[test]
    fn fluorescent_is_the_unit_and_the_rest_are_shares() {
        let unit = placed(Kind::Fluorescent).intensity();
        assert_eq!(unit, UNIT_INTENSITY);
        let mut seen = vec![unit];
        for kind in Kind::ALL.into_iter().skip(1) {
            let strength = placed(kind).intensity();
            assert!(strength < unit, "{kind:?} {strength}");
            assert!(
                seen.iter().all(|other| *other != strength),
                "{kind:?} repeats {strength}"
            );
            seen.push(strength);
        }
    }

    #[test]
    fn each_kind_has_its_own_form() {
        let fluorescent = as_rectangle(placed(Kind::Fluorescent));
        assert!(fluorescent.half_u.length() > fluorescent.half_v.length() * 3.0);
        assert!((fluorescent.normal + Vec3::Z).length() < 1.0e-4);

        let bulb = as_disk(placed(Kind::Bulb));
        let sconce = as_disk(placed(Kind::Sconce));
        let camera = as_disk(placed(Kind::Camera));
        let emergency = as_disk(placed(Kind::Emergency));
        assert!(bulb.radius > sconce.radius);
        assert!(sconce.radius > emergency.radius);
        assert!(emergency.radius > camera.radius);

        let bulb_tilt = tilt_from_down(bulb.normal);
        let sconce_tilt = tilt_from_down(sconce.normal);
        let camera_tilt = tilt_from_down(camera.normal);
        let emergency_tilt = tilt_from_down(emergency.normal);
        assert!(sconce_tilt > bulb_tilt);
        assert!(bulb_tilt > camera_tilt);
        assert!((emergency_tilt - camera_tilt).abs() > 0.05);
    }

    #[test]
    fn only_the_emergency_is_red() {
        for kind in Kind::ALL {
            let color = placed(kind).color();
            if kind == Kind::Emergency {
                assert!(color.x > 0.9 && color.y < 0.05 && color.z < 0.05);
            } else {
                assert_eq!(color, WHITE);
            }
        }
    }

    #[test]
    fn fluorescent_matches_the_room_fixture() {
        let lamp = as_rectangle(placed(Kind::Fluorescent));
        let Area::Rectangle(fixture) = solve::room().areas[0] else {
            panic!("fixture light is a rectangle");
        };
        assert!((lamp.center - fixture.center).length() < 1.0e-4);
        assert!((lamp.half_u - fixture.half_u).length() < 1.0e-4);
        assert!((lamp.half_v - fixture.half_v).length() < 1.0e-4);
        assert!((lamp.normal - fixture.normal).length() < 1.0e-4);
        assert_eq!(lamp.intensity, fixture.intensity);
        assert_eq!(lamp.color, fixture.color);
    }

    #[test]
    fn bulb_center_ray_meets_the_floor_in_front_of_the_bars() {
        // The mock grate stands on x = 150. The disk sits against those bars.
        let lamp = Lamp {
            kind: Kind::Bulb,
            position: Vec3::new(146.0, 128.0, 64.0),
            forward: -Vec3::X,
        };
        let disk = as_disk(lamp.area());
        assert!((disk.center - lamp.position).length() < 1.0e-4);
        let hit = floor_hit(disk.center, disk.normal);
        assert!(hit.x < 150.0, "hit x {}", hit.x);
        assert!(hit.x > 16.0, "hit x {}", hit.x);
        assert!((hit.y - 128.0).abs() < 1.0e-3, "{}", hit.y);
    }

    #[test]
    fn bulb_from_the_open_floor_lands_before_the_bars() {
        let lamp = Lamp {
            kind: Kind::Bulb,
            position: Vec3::new(80.0, 128.0, Kind::Bulb.height()),
            forward: Vec3::X,
        };
        let disk = as_disk(lamp.area());
        let hit = floor_hit(disk.center, disk.normal);
        assert!(hit.x > disk.center.x, "{}", hit.x);
        assert!(hit.x < 150.0, "{}", hit.x);
    }

    #[test]
    fn kinds_change_the_light_on_the_same_room() {
        let fixture = solve::room();
        let receivers: Vec<_> = fixture.luxels.iter().map(|luxel| luxel.receiver).collect();
        let probe = fixture
            .luxels
            .iter()
            .position(|luxel| {
                let point = luxel.receiver.position;
                (point.x - 88.0).abs() < 0.1 && (point.y - 136.0).abs() < 0.1
            })
            .expect("luxel");

        let mut lights = Vec::new();
        let mut emergency = [0.0; 3];
        for kind in Kind::ALL {
            let lamp = Lamp {
                kind,
                position: Vec3::new(80.0, 128.0, kind.height()),
                forward: Vec3::X,
            };
            let areas = [lamp.area(), fixture.areas[1]];
            let light = solve(&fixture.triangles, &receivers, &areas, 16).light;
            if kind == Kind::Emergency {
                emergency = light[probe];
            }
            lights.push(light);
        }

        for left in 0..lights.len() {
            for right in (left + 1)..lights.len() {
                assert_ne!(lights[left], lights[right], "{left} vs {right}");
            }
        }

        assert!(emergency[0] > emergency[1] * 2.0, "{emergency:?}");
        assert!(emergency[0] > emergency[2] * 2.0, "{emergency:?}");
    }
}
