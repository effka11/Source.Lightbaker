use std::io::{Cursor, Write};
use std::path::PathBuf;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::{pack_sample, write, Error, PlacedLight};

const HEADER: usize = 8 + 64 * 16 + 4;

#[test]
fn color_packs_into_the_engine_sample() {
    assert_eq!(unpack(pack_sample([0.0, 0.0, 0.0])), [0.0, 0.0, 0.0]);
    let folded = unpack(pack_sample([0.2, 0.1, 0.0]));
    assert!(folded[0] < 0.08 && folded[0] > 0.02, "{folded:?}");
    assert!(folded[1] < folded[0] && folded[1] > 0.0, "{folded:?}");
    assert_eq!(folded[2], 0.0);
    let byte = [0.45, 0.32, 0.20];
    let light = [0.8, 0.3, 0.1];
    let stored = super::engine_linear(light, byte);
    assert!(stored.iter().all(|channel| *channel <= 4.0), "{stored:?}");
    let display = super::screen([byte[0] * light[0], byte[1] * light[1], byte[2] * light[2]])
        .map(super::srgb_encode);
    for index in 0..3 {
        let game = byte[index] * stored[index].powf(1.0 / 2.2);
        assert!(
            (game - display[index]).abs() < 1.0e-4,
            "{game} vs {}",
            display[index]
        );
    }
    let dark = [0.15, 0.12, 0.08];
    let lamp = super::engine_linear([100.0, 80.0, 40.0], dark);
    assert!(lamp[0] <= 4.0, "{lamp:?}");
    let shown = dark[0] * lamp[0].powf(1.0 / 2.2);
    let buried = dark[0] * super::srgb_encode(super::screen([dark[0] * 100.0, 0.0, 0.0])[0]);
    assert!(shown > buried + 0.05, "{shown} vs {buried}");
    assert_eq!(unpack(pack_sample([1.0, -4.0, 0.0]))[1], 0.0);
    let modest = unpack(pack_sample([1.0, 0.0, 0.0]));
    let hot = unpack(pack_sample([4.0, 4.0, 4.0]));
    assert!(modest[0] < 1.0 && modest[0] > 0.4, "{modest:?}");
    assert!(hot[0] < 1.0 && hot[0] > modest[0], "{hot:?} vs {modest:?}");
    assert_eq!(
        unpack(pack_sample([f32::NAN, f32::INFINITY, 0.0])),
        [0.0, 0.0, 0.0]
    );
}

#[test]
fn the_source_path_is_refused_and_the_file_stays() {
    let dir = scratch("same");
    let source = dir.join("room.bsp");
    let bytes = fixture();
    std::fs::write(&source, &bytes).unwrap();
    let map = map::open(&source).unwrap();
    let light = vec![[1.0, 0.0, 0.0]; map.luxels.len()];
    let error = write(&map.snapshot, &light, &[], &[], &source, &source).unwrap_err();
    assert!(matches!(error, Error::SamePath));
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
}

#[test]
fn light_that_does_not_match_the_receivers_is_refused() {
    let dir = scratch("mismatch");
    let source = dir.join("room.bsp");
    let dest = dir.join("room_light.bsp");
    std::fs::write(&source, fixture()).unwrap();
    let map = map::open(&source).unwrap();
    let error = write(&map.snapshot, &[[1.0, 0.0, 0.0]], &[], &[], &source, &dest).unwrap_err();
    assert!(matches!(error, Error::Light));
    assert!(!dest.exists());
    assert_eq!(std::fs::read(&source).unwrap(), fixture());
}

