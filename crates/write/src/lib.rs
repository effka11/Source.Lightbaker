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
const PAKFILE: usize = 40;
const LIGHTING_HDR: usize = 53;
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

/// `source` is the map that was opened. The same path as `destination` is refused.
pub fn write(
    snapshot: &Snapshot,
    light: &[[f32; 3]],
    source: &Path,
    destination: &Path,
) -> Result<(), Error> {
    if same_path(source, destination) {
        return Err(Error::SamePath);
    }
    let bytes = assemble(snapshot, light)?;
    commit(destination, &bytes)
}

fn assemble(snapshot: &Snapshot, light: &[[f32; 3]]) -> Result<Vec<u8>, Error> {
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
    let (lighting, patches) = paint(&snapshot.faces, light)?;
    apply_faces(&mut lumps[FACES].data, &patches);
    if !snapshot.props.is_empty() {
        lumps[PAKFILE].data = propfile::repack(&lumps[PAKFILE].data, &snapshot.props, light)?;
    }
    if lumps[LIGHTING_HDR].data.is_empty() {
        lumps[LIGHTING_HDR].version = lumps[LIGHTING].version;
        lumps[LIGHTING_HDR].fourcc = lumps[LIGHTING].fourcc;
    }
    lumps[LIGHTING].data = lighting.clone();
    lumps[LIGHTING_HDR].data = lighting;
    Ok(pack(data, &lumps))
}

fn paint(faces: &[FaceLight], light: &[[f32; 3]]) -> Result<(Vec<u8>, Vec<Patch>), Error> {
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
        let color_at = |index: usize| -> [f32; 3] {
            if count == 0 {
                [0.0, 0.0, 0.0]
            } else {
                light[start + index]
            }
        };
        // The average sits in front of lightofs. The engine reads face samples at lightofs.
        push_sample(&mut bytes, mean(samples, &color_at));
        let offset = i32::try_from(bytes.len()).map_err(|_| Error::NotAMap)?;
        let slots = if face.bumped { BUMP_SLOTS } else { 1 };
        for _slot in 0..slots {
            for index in 0..samples {
                push_sample(&mut bytes, color_at(index));
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
        entries[index].0 = (HEADER + body.len()) as i32;
        entries[index].1 = lumps[index].data.len() as i32;
        body.extend_from_slice(&lumps[index].data);
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

/// ColorRGBExp32. The engine's linear value is `byte / 255 * 2^exponent`,
/// so the stored vector is the linear color multiplied by 255.
fn pack_sample(linear: [f32; 3]) -> [u8; 4] {
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

fn push_sample(out: &mut Vec<u8>, linear: [f32; 3]) {
    out.extend_from_slice(&pack_sample(linear));
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
