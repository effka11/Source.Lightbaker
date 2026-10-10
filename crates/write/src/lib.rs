//! Puts linear light into a new BSP. The source path is refused.
//! LDR and HDR are two packings of that light, style 0 only.
//! Face light replaces the lighting lumps. Prop light replaces luxel and
//! vertex records inside the copied pak; models and materials stay byte for byte.

mod propfile;

#[cfg(test)]
mod tests;

use std::io;
use std::path::{Component, Path, PathBuf};

use map::{FaceLight, PropLight, Snapshot};

const LUMPS: usize = 64;
const HEADER: usize = 8 + LUMPS * 16 + 4;
const FACE: usize = 56;
const FACES: usize = 7;
const LIGHTING: usize = 8;
/// Compiled lights. An empty lump makes the engine set `mat_fullbright 1`.
const WORLDLIGHTS: usize = 15;
const PAKFILE: usize = 40;
const GAME: usize = 35;
const LIGHTING_HDR: usize = 53;
/// `dworldlight_t` as stored by version 0 of the lump.
const WORLDLIGHT_BYTES: usize = 88;
const EMIT_POINT: i32 = 1;
/// Garry's Mod reads these faces, not lump 7, whenever HDR is on.
const FACES_HDR: usize = 58;
/// Flat sample plus the three bump directions Source already stored.
const BUMP_SLOTS: usize = 4;