#[test]
fn the_neighbor_replaces_face_light_and_leaves_the_source() {
    let dir = scratch("neighbor");
    let source = dir.join("room.bsp");
    let dest = dir.join("room_light.bsp");
    let original = fixture();
    std::fs::write(&source, &original).unwrap();
    let map = map::open(&source).unwrap();

    assert_eq!(map.luxels.len(), 8);
    assert_eq!(map.snapshot.faces[0].first_luxel, 0);
    assert_eq!(map.snapshot.faces[0].luxel_count, 4);
    assert!(!map.snapshot.faces[0].bumped);
    assert_eq!(map.snapshot.faces[1].styles, [0, 4, 255, 255]);
    assert!(map.snapshot.faces[1].bumped);
    assert_eq!(map.snapshot.faces[1].luxel_count, 4);
    assert_eq!(map.snapshot.faces[2].light_offset, -1);
    assert_eq!(map.snapshot.faces[2].luxel_count, 0);

    let floor = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 1.0, 0.0],
    ];
    let wall = [
        [0.5, 0.25, 0.125],
        [0.25, 0.5, 0.125],
        [0.125, 0.25, 0.5],
        [0.5, 0.5, 0.25],
    ];
    let mut light = Vec::new();
    light.extend_from_slice(&floor);
    light.extend_from_slice(&wall);

    write(&map.snapshot, &light, &[], &[], &source, &dest).unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), original);

    let written = std::fs::read(&dest).unwrap();
    assert_eq!(&written[..4], b"VBSP");
    assert_eq!(&written[4..8], &original[4..8]);
    assert_eq!(&written[HEADER - 4..HEADER], &original[HEADER - 4..HEADER]);
    for index in 0..64 {
        assert_eq!(
            lump_version(&original, index),
            lump_version(&written, index),
            "version {index}"
        );
        assert_eq!(
            lump_fourcc(&original, index),
            lump_fourcc(&written, index),
            "fourcc {index}"
        );
        if index == 7 || index == 8 || index == 53 {
            continue;
        }
        assert_eq!(
            lump(&original, index),
            lump(&written, index),
            "lump {index}"
        );
    }
    assert!(lump(&original, 0).windows(5).any(|bytes| bytes == b"light"));
    assert!(lump(&original, 40)
        .windows(14)
        .any(|bytes| bytes == b"OLD-PROP-LIGHT"));
    assert_eq!(lump(&original, 8), &[0xCD; 8]);

    for index in 0..2 {
        let old = face_bytes(&original, index);
        let new = face_bytes(&written, index);
        assert_eq!(&old[..16], &new[..16], "face {index} head");
        assert_eq!(&old[24..], &new[24..], "face {index} tail");
        assert_eq!(&new[16..20], &[0, 255, 255, 255]);
    }
    assert_eq!(face_bytes(&original, 2), face_bytes(&written, 2));

    let opened = map::open(&dest).unwrap();
    assert_eq!(opened.luxels.len(), 8);
    let ldr = lighting(&opened, false);
    let hdr = lighting(&opened, true);
    assert_eq!(ldr, hdr);
    // One average sample, then one style. The bumped wall repeats that color across four slots.
    // 4 + 4*4 + 4 + 4*4*4 = 88.
    assert_eq!(ldr.len(), 88);
    assert_eq!(opened.snapshot.faces[0].light_offset, 4);
    assert_eq!(opened.snapshot.faces[0].styles, [0, 255, 255, 255]);
    assert_eq!(opened.snapshot.faces[1].light_offset, 24);
    assert_eq!(opened.snapshot.faces[1].styles, [0, 255, 255, 255]);
    assert!(opened.snapshot.faces[1].bumped);
    assert_eq!(opened.snapshot.faces[2].light_offset, -1);
    assert_eq!(opened.snapshot.faces[2].styles, [255, 255, 255, 255]);

    let floor_soft = super::soften(&floor, 2, 2);
    let floor_stored: Vec<[f32; 3]> = floor_soft
        .iter()
        .copied()
        .map(|color| super::engine_linear(color, [1.0, 1.0, 1.0]))
        .collect();
    near(
        sample(&ldr, 0),
        unpack(super::pack_linear(mean(&floor_stored))),
    );
    for (index, color) in floor_stored.iter().enumerate() {
        near(
            sample(&ldr, 4 + index * 4),
            unpack(super::pack_linear(*color)),
        );
    }
    let wall_soft = super::soften(&wall, 2, 2);
    let wall_stored: Vec<[f32; 3]> = wall_soft
        .iter()
        .copied()
        .map(|color| super::engine_linear(color, [1.0, 1.0, 1.0]))
        .collect();
    let wall_at = 24;
    near(
        sample(&ldr, wall_at - 4),
        unpack(super::pack_linear(mean(&wall_stored))),
    );
    for slot in 0..4 {
        for (index, color) in wall_stored.iter().enumerate() {
            near(
                sample(&ldr, wall_at + (slot * 4 + index) * 4),
                unpack(super::pack_linear(*color)),
            );
        }
    }
}

