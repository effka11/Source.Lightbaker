use std::io::{Cursor, Read, Write};
use std::path::PathBuf;

use glam::Vec3;
use map::PropBody;
use solve::{solve, Area, Rectangle, Triangle};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

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

#[test]
fn grate_blocks_a_stripe_and_prop_light_replaces_only_the_pak() {
    let grate = studio_files(
        0x00C0_FFEE,
        [
            vert([10.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [0.0, 0.0]),
            vert([10.0, 0.0, 40.0], [-1.0, 0.0, 0.0], [0.0, 1.0]),
            vert([10.0, 16.0, 40.0], [-1.0, 0.0, 0.0], [1.0, 1.0]),
            vert([10.0, 16.0, 1.0], [-1.0, 0.0, 0.0], [1.0, 0.0]),
        ],
    );
    let card = studio_files(
        0x0000_BEEF,
        [
            vert([40.0, 0.0, 20.0], [0.0, 0.0, 1.0], [0.0, 0.0]),
            vert([56.0, 0.0, 20.0], [0.0, 0.0, 1.0], [1.0, 0.0]),
            vert([56.0, 16.0, 20.0], [0.0, 0.0, 1.0], [1.0, 1.0]),
            vert([40.0, 16.0, 20.0], [0.0, 0.0, 1.0], [0.0, 1.0]),
        ],
    );
    let material = b"MATERIAL-BYTES".to_vec();
    let kept_vhv = b"KEEP-CARD-VHV".to_vec();
    let files = [
        ("models/test/grate.mdl", grate.0.clone(), true),
        ("models/test/grate.vvd", grate.1.clone(), true),
        ("models/test/grate.dx90.vtx", grate.2.clone(), true),
        ("models/test/card.mdl", card.0.clone(), true),
        ("models/test/card.vvd", card.1.clone(), true),
        ("models/test/card.dx90.vtx", card.2.clone(), true),
        ("materials/test/card.vmt", material.clone(), false),
        ("sp_0.vhv", b"OLD-GRATE-LIGHT".to_vec(), false),
        ("sp_hdr_0.vhv", b"OLD-GRATE-LIGHT".to_vec(), false),
        ("texelslighting_1.ppl", dummy_ppl(2), false),
        ("texelslighting_1_hdr.ppl", dummy_ppl(24), false),
        ("sp_1.vhv", kept_vhv.clone(), false),
    ];
    let original = finish_with_props(room_bin(zip_of(&files)), prop_blob());
    let dir = scratch_named("props");
    let source = dir.join("props.bsp");
    let dest = dir.join("props_lit.bsp");
    std::fs::write(&source, &original).unwrap();

    let map = map::open(&source).unwrap();
    assert_eq!(map.triangles.len(), 8);
    assert_eq!(map.luxels.len(), 16);
    assert!((map.luxels[0].receiver.position - Vec3::new(8.0, 8.0, 0.0)).length() < 1.0e-3);
    assert!((map.luxels[1].receiver.position - Vec3::new(24.0, 8.0, 0.0)).length() < 1.0e-3);
    assert!((map.luxels[2].receiver.position - Vec3::new(8.0, 24.0, 0.0)).length() < 1.0e-3);
    assert!(map.luxels[8..12]
        .iter()
        .all(|luxel| (luxel.receiver.position.x - 10.0).abs() < 1.0e-3));
    assert!(map.luxels[12..16]
        .iter()
        .all(|luxel| (luxel.receiver.position.z - 20.0).abs() < 1.0e-2));
    assert!((map.luxels[12].receiver.albedo - Vec3::splat(0.5)).length() < 1.0e-6);

    assert_eq!(map.snapshot.props.len(), 2);
    let grate_prop = &map.snapshot.props[0];
    let card_prop = &map.snapshot.props[1];
    assert_eq!(grate_prop.checksum, 0x00C0_FFEE);
    assert_eq!(grate_prop.first, 8);
    assert_eq!(grate_prop.ldr, "sp_0.vhv");
    assert_eq!(grate_prop.hdr, "sp_hdr_0.vhv");
    assert!(matches!(grate_prop.body, PropBody::Vertices { .. }));
    assert_eq!(grate_prop.samples(), 4);
    assert_eq!(card_prop.checksum, 0x0000_BEEF);
    assert_eq!(card_prop.first, 12);
    assert_eq!(card_prop.ldr, "texelslighting_1.ppl");
    assert_eq!(card_prop.hdr, "texelslighting_1_hdr.ppl");
    match &card_prop.body {
        PropBody::Luxels {
            width,
            height,
            lods,
            ldr_format,
            hdr_format,
        } => {
            assert_eq!((*width, *height), (2, 2));
            assert_eq!(lods, &[0]);
            assert_eq!((*ldr_format, *hdr_format), (2, 24));
        }
        PropBody::Vertices { .. } => panic!("card should keep its lightmap"),
    }

    let floor: Vec<_> = map.luxels[..4].iter().map(|luxel| luxel.receiver).collect();
    let lamp = Area::Rectangle(Rectangle {
        center: Vec3::new(16.0, 16.0, 64.0),
        half_u: Vec3::new(2.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 2.0, 0.0),
        normal: -Vec3::Z,
        intensity: 10_000.0,
        color: Vec3::ONE,
    });
    let solved = solve(&map.triangles, &floor, &[lamp], 32);
    let luma = |color: [f32; 3]| color[0] + color[1] + color[2];
    let bar = luma(solved.light[0]);
    let front = luma(solved.light[1]);
    let slit = luma(solved.light[2]);
    assert!(
        slit > bar * 2.0 + 1.0e-3,
        "slit {slit} should pass the grate, bar {bar}"
    );
    assert!(
        front > bar * 2.0 + 1.0e-3,
        "open floor {front} should pass, bar {bar}"
    );

    let mut light = vec![[1.0, 0.0, 0.0]; map.luxels.len()];
    for sample in &mut light[8..12] {
        *sample = [0.0, 0.0, 1.0];
    }
    for sample in &mut light[12..] {
        *sample = [0.0, 1.0, 0.0];
    }
    write::write(&map.snapshot, &light, &source, &dest).unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), original);

    let written = std::fs::read(&dest).unwrap();
    for index in 0..64 {
        if index == 7 || index == 8 || index == 40 || index == 53 {
            continue;
        }
        assert_eq!(
            lump(&original, index),
            lump(&written, index),
            "lump {index}"
        );
    }
    let faces = lump(&original, 7).len() / 56;
    for index in 0..faces {
        let old = &lump(&original, 7)[index * 56..(index + 1) * 56];
        let new = &lump(&written, 7)[index * 56..(index + 1) * 56];
        assert_eq!(&old[..16], &new[..16], "face {index}");
        assert_eq!(&old[24..], &new[24..], "face {index}");
        assert_eq!(&new[16..20], &[0, 255, 255, 255]);
    }
    let ldr = lump(&written, 8);
    assert_eq!(ldr, lump(&written, 53));
    assert_eq!(unpack_rgbexp(ldr[..4].try_into().unwrap()), [1.0, 0.0, 0.0]);

    let old_pak = lump(&original, 40);
    let new_pak = lump(&written, 40);
    for (name, bytes, _) in &files {
        if name.ends_with(".ppl") || *name == "sp_0.vhv" || *name == "sp_hdr_0.vhv" {
            continue;
        }
        assert_eq!(zip_raw(old_pak, name), zip_raw(new_pak, name), "{name}");
        assert_eq!(
            zip_bytes(new_pak, name).as_slice(),
            bytes.as_slice(),
            "{name}"
        );
    }
    assert!(!new_pak
        .windows(15)
        .any(|window| window == b"OLD-GRATE-LIGHT"));
    assert!(!new_pak
        .windows(14)
        .any(|window| window == b"OLD-CARD-LIGHT"));

    let ldr_vhv = zip_bytes(new_pak, "sp_0.vhv");
    let hdr_vhv = zip_bytes(new_pak, "sp_hdr_0.vhv");
    assert_eq!(ldr_vhv, hdr_vhv);
    assert_eq!(i32_at(&ldr_vhv, 0), 2);
    assert_eq!(u32_at(&ldr_vhv, 4), 0x00C0_FFEE);
    assert_eq!(u32_at(&ldr_vhv, 16), 4);
    assert_eq!(u32_at(&ldr_vhv, 20), 1);
    let colors = u32_at(&ldr_vhv, 48) as usize;
    assert_eq!(&ldr_vhv[colors..colors + 4], &[255, 0, 0, 255]);
    assert_eq!(ldr_vhv.len() % 512, 0);

    let ldr_ppl = zip_bytes(new_pak, "texelslighting_1.ppl");
    let hdr_ppl = zip_bytes(new_pak, "texelslighting_1_hdr.ppl");
    assert_eq!(u32_at(&ldr_ppl, 4), 0x0000_BEEF);
    assert_eq!(u32_at(&ldr_ppl, 8), 2);
    assert_eq!(u32_at(&hdr_ppl, 8), 24);
    assert_eq!(u32_at(&ldr_ppl, 44), 2);
    assert_eq!(u32_at(&ldr_ppl, 48), 2);
    let pixels = u32_at(&ldr_ppl, 36) as usize;
    assert_eq!(&ldr_ppl[pixels..pixels + 3], &[0, 255, 0]);
    let hdr_pixels = u32_at(&hdr_ppl, 36) as usize;
    assert_eq!(
        &hdr_ppl[hdr_pixels..hdr_pixels + 8],
        &[0, 0, 0x00, 0x3C, 0, 0, 0x00, 0x3C]
    );
    assert_eq!(ldr_ppl.len() % 512, 0);
    assert_eq!(hdr_ppl.len() % 512, 0);
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
    scratch_named("room")
}