#[derive(Debug)]
pub enum Error {
    SamePath,
    Light,
    NotAMap,
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::SamePath => write!(f, "нельзя записать поверх исходной карты"),
            Error::Light => write!(f, "свет не сходится с приёмниками"),
            Error::NotAMap => write!(f, "это не карта Source 1"),
            Error::Io(err) => write!(f, "файл не пишется: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

struct Raw {
    offset: i32,
    version: i32,
    fourcc: [u8; 4],
    data: Vec<u8>,
    present: bool,
}

enum Patch {
    Keep,
    Clear,
    Lit(i32),
}

/// One placed lamp. The engine does not read the solve. A worldlight at this
/// point keeps the map from going fullbright and lights a character standing
/// under the lamp. Without that lump the lightmaps never show.
#[derive(Clone, Copy, Debug)]
pub struct PlacedLight {
    pub origin: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
}

/// `source` is the map that was opened. The same path as `destination` is refused.
pub fn write(
    snapshot: &Snapshot,
    light: &[[f32; 3]],
    albedo: &[[f32; 3]],
    lights: &[PlacedLight],
    source: &Path,
    destination: &Path,
) -> Result<(), Error> {
    if same_path(source, destination) {
        return Err(Error::SamePath);
    }
    let bytes = assemble(snapshot, light, albedo, lights)?;
    commit(destination, &bytes)
}

fn assemble(
    snapshot: &Snapshot,
    light: &[[f32; 3]],
    albedo: &[[f32; 3]],
    lights: &[PlacedLight],
) -> Result<Vec<u8>, Error> {
    let data = &snapshot.bytes;
    if data.len() < HEADER || &data[..4] != b"VBSP" {
        return Err(Error::NotAMap);
    }
    let version = i32::from_le_bytes(data[4..8].try_into().unwrap());
    if version != 19 && version != 20 {
        return Err(Error::NotAMap);
    }

    let mut lumps = read_lumps(data)?;
    let face_bytes = lumps[FACES].data.len();
    if face_bytes % FACE != 0 || face_bytes / FACE != snapshot.faces.len() {
        return Err(Error::NotAMap);
    }

    let face_end = snapshot
        .faces
        .iter()
        .map(|face| face.first_luxel as usize + face.luxel_count as usize)
        .max()
        .unwrap_or(0);
    if !light_spans(&snapshot.props, face_end, light.len()) {
        return Err(Error::Light);
    }
    let (lighting, patches) = paint(&snapshot.faces, light, albedo)?;
    apply_faces(&mut lumps[FACES].data, &patches);
    // HDR faces keep their own geometry, but their light offsets must land in
    // the lighting we just wrote. Leaving the old offsets reads off the end.
    if !lumps[FACES_HDR].data.is_empty() {
        if lumps[FACES_HDR].fourcc != [0; 4] || lumps[FACES_HDR].data.len() != face_bytes {
            return Err(Error::NotAMap);
        }
        apply_faces(&mut lumps[FACES_HDR].data, &patches);
    }
    if !snapshot.props.is_empty() {
        lumps[PAKFILE].data = propfile::repack(&lumps[PAKFILE].data, &snapshot.props, light)?;
    }
    // A map compiled without HDR has an empty HDR lump. Filling it makes
    // Garry's Mod read HDR light while the HDR face lump is still absent,
    // and the load hangs. HDR is written only when the map already had it.
    let had_hdr = !lumps[LIGHTING_HDR].data.is_empty();
    lumps[LIGHTING].data = lighting.clone();
    if had_hdr {
        lumps[LIGHTING_HDR].data = lighting;
    }
    // Relapse ships with an empty worldlight lump. The engine then sets
    // mat_fullbright, and neither the lightmaps nor the prop colors show.
    // One record per lamp, cut off at a room's width, is enough for characters
    // standing under it. A light entity is not written: that point would light
    // the brushes a second time and cast shadow blobs the bake does not have.
    if lumps[WORLDLIGHTS].data.is_empty() && !lights.is_empty() {
        lumps[WORLDLIGHTS].data = worldlight_records(lights);
        lumps[WORLDLIGHTS].version = 0;
        lumps[WORLDLIGHTS].present = true;
    }
    Ok(pack(data, &lumps))
}

/// A fluorescent (18000) lands near a normal indoor Source brightness.
/// The sun is far brighter in the solve; past 2000 the worldlight would cover the map.
fn source_brightness(intensity: f32) -> f32 {
    if !intensity.is_finite() || intensity <= 0.0 {
        return 1.0;
    }
    (intensity / 18_000.0 * 200.0).clamp(1.0, 2_000.0)
}

fn worldlight_records(lights: &[PlacedLight]) -> Vec<u8> {
    let mut out = Vec::with_capacity(lights.len() * WORLDLIGHT_BYTES);
    for light in lights {
        let brightness = source_brightness(light.intensity);
        let color = light
            .color
            .map(|channel| channel.clamp(0.0, 1.0) * brightness);
        push_f32s(&mut out, &light.origin);
        push_f32s(&mut out, &color);
        push_f32s(&mut out, &[0.0, 0.0, 1.0]);
        out.extend((-1i32).to_le_bytes());
        out.extend(EMIT_POINT.to_le_bytes());
        out.extend(0i32.to_le_bytes());
        // stopdot, stopdot2, exponent, radius, constant, linear, quadratic.
        // Radius 0 is an infinite light: its shadow reaches rooms this lamp
        // does not light. A room's width is as far as the character light goes.
        for value in [0.0f32, 0.0, 0.0, 384.0, 0.0, 0.0, 1.0] {
            out.extend(value.to_le_bytes());
        }
        out.extend(0i32.to_le_bytes());
        out.extend((-1i32).to_le_bytes());
        out.extend((-1i32).to_le_bytes());
    }
    out
}

fn push_f32s(out: &mut Vec<u8>, values: &[f32; 3]) {
    for value in values {
        out.extend(finite(*value).to_le_bytes());
    }
}

fn paint(
    faces: &[FaceLight],
    light: &[[f32; 3]],
    albedo: &[[f32; 3]],
) -> Result<(Vec<u8>, Vec<Patch>), Error> {
    for face in faces {
        let start = face.first_luxel as usize;
        let count = face.luxel_count as usize;
        if start
            .checked_add(count)
            .map(|end| end > light.len())
            .unwrap_or(true)
        {
            return Err(Error::Light);
        }
    }
    let covered = faces
        .iter()
        .map(|face| face.first_luxel + face.luxel_count)
        .max()
        .unwrap_or(0) as usize;
    if covered > light.len() {
        return Err(Error::Light);
    }

    let mut bytes = Vec::new();
    let mut patches = Vec::with_capacity(faces.len());
    for face in faces {
        let samples = grid_samples(face)?;
        if face.light_offset < 0 || samples == 0 {
            patches.push(if face.light_offset < 0 {
                Patch::Keep
            } else {
                Patch::Clear
            });
            continue;
        }
        let count = face.luxel_count as usize;
        if count != samples && count != 0 {
            return Err(Error::Light);
        }
        let start = face.first_luxel as usize;
        let raw: Vec<[f32; 3]> = (0..samples)
            .map(|index| {
                if count == 0 {
                    [0.0, 0.0, 0.0]
                } else {
                    light[start + index]
                }
            })
            .collect();
        // The window blurs each face by one luxel before the curve. The game
        // samples the stored texel on its own, so the same blur has to be in
        // the file or a dark wall shows the raw speckles.
        let blurred = soften(&raw, face.width as usize, face.height as usize);
        let stored: Vec<[f32; 3]> = blurred
            .iter()
            .enumerate()
            .map(|(index, color)| engine_linear(*color, albedo_of(albedo, start + index)))
            .collect();
        // The average sits in front of lightofs. The engine reads face samples at lightofs.
        push_linear(&mut bytes, mean(stored.len(), &|index| stored[index]));
        let offset = i32::try_from(bytes.len()).map_err(|_| Error::NotAMap)?;
        let slots = if face.bumped { BUMP_SLOTS } else { 1 };
        for _slot in 0..slots {
            for color in &stored {
                push_linear(&mut bytes, *color);
            }
        }
        patches.push(Patch::Lit(offset));
    }
    Ok((bytes, patches))
}

fn light_spans(props: &[PropLight], face_end: usize, len: usize) -> bool {
    let mut cursor = face_end;
    for prop in props {
        if prop.first as usize != cursor {
            return false;
        }
        cursor = cursor.saturating_add(prop.samples());
    }
    cursor == len
}

fn grid_samples(face: &FaceLight) -> Result<usize, Error> {
    if face.width > 4096 || face.height > 4096 {
        return Err(Error::NotAMap);
    }
    (face.width as usize)
        .checked_mul(face.height as usize)
        .ok_or(Error::NotAMap)
}

fn mean(samples: usize, color_at: &impl Fn(usize) -> [f32; 3]) -> [f32; 3] {
    if samples == 0 {
        return [0.0, 0.0, 0.0];
    }
    let mut sum = [0.0f32; 3];
    for index in 0..samples {
        let color = color_at(index);
        sum[0] += finite(color[0]);
        sum[1] += finite(color[1]);
        sum[2] += finite(color[2]);
    }
    let scale = samples as f32;
    [sum[0] / scale, sum[1] / scale, sum[2] / scale]
}

fn apply_faces(faces: &mut [u8], patches: &[Patch]) {
    for (index, patch) in patches.iter().enumerate() {
        let face = &mut faces[index * FACE..(index + 1) * FACE];
        match *patch {
            Patch::Keep => {}
            Patch::Clear => {
                face[16..20].fill(255);
                face[20..24].copy_from_slice(&(-1i32).to_le_bytes());
            }
            Patch::Lit(offset) => {
                face[16] = 0;
                face[17] = 255;
                face[18] = 255;
                face[19] = 255;
                face[20..24].copy_from_slice(&offset.to_le_bytes());
            }
        }
    }
}

fn read_lumps(data: &[u8]) -> Result<[Raw; LUMPS], Error> {
    let mut lumps = std::array::from_fn(|_| Raw {
        offset: 0,
        version: 0,
        fourcc: [0; 4],
        data: Vec::new(),
        present: false,
    });
    for index in 0..LUMPS {
        let at = 8 + index * 16;
        let offset = i32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let length = i32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap());
        let version = i32::from_le_bytes(data[at + 8..at + 12].try_into().unwrap());
        let mut fourcc = [0u8; 4];
        fourcc.copy_from_slice(&data[at + 12..at + 16]);
        if offset < 0 || length < 0 {
            return Err(Error::NotAMap);
        }
        let bytes = if length == 0 {
            Vec::new()
        } else {
            let start = offset as usize;
            let end = start.checked_add(length as usize).ok_or(Error::NotAMap)?;
            data.get(start..end).ok_or(Error::NotAMap)?.to_vec()
        };
        lumps[index] = Raw {
            offset,
            version,
            fourcc,
            present: length > 0,
            data: bytes,
        };
    }
    Ok(lumps)
}

fn pack(data: &[u8], lumps: &[Raw; LUMPS]) -> Vec<u8> {
    let mut order: Vec<usize> = (0..LUMPS)
        .filter(|&index| !lumps[index].data.is_empty())
        .collect();
    order.sort_by(
        |&left, &right| match (lumps[left].present, lumps[right].present) {
            (true, true) => lumps[left]
                .offset
                .cmp(&lumps[right].offset)
                .then(left.cmp(&right)),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => left.cmp(&right),
        },
    );

    let mut body = Vec::new();
    let mut entries = [(0i32, 0i32, 0i32, [0u8; 4]); LUMPS];
    for index in 0..LUMPS {
        entries[index].2 = lumps[index].version;
        entries[index].3 = lumps[index].fourcc;
    }
    for index in order {
        let pad = (4 - body.len() % 4) % 4;
        body.extend(std::iter::repeat(0).take(pad));
        let new_offset = (HEADER + body.len()) as i32;
        entries[index].0 = new_offset;
        entries[index].1 = lumps[index].data.len() as i32;
        if index == GAME && new_offset != lumps[index].offset {
            let mut bytes = lumps[index].data.clone();
            // The prop directory stores offsets into the file, not into the
            // lump. The bytes move with the lump; the old offsets land on
            // other data, and the engine builds no static props.
            relocate_game_lump(&mut bytes, lumps[index].offset, new_offset);
            body.extend_from_slice(&bytes);
        } else {
            body.extend_from_slice(&lumps[index].data);
        }
    }

    let mut out = Vec::with_capacity(HEADER + body.len());
    out.extend_from_slice(&data[..8]);
    for (offset, length, version, fourcc) in entries {
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&fourcc);
    }
    out.extend_from_slice(&data[HEADER - 4..HEADER]);
    out.extend_from_slice(&body);
    out
}

