use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::Vec3;
use solve::{solve, Area, Rectangle, Role};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::{open, open_reporting, Error, LoadPhase};

#[test]
fn foreign_bytes_do_not_open() {
    let path = temp("lightbaker-not-a-map.bin");
    std::fs::write(&path, b"not a bsp").unwrap();
    assert!(matches!(open(&path), Err(Error::NotAMap)));
}

#[test]
fn a_map_without_a_luxel_grid_does_not_open() {
    let path = temp("lightbaker-noluxel.bsp");
    std::fs::write(&path, room_bsp(-1, Vec::new())).unwrap();
    assert!(matches!(open(&path), Err(Error::NoLuxelGrid)));
}

#[test]
fn opening_reports_each_stage_it_finishes() {
    let path = temp("lightbaker-load.bsp");
    std::fs::write(&path, room_bsp(0, wall_pak())).unwrap();
    let seen = AtomicU64::new(0);
    let map = open_reporting(&path, &|phase, done, total| {
        assert!(total > 0);
        assert!(done <= total);
        seen.fetch_or(1 << phase as u64, Ordering::Relaxed);
    })
    .unwrap();
    assert!(!map.luxels.is_empty());
    let bits = seen.load(Ordering::Relaxed);
    assert_ne!(bits & (1 << LoadPhase::File as u64), 0);
    assert_ne!(bits & (1 << LoadPhase::World as u64), 0);
    assert_ne!(bits & (1 << LoadPhase::Props as u64), 0);
}

#[test]
fn luxels_sit_on_the_face_grid_and_pak_color_wins() {
    let path = temp("lightbaker-room.bsp");
    std::fs::write(&path, room_bsp(0, wall_pak())).unwrap();
    let map = open(&path).unwrap();

    assert_eq!(map.luxels.len(), 8);
    assert_eq!(map.triangles.len(), 4);
    assert_eq!(map.surface.len(), 4);
    assert!((map.surface[0].albedo - Vec3::Y).length() < 1.0e-3);
    assert!((map.surface[2].albedo - Vec3::X).length() < 1.0e-3);

    // The floor fills its lightmap exactly, so each luxel is sampled at the
    // center of its texel.
    let floor = &map.luxels[0];
    assert_eq!(floor.receiver.role, Role::Floor);
    assert!((floor.receiver.position - Vec3::new(8.0, 8.0, 0.0)).length() < 1.0e-3);
    assert!((floor.receiver.albedo - Vec3::Y).length() < 1.0e-3);
    let inner = &map.luxels[3];
    assert!((inner.receiver.position - Vec3::new(24.0, 24.0, 0.0)).length() < 1.0e-3);

    let wall = &map.luxels[4];
    assert_eq!(wall.receiver.role, Role::Wall);
    assert!((wall.receiver.position - Vec3::new(8.0, 0.0, 8.0)).length() < 1.0e-3);
    assert!(
        (wall.receiver.albedo - Vec3::X).length() < 1.0e-3,
        "albedo {}",
        wall.receiver.albedo
    );
    assert_eq!(map.snapshot.faces[0].width, 2);
    assert_eq!(map.snapshot.faces[0].luxel_count, 4);
    let floor = map
        .surface
        .iter()
        .find(|tri| (tri.albedo - Vec3::Y).length() < 1.0e-3)
        .expect("floor triangle");
    assert_eq!(floor.light_face, 0);
    assert!(floor
        .light_uv
        .iter()
        .any(|uv| uv[0].abs() < 1.0e-3 && uv[1].abs() < 1.0e-3));
    assert!(floor
        .light_uv
        .iter()
        .any(|uv| (uv[0] - 2.0).abs() < 1.0e-3 && (uv[1] - 2.0).abs() < 1.0e-3));
    assert_eq!(map.snapshot.bytes, std::fs::read(&path).unwrap());
}

#[test]
fn nodraw_blocks_rays_and_stays_off_the_view() {
    let path = temp("lightbaker-nodraw.bsp");
    let mut bytes = room_bsp(0, wall_pak());
    flag_texinfo(&mut bytes, 1, 0x0080);
    std::fs::write(&path, &bytes).unwrap();
    let map = open(&path).unwrap();
    assert_eq!(map.triangles.len(), 4);
    assert_eq!(map.surface.len(), 2);
    assert!(map
        .surface
        .iter()
        .all(|tri| (tri.albedo - Vec3::Y).length() < 1.0e-3));
}