#[test]
fn hdr_faces_follow_the_new_lighting() {
    let dir = scratch("hdr-faces");
    let source = dir.join("room.bsp");
    let dest = dir.join("room_light.bsp");
    std::fs::write(&source, fixture_with_stale_hdr_faces()).unwrap();
    let map = map::open(&source).unwrap();
    let light = vec![[0.2, 0.3, 0.4]; map.luxels.len()];
    write(&map.snapshot, &light, &[], &[], &source, &dest).unwrap();
    let written = std::fs::read(&dest).unwrap();
    let ldr = lump(&written, 7);
    let hdr = lump(&written, 58);
    let lighting = lump(&written, 53);
    assert_eq!(ldr.len(), hdr.len());
    assert_eq!(f32::from_le_bytes(hdr[24..28].try_into().unwrap()), 7.0);
    for index in 0..ldr.len() / 56 {
        let start = index * 56;
        let ldr_face = &ldr[start..start + 56];
        let hdr_face = &hdr[start..start + 56];
        assert_eq!(&ldr_face[16..24], &hdr_face[16..24]);
        let lightofs = i32::from_le_bytes(hdr_face[20..24].try_into().unwrap());
        if lightofs >= 0 {
            assert!((lightofs as usize) < lighting.len());
        }
    }
}

#[test]
fn an_empty_hdr_lump_stays_empty() {
    let dir = scratch("no-hdr");
    let source = dir.join("room.bsp");
    let dest = dir.join("room_light.bsp");
    let mut original = fixture();
    let at = 8 + 53 * 16;
    original[at + 4..at + 8].copy_from_slice(&0i32.to_le_bytes());
    std::fs::write(&source, &original).unwrap();
    let map = map::open(&source).unwrap();
    let light = vec![[0.2, 0.2, 0.2]; map.luxels.len()];
    write(&map.snapshot, &light, &[], &[], &source, &dest).unwrap();
    let written = std::fs::read(&dest).unwrap();
    assert_eq!(
        i32::from_le_bytes(lump_head(&written, 53)[4..8].try_into().unwrap()),
        0,
        "an LDR map must not grow an HDR lighting lump"
    );
}

fn lump_head(data: &[u8], index: usize) -> &[u8] {
    let at = 8 + index * 16;
    &data[at..at + 16]
}

#[test]
fn an_empty_worldlight_lump_receives_the_placed_lamps() {
    let dir = scratch("worldlights");
    let source = dir.join("room.bsp");
    let dest = dir.join("room_light.bsp");
    std::fs::write(&source, fixture()).unwrap();
    let map = map::open(&source).unwrap();
    let light = vec![[0.2, 0.2, 0.2]; map.luxels.len()];
    let lamps = [PlacedLight {
        origin: [16.0, 16.0, 64.0],
        color: [1.0, 0.5, 0.25],
        intensity: 18_000.0,
    }];
    write(&map.snapshot, &light, &[], &lamps, &source, &dest).unwrap();
    let written = std::fs::read(&dest).unwrap();
    let world = lump(&written, 15);
    assert_eq!(world.len(), 88);
    assert_eq!(lump_version(&written, 15), 0);
    let origin = [
        f32::from_le_bytes(world[0..4].try_into().unwrap()),
        f32::from_le_bytes(world[4..8].try_into().unwrap()),
        f32::from_le_bytes(world[8..12].try_into().unwrap()),
    ];
    assert_eq!(origin, [16.0, 16.0, 64.0]);
    assert_eq!(i32::from_le_bytes(world[40..44].try_into().unwrap()), 1);
    assert_eq!(f32::from_le_bytes(world[60..64].try_into().unwrap()), 384.0);
    assert_eq!(f32::from_le_bytes(world[72..76].try_into().unwrap()), 1.0);
    let strength = f32::from_le_bytes(world[12..16].try_into().unwrap());
    assert!((strength - 200.0).abs() < 1.0e-3, "{strength}");
    let text = String::from_utf8(lump(&written, 0).to_vec()).unwrap();
    let original = String::from_utf8(lump(&fixture(), 0).to_vec()).unwrap();
    assert_eq!(text, original);
}