fn scratch_named(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightbaker-bake-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn room_bsp() -> Vec<u8> {
    room_bin(Vec::new()).finish()
}

fn room_bin(pak: Vec<u8>) -> Bin {
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
    bin.lump(40, pak);
    bin
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

fn finish_with_props(mut bin: Bin, blob: Vec<u8>) -> Vec<u8> {
    let header = 8 + 64 * 16 + 4;
    let prefix: usize = bin.lumps.iter().map(|(_, bytes)| bytes.len()).sum();
    let directory = 20;
    let blob_at = (header + prefix + directory) as i32;
    let mut game = Vec::new();
    game.extend(1i32.to_le_bytes());
    game.extend(b"prps");
    game.extend(0u16.to_le_bytes());
    game.extend(10u16.to_le_bytes());
    game.extend(blob_at.to_le_bytes());
    game.extend((blob.len() as i32).to_le_bytes());
    game.extend(blob);
    bin.lump(35, game);
    bin.finish()
}

fn prop_blob() -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend(2i32.to_le_bytes());
    blob.extend(fixed_name("models/test/grate.mdl"));
    blob.extend(fixed_name("models/test/card.mdl"));
    blob.extend(0i32.to_le_bytes());
    blob.extend(2i32.to_le_bytes());
    blob.extend(static_prop(0, 0x100, 0, 0));
    blob.extend(static_prop(1, 0, 4, 4));
    blob
}

fn fixed_name(name: &str) -> [u8; 128] {
    let mut out = [0u8; 128];
    out[..name.len()].copy_from_slice(name.as_bytes());
    out
}

fn static_prop(model: u16, flags: u32, width: u16, height: u16) -> [u8; 72] {
    let mut out = [0u8; 72];
    out[24..26].copy_from_slice(&model.to_le_bytes());
    out[64..68].copy_from_slice(&flags.to_le_bytes());
    out[68..70].copy_from_slice(&width.to_le_bytes());
    out[70..72].copy_from_slice(&height.to_le_bytes());
    out
}

fn dummy_ppl(format: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 64];
    put_u32(&mut bytes, 8, format);
    put_u32(&mut bytes, 12, 1);
    put_u32(&mut bytes, 44, 2);
    put_u32(&mut bytes, 48, 2);
    bytes.extend(b"OLD-CARD-LIGHT");
    bytes
}