#[test]
fn glass_stays_on_the_view_and_does_not_block() {
    let path = temp("lightbaker-glass.bsp");
    let mut bytes = room_bsp(0, wall_pak());
    flag_texinfo(&mut bytes, 1, 0x0010);
    std::fs::write(&path, &bytes).unwrap();
    let map = open(&path).unwrap();
    assert_eq!(map.triangles.len(), 2);
    assert_eq!(map.surface.len(), 4);
}

#[test]
fn a_luxel_cut_by_a_wall_is_sampled_on_the_lit_side() {
    let path = temp("lightbaker-cut.bsp");
    std::fs::write(&path, cut_bsp()).unwrap();
    let map = open(&path).unwrap();
    assert_eq!(map.luxels.len(), 9);

    // Texel (0, 0) is centered at world (8, 8), outside the floor, which
    // starts at x = 12. The part of the texel on the floor is sampled instead.
    let luxel = &map.luxels[0];
    assert!(
        (luxel.receiver.position - Vec3::new(14.0, 8.0, 0.0)).length() < 1.0e-2,
        "{}",
        luxel.receiver.position
    );
    assert!(luxel.receiver.normal.z > 0.7, "{:?}", luxel.receiver.normal);
    assert!(
        luxel.corners.len() >= 3 && luxel.corners.iter().all(|corner| corner.x >= 12.0 - 1.0e-3),
        "{:?}",
        luxel.corners
    );

    let lamp = Area::Rectangle(Rectangle {
        center: Vec3::new(24.0, 16.0, 48.0),
        half_u: Vec3::new(4.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 4.0, 0.0),
        normal: -Vec3::Z,
        intensity: 8_000.0,
        color: Vec3::ONE,
    });
    let lit = solve(&map.triangles, &[luxel.receiver], &[lamp], 16);
    assert!(lit.light[0][0] > 0.0, "cut luxel {:?}", lit.light[0]);

    let mut behind = luxel.receiver;
    behind.position = Vec3::new(8.0, 8.0, 0.0);
    let dark = solve(&map.triangles, &[behind], &[lamp], 16);
    assert_eq!(
        dark.light[0],
        [0.0, 0.0, 0.0],
        "outside the floor {:?}",
        dark.light[0]
    );
}

#[test]
fn gm_construct_opens_and_feeds_the_same_solve() {
    let Some(path) = construct_path() else {
        return;
    };
    let map = open(&path).expect("gm_construct");
    let face_end = face_sample_end(&map);
    assert!(face_end > 500_000, "face luxels {face_end}");
    assert!(
        map.luxels.len() > face_end,
        "prop receivers {} after {face_end} faces",
        map.luxels.len()
    );
    assert!(
        !map.snapshot.props.is_empty(),
        "static props did not become receivers"
    );
    assert!(
        map.triangles.len() > 80_000 && map.triangles.len() < 800_000,
        "triangles {}",
        map.triangles.len()
    );
    assert!(map.snapshot.lighting.length > 0);
    assert_eq!(
        map.snapshot.bytes.len(),
        std::fs::metadata(&path).unwrap().len() as usize
    );

    let faces = &map.luxels[..face_end];
    let floor = nearest(faces, Vec3::new(823.0, -32.0, -148.0));
    assert_eq!(
        floor.receiver.role,
        Role::Floor,
        "{}",
        floor.receiver.position
    );
    assert!(
        (floor.receiver.position.z + 148.0).abs() < 4.0,
        "{}",
        floor.receiver.position
    );
    assert!(floor.receiver.albedo.length_squared() > 0.01);
    assert!(floor.receiver.albedo.max_element() <= 1.0);

    let hill = nearest_xy(faces, -3360.0, 3430.0);
    assert!(
        hill.receiver.position.z > -135.0,
        "displacement stayed on the base plane at {}",
        hill.receiver.position
    );

    let anchor = Vec3::new(823.0, -32.0, -100.0);
    let mut order: Vec<usize> = (0..face_end).collect();
    order.sort_by(|&left, &right| {
        let dl = (map.luxels[left].receiver.position - anchor).length_squared();
        let dr = (map.luxels[right].receiver.position - anchor).length_squared();
        dl.total_cmp(&dr)
    });
    order.truncate(200);
    let receivers: Vec<_> = order
        .iter()
        .map(|&index| map.luxels[index].receiver)
        .collect();
    let area = Area::Rectangle(Rectangle {
        center: anchor,
        half_u: Vec3::new(36.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 8.0, 0.0),
        normal: -Vec3::Z,
        intensity: 18_000.0,
        color: Vec3::ONE,
    });
    let once = solve(&map.triangles, &receivers, &[area], 16);
    let twice = solve(&map.triangles, &receivers, &[area], 16);
    assert_eq!(once, twice);
    assert!(once.light.iter().any(|color| color[0] > 0.0));
}