#[test]
fn static_prop_offsets_move_with_the_game_lump() {
    let dir = scratch("props-move");
    let source = dir.join("room.bsp");
    let dest = dir.join("room_light.bsp");
    let mut original = fixture();
    let marker = b"PROPS";
    let mut game = vec![0u8; 4 + 32 + marker.len()];
    game[0..4].copy_from_slice(&2i32.to_le_bytes());
    game[4..8].copy_from_slice(b"prps");
    game[10..12].copy_from_slice(&10u16.to_le_bytes());
    let lump_at = original.len() as i32;
    let payload = lump_at + (4 + 32);
    game[12..16].copy_from_slice(&payload.to_le_bytes());
    game[16..20].copy_from_slice(&(marker.len() as i32).to_le_bytes());
    game[20..24].copy_from_slice(b"prpd");
    game[36..].copy_from_slice(marker);
    let at = 8 + 35 * 16;
    original[at..at + 4].copy_from_slice(&lump_at.to_le_bytes());
    original[at + 4..at + 8].copy_from_slice(&(game.len() as i32).to_le_bytes());
    original.extend_from_slice(&game);
    std::fs::write(&source, &original).unwrap();
    let map = map::open(&source).unwrap();
    let light = vec![[0.2, 0.2, 0.2]; map.luxels.len()];
    let lamps = [PlacedLight {
        origin: [16.0, 16.0, 64.0],
        color: [1.0, 1.0, 1.0],
        intensity: 18_000.0,
    }];
    write(&map.snapshot, &light, &[], &lamps, &source, &dest).unwrap();
    let written = std::fs::read(&dest).unwrap();
    let new_at = i32::from_le_bytes(lump_head(&written, 35)[0..4].try_into().unwrap());
    assert_ne!(
        new_at, lump_at,
        "the game lump has to move for the test to mean anything"
    );
    let game = lump(&written, 35);
    let moved = i32::from_le_bytes(game[12..16].try_into().unwrap());
    assert_eq!(moved, new_at + (payload - lump_at));
    assert_eq!(
        &written[moved as usize..moved as usize + marker.len()],
        marker
    );
    let untouched = i32::from_le_bytes(game[28..32].try_into().unwrap());
    assert_eq!(untouched, 0);
}

fn sample(bytes: &[u8], at: usize) -> [f32; 3] {
    unpack(bytes[at..at + 4].try_into().unwrap())
}

fn unpack(sample: [u8; 4]) -> [f32; 3] {
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
    let tol = (scale / 64.0).max(1.0e-4);
    for (index, (left, right)) in got.into_iter().zip(expected).enumerate() {
        assert!(
            (left - right).abs() <= tol,
            "channel {index}: {left} vs {right} (tol {tol})"
        );
    }
}

fn mean(colors: &[[f32; 3]]) -> [f32; 3] {
    let scale = colors.len() as f32;
    let mut sum = [0.0f32; 3];
    for color in colors {
        sum[0] += color[0];
        sum[1] += color[1];
        sum[2] += color[2];
    }
    [sum[0] / scale, sum[1] / scale, sum[2] / scale]
}

fn lighting(map: &map::Map, hdr: bool) -> Vec<u8> {
    let span = if hdr {
        map.snapshot.lighting_hdr
    } else {
        map.snapshot.lighting
    };
    let start = span.offset as usize;
    map.snapshot.bytes[start..start + span.length as usize].to_vec()
}

fn lump(data: &[u8], index: usize) -> &[u8] {
    let at = 8 + index * 16;
    let offset = i32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    let length = i32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap());
    if length == 0 {
        return &[];
    }
    &data[offset as usize..offset as usize + length as usize]
}

