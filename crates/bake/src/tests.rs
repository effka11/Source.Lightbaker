use std::path::PathBuf;

use glam::Vec3;
use solve::{solve, Area, Rectangle, Triangle};

use super::{bake, FULL_RAYS};

#[test]
fn bake_writes_the_full_budget() {
    let dir = scratch();
    let source = dir.join("room.bsp");
    let dest = dir.join("room_light.bsp");
    let original = room_bsp();
    std::fs::write(&source, &original).unwrap();
    let map = map::open(&source).unwrap();

    let mut triangles = map.triangles.clone();
    triangles.push(Triangle {
        vertices: [
            Vec3::new(-8.0, -8.0, 24.0),
            Vec3::new(20.0, -8.0, 24.0),
            Vec3::new(20.0, 40.0, 24.0),
        ],
    });
    let receivers: Vec<_> = map.luxels.iter().map(|luxel| luxel.receiver).collect();
    let areas = [Area::Rectangle(Rectangle {
        center: Vec3::new(16.0, 16.0, 48.0),
        half_u: Vec3::new(24.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 24.0, 0.0),
        normal: -Vec3::Z,
        intensity: 18_000.0,
        color: Vec3::ONE,
    })];

    let full = solve(&triangles, &receivers, &areas, FULL_RAYS);
    let one = solve(&triangles, &receivers, &areas, 1);
    assert_ne!(full.light, one.light);

    bake(
        &triangles,
        &receivers,
        &areas,
        &map.snapshot,
        &source,
        &dest,
    )
    .unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), original);

    let written = map::open(&dest).unwrap();
    let ldr = span(&written, false);
    let hdr = span(&written, true);
    assert_eq!(ldr, hdr);
    let mut differed = false;
    for face in &written.snapshot.faces {
        assert_eq!(face.styles, [0, 255, 255, 255]);
        if face.light_offset < 0 {
            continue;
        }
        for index in 0..face.luxel_count {
            let at = face.light_offset as usize + index as usize * 4;
            let got = unpack(&ldr[at..at + 4]);
            let slot = face.first_luxel as usize + index as usize;
            near(got, full.light[slot]);
            let preview = one.light[slot];
            if (got[0] - preview[0]).abs() > 0.05
                || (got[1] - preview[1]).abs() > 0.05
                || (got[2] - preview[2]).abs() > 0.05
            {
                differed = true;
            }
        }
    }
    assert!(differed, "full budget matched the single-ray light");
}

fn span(map: &map::Map, hdr: bool) -> Vec<u8> {
    let span = if hdr {
        map.snapshot.lighting_hdr
    } else {
        map.snapshot.lighting
    };
    let start = span.offset as usize;
    map.snapshot.bytes[start..start + span.length as usize].to_vec()
}

fn unpack(sample: &[u8]) -> [f32; 3] {
    let exponent = sample[3] as i8 as i32;
    let scale = 2.0f32.powi(exponent) / 255.0;
    [
        sample[0] as f32 * scale,
        sample[1] as f32 * scale,
        sample[2] as f32 * scale,
    ]
}

fn near(got: [f32; 3], expected: [f32; 3]) {
    let scale = got
        .into_iter()
        .chain(expected)
        .map(f32::abs)
        .fold(0.0f32, f32::max);
    let tol = (scale / 64.0).max(1.0e-3);
    for (left, right) in got.into_iter().zip(expected) {
        assert!((left - right).abs() <= tol, "{left} vs {right}");
    }
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightbaker-bake-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn room_bsp() -> Vec<u8> {
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
    let mut edges = Vec::new();
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
        edges.extend(a.to_le_bytes());
        edges.extend(b.to_le_bytes());
    }
    bin.lump(12, edges);
    let mut surf = Vec::new();
    for index in 0..8i32 {
        surf.extend(index.to_le_bytes());
    }
    bin.lump(13, surf);
    bin.lump(7, pack([face(0, 0, 0, 0), face(1, 4, 1, 16)]));
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
    bin.lump(43, b"room/floor\0room/wall\0".to_vec());
    let floor_len = "room/floor\0".len() as i32;
    bin.lump(44, pack([0i32.to_le_bytes(), floor_len.to_le_bytes()]));
    bin.lump(8, vec![0u8; 32]);
    bin.lump(53, vec![0u8; 32]);
    bin.finish()
}

fn plane(normal: Vec3, dist: f32) -> [u8; 20] {
    let mut out = [0u8; 20];
    out[..12].copy_from_slice(&vec3(normal.x, normal.y, normal.z));
    out[12..16].copy_from_slice(&dist.to_le_bytes());
    out
}

fn face(plane: u16, first_edge: i32, texinfo: i16, light_offset: i32) -> [u8; 56] {
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