#[test]
fn a_wall_facing_into_its_slab_is_turned_toward_the_lamp() {
    let mut triangles = Vec::new();
    triangles.extend(wall_quad(0.0, 1.0));
    triangles.extend(wall_quad(32.0, -1.0));
    triangles.extend(wall_quad(64.0, -1.0));
    let inward = receiver(Vec3::new(12.0, 0.0, 4.0), Vec3::Y);
    let outward = receiver(Vec3::new(12.0, 32.0, 4.0), Vec3::Y);
    let mut luxels = vec![
        crate::Luxel {
            receiver: inward,
            corners: Vec::new(),
        },
        crate::Luxel {
            receiver: outward,
            corners: Vec::new(),
        },
    ];
    let lamp = Area::Rectangle(Rectangle {
        center: Vec3::new(12.0, -24.0, 20.0),
        half_u: Vec3::new(2.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 2.0, 0.0),
        normal: -Vec3::Z,
        intensity: 4_000.0,
        color: Vec3::ONE,
    });
    let dark = solve(&triangles, &[inward], &[lamp], 16);
    assert_eq!(dark.light[0], [0.0, 0.0, 0.0]);
    crate::bsp::turn_faces(&triangles, &mut luxels, &[(0, 1), (1, 1)]);
    assert!(
        luxels[0].receiver.normal.y < -0.5,
        "near side {:?}",
        luxels[0].receiver.normal
    );
    assert!(
        luxels[1].receiver.normal.y > 0.5,
        "far side {:?}",
        luxels[1].receiver.normal
    );
    let lit = solve(&triangles, &[luxels[0].receiver], &[lamp], 16);
    assert!(lit.light[0][0] > 0.0, "{:?}", lit.light[0]);
}

#[test]
fn a_floor_under_a_low_ceiling_keeps_facing_up() {
    let ceiling = solve::Triangle {
        vertices: [
            Vec3::new(0.0, 0.0, 32.0),
            Vec3::new(16.0, 0.0, 32.0),
            Vec3::new(16.0, 16.0, 32.0),
        ],
    };
    let mut luxels = vec![crate::Luxel {
        receiver: receiver(Vec3::new(12.0, 4.0, 0.0), Vec3::Z),
        corners: Vec::new(),
    }];
    crate::bsp::turn_faces(&[ceiling], &mut luxels, &[(0, 1)]);
    assert!(luxels[0].receiver.normal.z > 0.9);
}

#[test]
fn construct_displacements_stay_on_their_grid() {
    let Some(path) = construct_path() else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let mut ratios = crate::bsp::terrain_stretch(&bytes);
    ratios.sort_by(|left, right| left.total_cmp(right));
    let p99 = ratios[ratios.len() * 99 / 100];
    assert!(p99 < 3.0, "displacement edges stretch {p99:.1} grid steps");
    let seam = crate::bsp::shared_seam_median(&bytes);
    assert!(seam < 8.0, "displacement neighbors stay {seam:.1} apart");
}

#[test]
fn construct_wall_beside_the_lamp_is_lit() {
    let Some(path) = construct_path() else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let map = crate::bsp::assemble(&bytes).unwrap();
    let wall_samples: Vec<_> = map
        .luxels
        .iter()
        .filter(|luxel| {
            let position = luxel.receiver.position;
            (position.x - 1472.0).abs() < 80.0
                && (position.y + 1056.0).abs() < 2.0
                && position.z < -48.0
                && luxel.receiver.normal.y < -0.5
        })
        .map(|luxel| luxel.receiver)
        .collect();
    assert!(!wall_samples.is_empty(), "wall facing the lamp");
    let floor = map
        .luxels
        .iter()
        .find(|luxel| {
            let position = luxel.receiver.position;
            (position.x - 1472.0).abs() < 16.0
                && (position.y + 1315.0).abs() < 16.0
                && (position.z + 144.0).abs() < 1.0
        })
        .map(|luxel| luxel.receiver)
        .expect("floor under the lamp");
    assert!(floor.normal.z > 0.7, "{:?}", floor.normal);
    let area = Area::Rectangle(Rectangle {
        center: Vec3::new(1472.0, -1315.45, -48.0),
        half_u: Vec3::new(36.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 8.0, 0.0),
        normal: -Vec3::Z,
        intensity: 18_000.0,
        color: Vec3::ONE,
    });
    let mut receivers = wall_samples;
    receivers.push(floor);
    let lit = solve(&map.triangles, &receivers, &[area], 16);
    let wall_light = &lit.light[..lit.light.len() - 1];
    let lit_count = wall_light.iter().filter(|color| color[0] > 0.0).count();
    let brightest = wall_light
        .iter()
        .map(|color| color[0])
        .fold(0.0f32, f32::max);
    assert!(
        lit_count * 4 > wall_light.len() * 3,
        "lit {lit_count} of {} brightest {brightest}",
        wall_light.len()
    );
    assert!(
        lit.light.last().unwrap()[0] > 0.0,
        "floor {:?}",
        lit.light.last()
    );
}