/// `dgamelump_t.fileofs` is an offset from the start of the BSP. An entry
/// that pointed into this lump still does, at the lump's new position.
/// An entry that pointed elsewhere is left alone.
fn relocate_game_lump(data: &mut [u8], old_offset: i32, new_offset: i32) {
    let delta = new_offset.wrapping_sub(old_offset);
    if delta == 0 || data.len() < 4 {
        return;
    }
    let count = i32::from_le_bytes(data[0..4].try_into().unwrap());
    if count <= 0 || count > 64 {
        return;
    }
    let end = old_offset as i64 + data.len() as i64;
    for index in 0..count as usize {
        let at = 4 + index * 16 + 8;
        if at + 4 > data.len() {
            return;
        }
        let offset = i32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let start = offset as i64;
        if start < old_offset as i64 || start >= end {
            continue;
        }
        let moved = offset.wrapping_add(delta);
        data[at..at + 4].copy_from_slice(&moved.to_le_bytes());
    }
}

/// The window squares `albedo * light` and then bends the shoulder.
pub fn screen(linear: [f32; 3]) -> [f32; 3] {
    linear.map(screen_channel)
}

fn screen_channel(value: f32) -> f32 {
    let value = finite(value).max(0.0);
    let curve = value * value;
    curve / (1.0 + (curve - 0.25).max(0.0))
}

