use std::io::{Cursor, Write};
use std::path::PathBuf;

use glam::Vec3;
use solve::{solve, Area, Rectangle, Role};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::{open, Error};

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
fn luxels_sit_on_the_face_grid_and_pak_color_wins() {
    let path = temp("lightbaker-room.bsp");
    std::fs::write(&path, room_bsp(0, wall_pak())).unwrap();
    let map = open(&path).unwrap();

    assert_eq!(map.luxels.len(), 8);
    assert_eq!(map.triangles.len(), 4);

    let floor = &map.luxels[0];
    assert_eq!(floor.receiver.role, Role::Floor);
    assert!((floor.receiver.position - Vec3::new(8.0, 8.0, 0.0)).length() < 1.0e-3);
    assert!((floor.receiver.albedo - Vec3::Y).length() < 1.0e-3);

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
    assert_eq!(map.snapshot.bytes, std::fs::read(&path).unwrap());
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
        map.triangles.len() > 80_000 && map.triangles.len() < 200_000,
        "triangles {}",
        map.triangles.len()
    );
    assert!(map.snapshot.lighting.length > 0);
    assert_eq!(
        map.snapshot.bytes.len(),
        std::fs::metadata(&path).unwrap().len() as usize
    );

    let faces = &map.luxels[..face_end];
    let floor = nearest(faces, Vec3::new(823.0, -32.0, -144.0));
    assert_eq!(
        floor.receiver.role,
        Role::Floor,
        "{}",
        floor.receiver.position
    );
    assert!(
        (floor.receiver.position.z + 144.0).abs() < 2.0,
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

fn face(plane: u16, first_edge: i32, texinfo: i16, tex_unused: i16, light_offset: i32) -> [u8; 56] {
    let _ = tex_unused;
    let mut out = [0u8; 56];
    out[..2].copy_from_slice(&plane.to_le_bytes());
    out[4..8].copy_from_slice(&first_edge.to_le_bytes());
    out[8..10].copy_from_slice(&4i16.to_le_bytes());
    out[10..12].copy_from_slice(&texinfo.to_le_bytes());
    out[12..14].copy_from_slice(&(-1i16).to_le_bytes());
    out[14..16].copy_from_slice(&(-1i16).to_le_bytes());
    out[16] = 0;
    out[17] = 255;
    out[18] = 255;
    out[19] = 255;
    out[20..24].copy_from_slice(&light_offset.to_le_bytes());
    out[24..28].copy_from_slice(&1024f32.to_le_bytes());
    out[36..40].copy_from_slice(&1i32.to_le_bytes());
    out[40..44].copy_from_slice(&1i32.to_le_bytes());
    out
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