fn wall_quad(y: f32, facing_y: f32) -> [solve::Triangle; 2] {
    let a = Vec3::new(0.0, y, 0.0);
    let b = Vec3::new(16.0, y, 0.0);
    let c = Vec3::new(16.0, y, 16.0);
    let d = Vec3::new(0.0, y, 16.0);
    if facing_y >= 0.0 {
        [
            solve::Triangle {
                vertices: [a, d, c],
            },
            solve::Triangle {
                vertices: [a, c, b],
            },
        ]
    } else {
        [
            solve::Triangle {
                vertices: [a, b, c],
            },
            solve::Triangle {
                vertices: [a, c, d],
            },
        ]
    }
}

fn receiver(position: Vec3, normal: Vec3) -> solve::Receiver {
    solve::Receiver {
        position,
        normal,
        albedo: Vec3::splat(0.5),
        role: Role::Wall,
    }
}

fn face_sample_end(map: &crate::Map) -> usize {
    map.snapshot
        .faces
        .iter()
        .map(|face| face.first_luxel as usize + face.luxel_count as usize)
        .max()
        .unwrap_or(0)
}

fn nearest(luxels: &[crate::Luxel], point: Vec3) -> &crate::Luxel {
    luxels
        .iter()
        .min_by(|left, right| {
            (left.receiver.position - point)
                .length_squared()
                .total_cmp(&(right.receiver.position - point).length_squared())
        })
        .expect("luxel")
}

fn nearest_xy(luxels: &[crate::Luxel], x: f32, y: f32) -> &crate::Luxel {
    luxels
        .iter()
        .min_by(|left, right| {
            let dl =
                (left.receiver.position.x - x).powi(2) + (left.receiver.position.y - y).powi(2);
            let dr =
                (right.receiver.position.x - x).powi(2) + (right.receiver.position.y - y).powi(2);
            dl.total_cmp(&dr)
        })
        .expect("luxel")
}