struct Corner {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

fn vert(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Corner {
    Corner {
        position,
        normal,
        uv,
    }
}

fn studio_files(checksum: i32, corners: [Corner; 4]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut mdl = vec![0u8; 536];
    mdl[..4].copy_from_slice(b"IDST");
    put_i32(&mut mdl, 4, 48);
    put_i32(&mut mdl, 8, checksum);
    put_i32(&mut mdl, 232, 1);
    put_i32(&mut mdl, 236, 256);
    put_i32(&mut mdl, 260, 1);
    put_i32(&mut mdl, 268, 16);
    put_i32(&mut mdl, 344, 1);
    put_i32(&mut mdl, 348, 148);
    put_i32(&mut mdl, 356, 0);
    put_i32(&mut mdl, 428, 4);
    put_i32(&mut mdl, 432, 0);

    let mut vvd = vec![0u8; 64 + 4 * 48];
    vvd[..4].copy_from_slice(b"IDSV");
    put_i32(&mut vvd, 4, 4);
    put_i32(&mut vvd, 8, checksum);
    put_i32(&mut vvd, 16, 4);
    put_i32(&mut vvd, 56, 64);
    for (index, corner) in corners.iter().enumerate() {
        let at = 64 + index * 48;
        put_f32s(&mut vvd, at + 16, &corner.position);
        put_f32s(&mut vvd, at + 28, &corner.normal);
        put_f32s(&mut vvd, at + 40, &corner.uv);
    }

    let mut vtx = vec![0u8; 173];
    put_i32(&mut vtx, 0, 7);
    put_i32(&mut vtx, 16, checksum);
    put_i32(&mut vtx, 28, 1);
    put_i32(&mut vtx, 32, 36);
    put_i32(&mut vtx, 36, 1);
    put_i32(&mut vtx, 40, 8);
    put_i32(&mut vtx, 44, 1);
    put_i32(&mut vtx, 48, 8);
    put_i32(&mut vtx, 52, 1);
    put_i32(&mut vtx, 56, 12);
    put_i32(&mut vtx, 64, 1);
    put_i32(&mut vtx, 68, 9);
    put_i32(&mut vtx, 73, 4);
    put_i32(&mut vtx, 77, 25);
    put_i32(&mut vtx, 81, 6);
    put_i32(&mut vtx, 85, 61);
    put_i32(&mut vtx, 89, 1);
    put_i32(&mut vtx, 93, 73);
    for index in 0..4u16 {
        let at = 98 + index as usize * 9;
        vtx[at + 4..at + 6].copy_from_slice(&index.to_le_bytes());
    }
    for (index, value) in [0u16, 1, 2, 0, 2, 3].into_iter().enumerate() {
        let at = 134 + index * 2;
        vtx[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    put_i32(&mut vtx, 146, 6);
    put_i32(&mut vtx, 150, 0);
    vtx[164] = 1;
    (mdl, vvd, vtx)
}

fn zip_of(files: &[(&str, Vec<u8>, bool)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes, deflate) in files {
        let method = if *deflate {
            CompressionMethod::Deflated
        } else {
            CompressionMethod::Stored
        };
        let opts = SimpleFileOptions::default().compression_method(method);
        zip.start_file(*name, opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn zip_bytes(pak: &[u8], name: &str) -> Vec<u8> {
    let mut archive = ZipArchive::new(Cursor::new(pak)).unwrap();
    let mut file = archive.by_name(name).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes
}

fn zip_raw(pak: &[u8], name: &str) -> (CompressionMethod, u32, Vec<u8>) {
    let mut archive = ZipArchive::new(Cursor::new(pak)).unwrap();
    let index = archive.index_for_name(name).unwrap();
    let mut file = archive.by_index_raw(index).unwrap();
    let method = file.compression();
    let crc = file.crc32();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    (method, crc, bytes)
}

fn lump<'a>(data: &'a [u8], index: usize) -> &'a [u8] {
    let at = 8 + index * 16;
    let offset = i32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
    let length = i32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
    &data[offset..offset + length]
}

fn unpack_rgbexp(sample: [u8; 4]) -> [f32; 3] {
    let exponent = sample[3] as i8 as i32;
    let scale = 2.0f32.powi(exponent) / 255.0;
    [
        sample[0] as f32 * scale,
        sample[1] as f32 * scale,
        sample[2] as f32 * scale,
    ]
}

fn put_i32(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    put_i32(bytes, offset, value as i32);
}

fn put_f32s(bytes: &mut [u8], offset: usize, values: &[f32]) {
    for (index, value) in values.iter().enumerate() {
        bytes[offset + index * 4..offset + index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
}

fn i32_at(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