/// ColorRGBExp32 whose `pow(x, 1/2.2)` is the window shade divided by the
/// texture byte. The engine multiplies the lightmap by that byte, which
/// puts the shade back. The LDR table stops at linear 4.
pub fn engine_linear(irradiance: [f32; 3], albedo: [f32; 3]) -> [f32; 3] {
    let cap = 4.0_f32.powf(1.0 / 2.2);
    [0, 1, 2].map(|channel| {
        let byte = finite(albedo[channel]).max(0.0);
        if byte <= 1.0e-3 {
            return 0.0;
        }
        let shaded = screen_channel(finite(irradiance[channel]).max(0.0) * byte);
        let multiplier = (srgb_encode(shaded) / byte).clamp(0.0, cap);
        if multiplier <= 0.0 {
            0.0
        } else {
            multiplier.powf(2.2).min(4.0)
        }
    })
}

pub fn srgb_encode(linear: f32) -> f32 {
    let value = finite(linear).clamp(0.0, 1.0);
    if value <= 0.0031308 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Byte a static-prop vertex stores. The engine multiplies it by two, which
/// restores the sRGB shade instead of the linear one.
pub fn shaded_byte(shaded: f32) -> u8 {
    let shaded = finite(shaded).max(0.0);
    if shaded <= 0.0 {
        return 0;
    }
    let shown = srgb_encode(shaded) * 0.5;
    if shown >= 1.0 {
        255
    } else {
        (shown * 255.0).round() as u8
    }
}

/// One-luxel blur, edges repeated. Matches the window's 3×3 on a padded chart.
pub fn soften(colors: &[[f32; 3]], width: usize, height: usize) -> Vec<[f32; 3]> {
    if width == 0 || height == 0 || colors.len() != width.saturating_mul(height) {
        return colors.to_vec();
    }
    let mut out = vec![[0.0; 3]; colors.len()];
    for t in 0..height {
        for s in 0..width {
            let mut sum = [0.0; 3];
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let x = (s as i32 + dx).clamp(0, width as i32 - 1) as usize;
                    let y = (t as i32 + dy).clamp(0, height as i32 - 1) as usize;
                    let color = colors[y * width + x];
                    sum[0] += finite(color[0]);
                    sum[1] += finite(color[1]);
                    sum[2] += finite(color[2]);
                }
            }
            out[t * width + s] = sum.map(|channel| channel / 9.0);
        }
    }
    out
}