#[test]
fn a_jail_ceiling_faces_the_room() {
    let path = PathBuf::from(
        r"D:\Steam\steamapps\common\GarrysMod\garrysmod\download\maps\relapse_jail.bsp",
    );
    if !path.exists() {
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let map = crate::bsp::assemble(&bytes).unwrap();
    let luxel = nearest(&map.luxels, Vec3::new(796.0, 455.3, 128.0));
    assert!(
        (luxel.receiver.position - Vec3::new(796.0, 455.3, 128.0)).length() < 32.0,
        "{}",
        luxel.receiver.position
    );
    assert!(
        luxel.receiver.normal.z < -0.5,
        "ceiling {:?}",
        luxel.receiver.normal
    );
}

#[test]
fn a_jail_door_keeps_its_model_texture() {
    let path = PathBuf::from(
        r"D:\Steam\steamapps\common\GarrysMod\garrysmod\download\maps\relapse_jail.bsp",
    );
    if !path.exists() {
        return;
    }
    let bytes = std::fs::read(&path).unwrap();
    let offset = i32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let length = i32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let entities = String::from_utf8_lossy(&bytes[offset..offset + length]);
    let mut origin = None;
    for block in entities.split('{').skip(1) {
        let body = block.split('}').next().unwrap_or("");
        if !body.contains("prop_door_rotating") {
            continue;
        }
        let mut quotes = body.match_indices('"').map(|(index, _)| index);
        while let Some(start) = quotes.next() {
            let Some(stop) = quotes.next() else { break };
            if &body[start + 1..stop] != "origin" {
                continue;
            }
            let Some(value_start) = quotes.next() else {
                break;
            };
            let Some(value_end) = quotes.next() else {
                break;
            };
            let mut parts = body[value_start + 1..value_end].split_whitespace();
            origin = Some(Vec3::new(
                parts.next().unwrap().parse().unwrap(),
                parts.next().unwrap().parse().unwrap(),
                parts.next().unwrap().parse().unwrap(),
            ));
            break;
        }
        if origin.is_some() {
            break;
        }
    }
    let origin = origin.expect("door");
    let map = open(&path).unwrap();
    let mut faces: Vec<&crate::Surface> = Vec::new();
    faces.extend(map.surface.iter());
    for door in &map.doors {
        faces.extend(door.closed.surface.iter());
    }
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let mut hinged = false;
    for face in faces {
        let textured = face.albedo == Vec3::splat(0.62)
            && map
                .materials
                .get(face.material as usize)
                .and_then(|material| material.texture)
                .is_some();
        if !textured {
            continue;
        }
        if !face
            .vertices
            .iter()
            .any(|point| point.distance(origin) < 160.0)
        {
            continue;
        }
        if face
            .vertices
            .iter()
            .any(|point| point.distance(origin) < 4.0)
        {
            hinged = true;
        }
        for vertex in face.vertices {
            if vertex.distance(origin) > 160.0 {
                continue;
            }
            min = min.min(vertex);
            max = max.max(vertex);
        }
    }
    let span = max - min;
    assert!(
        hinged,
        "door at {origin:?} has no textured hinge, span {span:?}"
    );
    assert!(
        span.z >= 80.0 && (span.x >= 40.0 || span.y >= 40.0),
        "door at {origin:?} is not a slab, span {span:?}"
    );
}

#[test]
fn construct_grass_uses_the_real_texture() {
    let Some(path) = construct_path() else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let assembled = crate::bsp::assemble(&bytes).unwrap();
    let used: Vec<u32> = assembled.surface.iter().map(|face| face.material).collect();
    let (materials, images) =
        crate::picture::Catalog::world(&bytes, &path, &assembled.names, &used, &|_, _, _| {})
            .finish();
    let grass = images
        .iter()
        .position(|image| image.width == 2048 && image.mips[0].starts_with(&[55, 64, 24, 255]))
        .expect("grass texture");
    assert_eq!(images[grass].mips[0].len(), 2048 * 2048 * 4);
    let material = materials
        .iter()
        .position(|material| material.texture == Some(grass as u32))
        .expect("grass material");
    let face = assembled
        .surface
        .iter()
        .find(|face| face.material == material as u32)
        .expect("grass face");
    let span = (face.uv[0][0] - face.uv[1][0]).hypot(face.uv[0][1] - face.uv[1][1]);
    assert!(span > 8.0, "texture axes {span}");
    assert!(assembled
        .surface
        .iter()
        .any(|face| face.blend.iter().any(|blend| *blend > 0.9)));
}

#[test]
fn construct_color_canvases_stay_off_the_view() {
    let Some(path) = construct_path() else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let assembled =
        crate::bsp::assemble_reporting(&bytes, Some(path.as_path()), &|_, _, _| {}).unwrap();
    assert!(
        assembled
            .surface
            .iter()
            .all(|face| !is_color_canvas(&face.vertices)),
        "paint canvas is still drawn"
    );
    assert!(
        assembled
            .triangles
            .iter()
            .all(|face| !is_color_canvas(&face.vertices)),
        "paint canvas still blocks light"
    );
}

#[test]
fn displacement_light_stays_on_the_luxel_grid() {
    let Some(path) = construct_path() else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    let map = crate::bsp::assemble(&bytes).unwrap();
    let mut count = vec![0u32; map.faces.len()];
    for tri in &map.surface {
        if tri.light_face != u32::MAX {
            count[tri.light_face as usize] += 1;
        }
    }
    for tri in &map.surface {
        if tri.light_face == u32::MAX {
            continue;
        }
        let face = &map.faces[tri.light_face as usize];
        if face.width == 0 || face.height == 0 {
            continue;
        }
        let displaced = count[tri.light_face as usize] >= 32;
        let (lo, hi_u, hi_v) = if displaced {
            (0.4, face.width as f32 - 0.4, face.height as f32 - 0.4)
        } else {
            (-0.51, face.width as f32 + 0.51, face.height as f32 + 0.51)
        };
        for uv in tri.light_uv {
            assert!(
                (lo..hi_u).contains(&uv[0]) && (lo..hi_v).contains(&uv[1]),
                "face {} grid {}x{} light uv {uv:?}",
                tri.light_face,
                face.width,
                face.height
            );
        }
    }
}

fn is_color_canvas(vertices: &[glam::Vec3; 3]) -> bool {
    let thin_wall = vertices.iter().all(|point| point.x.abs() < 30.0)
        && span(vertices, |point| point.y) > 400.0
        && vertices.iter().all(|point| point.z.abs() < 250.0);
    let sheet = vertices
        .iter()
        .all(|point| (point.z.abs() - 4.0).abs() < 1.0)
        && span(vertices, |point| point.x) > 1000.0
        && span(vertices, |point| point.y) > 800.0;
    thin_wall || sheet
}

fn span(vertices: &[glam::Vec3; 3], axis: impl Fn(glam::Vec3) -> f32) -> f32 {
    let mut low = f32::MAX;
    let mut high = f32::MIN;
    for point in vertices {
        let value = axis(*point);
        low = low.min(value);
        high = high.max(value);
    }
    high - low
}

fn construct_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("LIGHTBAKER_MAP") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let path =
        PathBuf::from(r"D:\Steam\steamapps\common\GarrysMod\garrysmod\maps\gm_construct.bsp");
    path.is_file().then_some(path)
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

fn wall_pak() -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("materials/room/wall.vmt", opts).unwrap();
    zip.write_all(br#""LightmappedGeneric" { "$color" "[1 0 0]" }"#)
        .unwrap();
    zip.finish().unwrap().into_inner()
}

/// A 32×32 floor and a wall. `light_offset` -1 means the file has no luxel grid.
fn room_bsp(light_offset: i32, pak: Vec<u8>) -> Vec<u8> {
    room_bin(light_offset, pak).finish()
}

fn room_bin(light_offset: i32, pak: Vec<u8>) -> Bin {
    let mut bin = Bin::default();
    bin.lump(1, pack([plane(Vec3::Z, 0.0), plane(Vec3::Y, 0.0)]));
    bin.lump(
        3,
        pack([
            vec3(0.0, 0.0, 0.0),
            vec3(32.0, 0.0, 0.0),
            vec3(32.0, 32.0, 0.0),
            vec3(0.0, 32.0, 0.0),
            vec3(0.0, 0.0, 0.0),
            vec3(32.0, 0.0, 0.0),
            vec3(32.0, 0.0, 32.0),
            vec3(0.0, 0.0, 32.0),
        ]),
    );
    let mut edge_bytes = Vec::new();
    for (a, b) in [
        (0u16, 1u16),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
    ] {
        edge_bytes.extend(a.to_le_bytes());
        edge_bytes.extend(b.to_le_bytes());
    }
    bin.lump(12, edge_bytes);
    let mut surf = Vec::new();
    for index in 0..8i32 {
        surf.extend(index.to_le_bytes());
    }
    bin.lump(13, surf);

    let floor_ofs = if light_offset < 0 { -1 } else { 0 };
    let wall_ofs = if light_offset < 0 { -1 } else { 16 };
    bin.lump(
        7,
        pack([face(0, 0, 0, 0, floor_ofs), face(1, 4, 1, 1, wall_ofs)]),
    );
    bin.lump(
        6,
        pack([
            texinfo([1.0 / 16.0, 0.0, 0.0, 0.0], [0.0, 1.0 / 16.0, 0.0, 0.0], 0),
            texinfo([1.0 / 16.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0 / 16.0, 0.0], 1),
        ]),
    );
    bin.lump(
        2,
        pack([texdata(Vec3::Y, 0), texdata(Vec3::new(0.0, 0.0, 1.0), 1)]),
    );
    let mut strings = b"room/floor\0room/wall\0".to_vec();
    bin.lump(43, strings.split_off(0));
    let floor_len = "room/floor\0".len() as i32;
    bin.lump(44, pack([0i32.to_le_bytes(), floor_len.to_le_bytes()]));
    bin.lump(8, vec![0u8; 32]);
    bin.lump(53, vec![0u8; 32]);
    bin.lump(40, pak);
    bin.lump(14, model());
    bin
}

fn plane(normal: Vec3, dist: f32) -> [u8; 20] {
    let mut out = [0u8; 20];
    out[..12].copy_from_slice(&vec3(normal.x, normal.y, normal.z));
    out[12..16].copy_from_slice(&dist.to_le_bytes());
    out
}

/// Floor from x = 12 and a wall on that edge. The first luxel's center falls
/// on the solid side of the wall; only the rest of its square lies on the floor.
fn cut_bsp() -> Vec<u8> {
    let mut bin = Bin::default();
    bin.lump(1, pack([plane(Vec3::Z, 0.0), plane(Vec3::X, 12.0)]));
    bin.lump(
        3,
        pack([
            vec3(12.0, 0.0, 0.0),
            vec3(32.0, 0.0, 0.0),
            vec3(32.0, 32.0, 0.0),
            vec3(12.0, 32.0, 0.0),
            vec3(12.0, 0.0, 32.0),
            vec3(12.0, 32.0, 32.0),
        ]),
    );
    let mut edge_bytes = Vec::new();
    for (a, b) in [
        (0u16, 1u16),
        (1, 2),
        (2, 3),
        (3, 0),
        (0, 4),
        (4, 5),
        (5, 3),
        (3, 0),
    ] {
        edge_bytes.extend(a.to_le_bytes());
        edge_bytes.extend(b.to_le_bytes());
    }
    bin.lump(12, edge_bytes);
    let mut surf = Vec::new();
    for index in 0..8i32 {
        surf.extend(index.to_le_bytes());
    }
    bin.lump(13, surf);
    bin.lump(
        7,
        pack([
            face_grid(0, 0, 4, 0, 0, [0, 0], [2, 2]),
            face_grid(1, 4, 4, 0, -1, [0, 0], [0, 0]),
        ]),
    );
    bin.lump(
        6,
        pack([texinfo(
            [1.0 / 16.0, 0.0, 0.0, 0.0],
            [0.0, 1.0 / 16.0, 0.0, 0.0],
            0,
        )]),
    );
    bin.lump(2, pack([texdata(Vec3::ONE, 0)]));
    bin.lump(43, b"room/floor\0".to_vec());
    bin.lump(44, pack([0i32.to_le_bytes()]));
    bin.lump(8, vec![0u8; 64]);
    bin.lump(53, vec![0u8; 64]);
    bin.lump(40, Vec::new());
    bin.lump(14, model());
    bin.finish()
}

fn face(plane: u16, first_edge: i32, texinfo: i16, tex_unused: i16, light_offset: i32) -> [u8; 56] {
    let _ = tex_unused;
    face_grid(plane, first_edge, 4, texinfo, light_offset, [0, 0], [1, 1])
}

fn face_grid(
    plane: u16,
    first_edge: i32,
    edges: i16,
    texinfo: i16,
    light_offset: i32,
    mins: [i32; 2],
    sizes: [i32; 2],
) -> [u8; 56] {
    let mut out = [0u8; 56];
    out[..2].copy_from_slice(&plane.to_le_bytes());
    out[4..8].copy_from_slice(&first_edge.to_le_bytes());
    out[8..10].copy_from_slice(&edges.to_le_bytes());
    out[10..12].copy_from_slice(&texinfo.to_le_bytes());
    out[12..14].copy_from_slice(&(-1i16).to_le_bytes());
    out[14..16].copy_from_slice(&(-1i16).to_le_bytes());
    out[16] = 0;
    out[17] = 255;
    out[18] = 255;
    out[19] = 255;
    out[20..24].copy_from_slice(&light_offset.to_le_bytes());
    out[24..28].copy_from_slice(&1024f32.to_le_bytes());
    out[28..32].copy_from_slice(&mins[0].to_le_bytes());
    out[32..36].copy_from_slice(&mins[1].to_le_bytes());
    out[36..40].copy_from_slice(&sizes[0].to_le_bytes());
    out[40..44].copy_from_slice(&sizes[1].to_le_bytes());
    out
}

fn flag_texinfo(bytes: &mut [u8], index: usize, flags: i32) {
    let at = 8 + 6 * 16;
    let offset = i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let pos = offset + index * 72 + 64;
    bytes[pos..pos + 4].copy_from_slice(&flags.to_le_bytes());
}

fn texinfo(s: [f32; 4], t: [f32; 4], texdata: i32) -> [u8; 72] {
    let mut out = [0u8; 72];
    for (index, value) in s.into_iter().chain(t).enumerate() {
        out[32 + index * 4..36 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    out[68..72].copy_from_slice(&texdata.to_le_bytes());
    out
}

fn texdata(reflectivity: Vec3, name: i32) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..12].copy_from_slice(&vec3(reflectivity.x, reflectivity.y, reflectivity.z));
    out[12..16].copy_from_slice(&name.to_le_bytes());
    out
}

fn model() -> Vec<u8> {
    let mut out = vec![0u8; 48];
    out[12..24].copy_from_slice(&vec3(32.0, 32.0, 32.0));
    out[44..48].copy_from_slice(&2i32.to_le_bytes());
    out
}

#[test]
fn a_brush_door_blocks_when_closed_and_slides_when_open() {
    let mut bin = room_bin(0, Vec::new());
    bin.lump(
        0,
        b"{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"origin\" \"16 0 16\"\n\"movedir\" \"1 0 0\"\n\"targetname\" \"gate\"\n\"hammerid\" \"15\"\n}\n"
            .to_vec(),
    );
    let mut models = dmodel([0.0, 0.0, 0.0], [32.0, 32.0, 32.0], [16.0, 16.0, 0.0], 0, 1).to_vec();
    models.extend(dmodel(
        [0.0, 0.0, 0.0],
        [32.0, 0.0, 32.0],
        [16.0, 0.0, 16.0],
        1,
        1,
    ));
    bin.lump(14, models);
    let map = crate::bsp::assemble(&bin.finish()).unwrap();
    assert_eq!(map.doors.len(), 1);
    assert_eq!(map.doors[0].name, "gate");
    assert_eq!(map.doors[0].id, "func_door#15");
    for triangle in &map.triangles {
        for point in triangle.vertices {
            assert!(point.z.abs() < 1.0e-3, "world kept the door: {point:?}");
        }
    }
    let closed = max_x(&map.doors[0].closed.triangles);
    let open = max_x(&map.doors[0].open.triangles);
    assert!(open > closed + 16.0, "closed {closed} open {open}");
    assert!(map.doors[0]
        .closed
        .triangles
        .iter()
        .any(|triangle| triangle.vertices.iter().any(|point| point.z > 16.0)));
}

fn max_x(triangles: &[solve::Triangle]) -> f32 {
    triangles
        .iter()
        .flat_map(|triangle| triangle.vertices)
        .map(|point| point.x)
        .fold(f32::MIN, f32::max)
}

fn dmodel(mins: [f32; 3], maxs: [f32; 3], origin: [f32; 3], first: i32, count: i32) -> [u8; 48] {
    let mut out = [0u8; 48];
    out[..12].copy_from_slice(&vec3(mins[0], mins[1], mins[2]));
    out[12..24].copy_from_slice(&vec3(maxs[0], maxs[1], maxs[2]));
    out[24..36].copy_from_slice(&vec3(origin[0], origin[1], origin[2]));
    out[40..44].copy_from_slice(&first.to_le_bytes());
    out[44..48].copy_from_slice(&count.to_le_bytes());
    out
}

fn vec3(x: f32, y: f32, z: f32) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[..4].copy_from_slice(&x.to_le_bytes());
    out[4..8].copy_from_slice(&y.to_le_bytes());
    out[8..].copy_from_slice(&z.to_le_bytes());
    out
}