fn lump_version(data: &[u8], index: usize) -> i32 {
    let at = 8 + index * 16 + 8;
    i32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

fn lump_fourcc(data: &[u8], index: usize) -> [u8; 4] {
    let at = 8 + index * 16 + 12;
    data[at..at + 4].try_into().unwrap()
}

fn face_bytes(data: &[u8], index: usize) -> &[u8] {
    let faces = lump(data, 7);
    &faces[index * 56..(index + 1) * 56]
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightbaker-write-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture() -> Vec<u8> {
    room(None)
}

fn fixture_with_stale_hdr_faces() -> Vec<u8> {
    room(Some(100_000))
}

fn room(stale_hdr_light: Option<i32>) -> Vec<u8> {
    let mut bin = Bin::default();
    bin.lump(
        0,
        b"{\n\"classname\" \"light\"\n\"origin\" \"0 0 64\"\n}\n\0".to_vec(),
    );
    bin.lump(
        1,
        pack([plane([0.0, 0.0, 1.0], 0.0), plane([0.0, 1.0, 0.0], 0.0)]),
    );
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
    bin.lump_meta(4, vec![9, 8, 7, 6], 2, *b"VIS!");
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
    let mut wall = face(1, 4, 1, 16);
    wall[17] = 4;
    let faces = pack([face(0, 0, 0, 0), wall, unlit_face()]);
    bin.lump_meta(7, faces.clone(), 1, [0; 4]);
    if let Some(stale) = stale_hdr_light {
        let mut hdr = faces;
        for index in 0..2 {
            let at = index * 56;
            hdr[at + 20..at + 24].copy_from_slice(&stale.to_le_bytes());
        }
        hdr[24..28].copy_from_slice(&7.0f32.to_le_bytes());
        bin.lump_meta(58, hdr, 1, [0; 4]);
    }
    let floor_tex = texinfo([1.0 / 16.0, 0.0, 0.0, 0.0], [0.0, 1.0 / 16.0, 0.0, 0.0], 0);
    let mut wall_tex = texinfo([1.0 / 16.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0 / 16.0, 0.0], 1);
    wall_tex[64..68].copy_from_slice(&0x800i32.to_le_bytes());
    bin.lump(6, pack([floor_tex, wall_tex]));
    bin.lump(
        2,
        pack([texdata([0.0, 1.0, 0.0], 0), texdata([0.0, 0.0, 1.0], 1)]),
    );
    bin.lump(43, b"room/floor\0room/wall\0".to_vec());
    let floor_len = "room/floor\0".len() as i32;
    bin.lump(44, pack([0i32.to_le_bytes(), floor_len.to_le_bytes()]));
    bin.lump_meta(8, vec![0xCD; 8], 1, *b"LDR!");
    bin.lump_meta(53, vec![0xCD; 8], 1, *b"HDR!");
    bin.lump(40, pak());
    bin.revision = 42;
    bin.finish()
}

fn pak() -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("maps/room/prop.ppl", opts).unwrap();
    zip.write_all(b"OLD-PROP-LIGHT").unwrap();
    zip.start_file("materials/room/wall.vmt", opts).unwrap();
    zip.write_all(br#""LightmappedGeneric" { "$color" "[1 0 0]" }"#)
        .unwrap();
    zip.finish().unwrap().into_inner()
}

fn unlit_face() -> [u8; 56] {
    let mut out = [0u8; 56];
    out[3] = 7;
    out[8..10].copy_from_slice(&0i16.to_le_bytes());
    out[12..14].copy_from_slice(&(-1i16).to_le_bytes());
    out[14..16].copy_from_slice(&(-1i16).to_le_bytes());
    out[16..20].fill(255);
    out[20..24].copy_from_slice(&(-1i32).to_le_bytes());
    out[24..28].copy_from_slice(&4242.0f32.to_le_bytes());
    out[44..48].copy_from_slice(&77i32.to_le_bytes());
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

fn texdata(reflectivity: [f32; 3], name: i32) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..12].copy_from_slice(&vec3(reflectivity[0], reflectivity[1], reflectivity[2]));
    out[12..16].copy_from_slice(&name.to_le_bytes());
    out
}

fn plane(normal: [f32; 3], dist: f32) -> [u8; 20] {
    let mut out = [0u8; 20];
    out[..12].copy_from_slice(&vec3(normal[0], normal[1], normal[2]));
    out[12..16].copy_from_slice(&dist.to_le_bytes());
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
    lumps: Vec<Stored>,
    revision: i32,
}

struct Stored {
    index: usize,
    version: i32,
    fourcc: [u8; 4],
    bytes: Vec<u8>,
}

impl Bin {
    fn lump(&mut self, index: usize, bytes: Vec<u8>) {
        self.lump_meta(index, bytes, 0, [0; 4]);
    }

    fn lump_meta(&mut self, index: usize, bytes: Vec<u8>, version: i32, fourcc: [u8; 4]) {
        self.lumps.push(Stored {
            index,
            version,
            fourcc,
            bytes,
        });
    }

    fn finish(self) -> Vec<u8> {
        let mut body = Vec::new();
        let mut entries = [(0i32, 0i32, 0i32, [0u8; 4]); 64];
        for stored in self.lumps {
            let offset = if stored.bytes.is_empty() {
                0
            } else {
                (HEADER + body.len()) as i32
            };
            entries[stored.index] = (
                offset,
                stored.bytes.len() as i32,
                stored.version,
                stored.fourcc,
            );
            body.extend(stored.bytes);
        }
        let mut out = Vec::new();
        out.extend(b"VBSP");
        out.extend(20i32.to_le_bytes());
        for (offset, length, version, fourcc) in entries {
            out.extend(offset.to_le_bytes());
            out.extend(length.to_le_bytes());
            out.extend(version.to_le_bytes());
            out.extend(fourcc);
        }
        out.extend(self.revision.to_le_bytes());
        out.extend(body);
        out
    }
}