fn albedo_of(albedo: &[[f32; 3]], index: usize) -> [f32; 3] {
    albedo.get(index).copied().unwrap_or([1.0, 1.0, 1.0])
}

/// ColorRGBExp32 of a white surface. Tests pack a known irradiance this way.
#[cfg(test)]
fn pack_sample(linear: [f32; 3]) -> [u8; 4] {
    pack_linear(engine_linear(linear, [1.0, 1.0, 1.0]))
}

/// ColorRGBExp32. The engine's linear value is `byte / 255 * 2^exponent`,
/// so the stored vector is the linear color multiplied by 255.
pub(crate) fn pack_linear(linear: [f32; 3]) -> [u8; 4] {
    let scaled = [
        finite(linear[0]).max(0.0) * 255.0,
        finite(linear[1]).max(0.0) * 255.0,
        finite(linear[2]).max(0.0) * 255.0,
    ];
    let max = scaled[0].max(scaled[1]).max(scaled[2]);
    if max <= 0.0 {
        return [0, 0, 0, 0];
    }
    let exponent = exponent_for(max).min(127);
    if exponent < -120 {
        return [0, 0, 0, 0];
    }
    let scalar = 2.0f32.powi(-exponent);
    let byte = |value: f32| -> u8 {
        let channel = value * scalar;
        if channel <= 0.0 {
            0
        } else if channel >= 255.0 {
            255
        } else {
            channel as u8
        }
    };
    [
        byte(scaled[0]),
        byte(scaled[1]),
        byte(scaled[2]),
        exponent as u8,
    ]
}

fn push_linear(out: &mut Vec<u8>, linear: [f32; 3]) {
    out.extend_from_slice(&pack_linear(linear));
}

fn exponent_for(max: f32) -> i32 {
    let field = (max.to_bits() >> 23) & 0xff;
    field as i32 - (7 + 127)
}

fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn commit(destination: &Path, bytes: &[u8]) -> Result<(), Error> {
    let partial = partial_path(destination);
    if let Err(err) = std::fs::write(&partial, bytes) {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Io(err));
    }
    if std::fs::rename(&partial, destination).is_ok() {
        return Ok(());
    }
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }
    if let Err(err) = std::fs::rename(&partial, destination) {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Io(err));
    }
    Ok(())
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("light.bsp"));
    name.push(".partial");
    destination.with_file_name(name)
}

fn same_path(source: &Path, destination: &Path) -> bool {
    if let (Ok(source), Ok(destination)) = (source.canonicalize(), destination.canonicalize()) {
        if eq_path(&source, &destination) {
            return true;
        }
    }
    eq_path(&normalize(source), &normalize(destination))
}

fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut out = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn eq_path(left: &Path, right: &Path) -> bool {
    let left: Vec<_> = left.components().collect();
    let right: Vec<_> = right.components().collect();
    left.len() == right.len()
        && left.iter().zip(&right).all(|(left, right)| {
            left.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
        })
}