fn pack<const N: usize, const M: usize>(parts: [[u8; N]; M]) -> Vec<u8> {
    parts.into_iter().flatten().collect()
}

#[derive(Default)]
struct Bin {
    lumps: Vec<(usize, Vec<u8>)>,
}

impl Bin {
    fn lump(&mut self, index: usize, bytes: Vec<u8>) {
        self.lumps.push((index, bytes));
    }

    fn finish(self) -> Vec<u8> {
        let header = 8 + 64 * 16 + 4;
        let mut body = Vec::new();
        let mut entries = [(0i32, 0i32); 64];
        for (index, bytes) in self.lumps {
            entries[index] = ((header + body.len()) as i32, bytes.len() as i32);
            body.extend(bytes);
        }
        let mut out = Vec::new();
        out.extend(b"VBSP");
        out.extend(20i32.to_le_bytes());
        for (offset, length) in entries {
            out.extend(offset.to_le_bytes());
            out.extend(length.to_le_bytes());
            out.extend(0i32.to_le_bytes());
            out.extend([0, 0, 0, 0]);
        }
        out.extend(0i32.to_le_bytes());
        out.extend(body);
        out
    }
}

#[test]
fn prop_lightmaps_keep_lod0_in_file_order() {
    use crate::{prop_grids, PropBody, PropLight, PropVerts};
    let props = [
        PropLight {
            ldr: String::new(),
            hdr: String::new(),
            checksum: 0,
            first: 10,
            body: PropBody::Luxels {
                width: 4,
                height: 2,
                lods: vec![1, 0, 0],
                ldr_format: 2,
                hdr_format: 24,
            },
        },
        PropLight {
            ldr: String::new(),
            hdr: String::new(),
            checksum: 0,
            first: 100,
            body: PropBody::Vertices {
                meshes: vec![PropVerts { lod: 0, count: 3 }],
            },
        },
    ];
    let grids = prop_grids(&props);
    assert_eq!(grids.len(), 2);
    assert_eq!(grids[0].first_luxel, 18);
    assert_eq!(grids[0].luxel_count, 8);
    assert_eq!(grids[1].first_luxel, 26);
    assert_eq!((grids[1].width, grids[1].height), (4, 2));
}
