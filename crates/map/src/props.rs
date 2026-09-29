//! Static props from the game lump. Model triangles come from the pak, then
//! from a VPK beside the map. A lightmap already on the prop becomes luxels;
//! everything else becomes vertices.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

use glam::Vec3;
use solve::{Receiver, Role, Triangle};
use zip::ZipArchive;

use crate::studio::{self, Group, Vert};
use crate::vpk;
use crate::{Luxel, PropBody, PropLight, PropVerts};

const GAME_LUMP: usize = 35;
const PAK_LUMP: usize = 40;
const NO_VERTEX: u32 = 0x40;
const NO_TEXEL: u32 = 0x100;
const RGB888: u32 = 2;
const RGBA16F: u32 = 24;

struct Static {
    origin: Vec3,
    angles: Vec3,
    model: String,
    flags: u32,
    lightmap: Option<(u32, u32)>,
}

struct Store {
    names: Vec<String>,
    files: HashMap<String, Vec<u8>>,
}

pub struct Placed {
    pub triangles: Vec<Triangle>,
    pub luxels: Vec<Luxel>,
    pub props: Vec<PropLight>,
}

pub fn place(bsp: &[u8], path: &Path, first: u32) -> Placed {
    let mut placed = Placed {
        triangles: Vec::new(),
        luxels: Vec::new(),
        props: Vec::new(),
    };
    let props = static_props(bsp);
    if props.is_empty() {
        return placed;
    }
    let store = pak_store(lump(bsp, PAK_LUMP).unwrap_or(&[]));
    let mut packs = None;
    let mut cursor = first;
    for (index, prop) in props.iter().enumerate() {
        let Some(model) = model_of(&store, &mut packs, path, &prop.model) else {
            continue;
        };
        let matrix = angle_matrix(prop.angles, prop.origin);
        let groups: Vec<Group> = model
            .groups
            .iter()
            .map(|group| transform_group(group, matrix))
            .collect();
        for group in groups.iter().filter(|group| group.lod == 0) {
            for tri in &group.triangles {
                push_world(&mut placed.triangles, group, *tri);
            }
        }
        let resolution = ppl_size(&store, index).or(prop.lightmap);
        if let Some((width, height)) = resolution {
            let count = (width as usize).saturating_mul(height as usize);
            if count == 0 || count > 4096 * 4096 {
                continue;
            }
            let mut lods = Vec::new();
            let start = placed.luxels.len();
            for group in &groups {
                let image = raster(group, width, height);
                if image.len() != count {
                    continue;
                }
                for sample in image {
                    push_sample(&mut placed.luxels, sample);
                }
                lods.push(group.lod);
            }
            let samples = placed.luxels.len() - start;
            if samples == 0 || cursor as usize + samples > u32::MAX as usize {
                placed.luxels.truncate(start);
                continue;
            }
            let (ldr, hdr) = ppl_names(&store, index);
            placed.props.push(PropLight {
                ldr,
                hdr,
                checksum: model.checksum,
                first: cursor,
                body: PropBody::Luxels {
                    width,
                    height,
                    lods,
                    ldr_format: ppl_format(&store, index, false).unwrap_or(RGB888),
                    hdr_format: ppl_format(&store, index, true).unwrap_or(RGBA16F),
                },
            });
            cursor += samples as u32;
            continue;
        }
        if prop.flags & NO_VERTEX != 0 {
            continue;
        }
        let mut meshes = Vec::new();
        let start = placed.luxels.len();
        for group in &groups {
            if group.verts.is_empty() {
                continue;
            }
            for vert in &group.verts {
                push_sample(
                    &mut placed.luxels,
                    Sample {
                        position: vert.position,
                        normal: vert.normal.normalize_or_zero(),
                        covered: true,
                    },
                );
            }
            meshes.push(PropVerts {
                lod: group.lod,
                count: group.verts.len() as u32,
            });
        }
        let samples = placed.luxels.len() - start;
        if samples == 0 || cursor as usize + samples > u32::MAX as usize {
            placed.luxels.truncate(start);
            continue;
        }
        let (ldr, hdr) = vhv_names(&store, index);
        placed.props.push(PropLight {
            ldr,
            hdr,
            checksum: model.checksum,
            first: cursor,
            body: PropBody::Vertices { meshes },
        });
        cursor += samples as u32;
    }
    placed
}

#[derive(Clone, Copy)]
struct Sample {
    position: Vec3,
    normal: Vec3,
    covered: bool,
}

fn model_of(
    store: &Store,
    packs: &mut Option<Vec<vpk::Pack>>,
    path: &Path,
    model: &str,
) -> Option<studio::Model> {
    let mdl = file_of(store, packs, path, model)?;
    let stem = model_stem(model);
    let vvd = file_of(store, packs, path, &format!("{stem}.vvd"))?;
    let vtx = ["dx90.vtx", "vtx", "sw.vtx", "dx80.vtx"]
        .iter()
        .find_map(|ext| file_of(store, packs, path, &format!("{stem}.{ext}")));
    studio::load(&mdl, &vvd, &vtx?)
}

fn model_stem(model: &str) -> String {
    let path = model.replace('\\', "/");
    path.trim_end_matches(".mdl")
        .trim_end_matches(".MDL")
        .to_string()
}

fn file_of(
    store: &Store,
    packs: &mut Option<Vec<vpk::Pack>>,
    path: &Path,
    name: &str,
) -> Option<Vec<u8>> {
    let key = normalize(name);
    if let Some(bytes) = store.files.get(&key) {
        return Some(bytes.clone());
    }
    let packs = packs.get_or_insert_with(|| vpk::search(path));
    vpk::read(packs, &key)
}

fn transform_group(group: &Group, matrix: [[f32; 4]; 3]) -> Group {
    Group {
        lod: group.lod,
        verts: group
            .verts
            .iter()
            .map(|vert| Vert {
                position: transform(matrix, vert.position),
                normal: rotate(matrix, vert.normal),
                uv: vert.uv,
            })
            .collect(),
        triangles: group.triangles.clone(),
    }
}

fn push_world(triangles: &mut Vec<Triangle>, group: &Group, tri: [u32; 3]) {
    let corners = tri.map(|index| {
        group
            .verts
            .get(index as usize)
            .map(|vert| vert.position)
            .unwrap_or(Vec3::ZERO)
    });
    let normal = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
    if normal.length_squared() < 1.0e-6 || !normal.is_finite() {
        return;
    }
    triangles.push(Triangle { vertices: corners });
}

fn raster(group: &Group, width: u32, height: u32) -> Vec<Sample> {
    let count = (width as usize).saturating_mul(height as usize);
    let mut image = vec![
        Sample {
            position: Vec3::ZERO,
            normal: Vec3::ZERO,
            covered: false,
        };
        count
    ];
    let w = width as f32;
    let h = height as f32;
    if w <= 0.0 || h <= 0.0 {
        return image;
    }
    for tri in &group.triangles {
        let Some(v0) = group.verts.get(tri[0] as usize).copied() else {
            continue;
        };
        let Some(v1) = group.verts.get(tri[1] as usize).copied() else {
            continue;
        };
        let Some(v2) = group.verts.get(tri[2] as usize).copied() else {
            continue;
        };
        let verts = [v0, v1, v2];
        let uvs = [verts[0].uv, verts[1].uv, verts[2].uv];
        let min_u = uvs.iter().map(|uv| uv[0]).fold(f32::MAX, f32::min);
        let max_u = uvs.iter().map(|uv| uv[0]).fold(f32::MIN, f32::max);
        let min_v = uvs.iter().map(|uv| uv[1]).fold(f32::MAX, f32::min);
        let max_v = uvs.iter().map(|uv| uv[1]).fold(f32::MIN, f32::max);
        if !min_u.is_finite() || !max_u.is_finite() || !min_v.is_finite() || !max_v.is_finite() {
            continue;
        }
        let x0 = (min_u * w).floor().max(0.0) as u32;
        let x1 = (max_u * w).ceil().clamp(0.0, w) as u32;
        let y0 = (min_v * h).floor().max(0.0) as u32;
        let y1 = (max_v * h).ceil().clamp(0.0, h) as u32;
        for y in y0..y1.min(height) {
            for x in x0..x1.min(width) {
                let slot = (y * width + x) as usize;
                if image.get(slot).is_some_and(|sample| sample.covered) {
                    continue;
                }
                let point = [(x as f32 + 0.5) / w, (y as f32 + 0.5) / h];
                let Some([a, b, c]) = barycentric(point, uvs[0], uvs[1], uvs[2]) else {
                    continue;
                };
                let position =
                    verts[0].position * a + verts[1].position * b + verts[2].position * c;
                let normal = (verts[0].normal * a + verts[1].normal * b + verts[2].normal * c)
                    .normalize_or_zero();
                if !position.is_finite() {
                    continue;
                }
                image[slot] = Sample {
                    position,
                    normal,
                    covered: true,
                };
            }
        }
    }
    image
}

fn barycentric(point: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> Option<[f32; 3]> {
    let denom = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if !denom.is_finite() || denom.abs() < 1.0e-8 {
        return None;
    }
    let u = ((b[1] - c[1]) * (point[0] - c[0]) + (c[0] - b[0]) * (point[1] - c[1])) / denom;
    let v = ((c[1] - a[1]) * (point[0] - c[0]) + (a[0] - c[0]) * (point[1] - c[1])) / denom;
    let w = 1.0 - u - v;
    if u >= -1.0e-3 && v >= -1.0e-3 && w >= -1.0e-3 {
        Some([u, v, w])
    } else {
        None
    }
}

fn push_sample(luxels: &mut Vec<Luxel>, sample: Sample) {
    let normal = if sample.covered {
        sample.normal.normalize_or_zero()
    } else {
        Vec3::ZERO
    };
    let corners = if normal == Vec3::ZERO {
        [sample.position; 4]
    } else {
        tangent_quad(sample.position, normal, 3.0)
    };
    luxels.push(Luxel {
        receiver: Receiver {
            position: sample.position,
            normal,
            albedo: Vec3::splat(0.5),
            role: role_of(normal),
        },
        corners,
    });
}

fn role_of(normal: Vec3) -> Role {
    let z = normal.normalize_or_zero().z;
    if z >= 0.7 {
        Role::Floor
    } else if z > -0.7 {
        Role::Wall
    } else {
        Role::Other
    }
}

fn tangent_quad(position: Vec3, normal: Vec3, half: f32) -> [Vec3; 4] {
    let normal = normal.normalize_or_zero();
    let helper = if normal.z.abs() > 0.9 {
        Vec3::X
    } else {
        Vec3::Z
    };
    let tangent = helper.cross(normal).normalize_or_zero() * half;
    let bitangent = normal.cross(tangent.normalize_or_zero()) * half;
    [
        position - tangent - bitangent,
        position + tangent - bitangent,
        position + tangent + bitangent,
        position - tangent + bitangent,
    ]
}

fn angle_matrix(angles: Vec3, origin: Vec3) -> [[f32; 4]; 3] {
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

fn transform(matrix: [[f32; 4]; 3], point: Vec3) -> Vec3 {
    Vec3::new(
        point.x * matrix[0][0] + point.y * matrix[0][1] + point.z * matrix[0][2] + matrix[0][3],
        point.x * matrix[1][0] + point.y * matrix[1][1] + point.z * matrix[1][2] + matrix[1][3],
        point.x * matrix[2][0] + point.y * matrix[2][1] + point.z * matrix[2][2] + matrix[2][3],
    )
}

fn rotate(matrix: [[f32; 4]; 3], point: Vec3) -> Vec3 {
    Vec3::new(
        point.x * matrix[0][0] + point.y * matrix[0][1] + point.z * matrix[0][2],
        point.x * matrix[1][0] + point.y * matrix[1][1] + point.z * matrix[1][2],
        point.x * matrix[2][0] + point.y * matrix[2][1] + point.z * matrix[2][2],
    )
}

fn ppl_names(store: &Store, index: usize) -> (String, String) {
    (
        stored(store, &format!("texelslighting_{index}.ppl")),
        stored(store, &format!("texelslighting_{index}_hdr.ppl")),
    )
}

fn vhv_names(store: &Store, index: usize) -> (String, String) {
    (
        stored(store, &format!("sp_{index}.vhv")),
        stored(store, &format!("sp_hdr_{index}.vhv")),
    )
}

fn stored(store: &Store, canonical: &str) -> String {
    store
        .names
        .iter()
        .find(|name| normalize(name) == canonical)
        .cloned()
        .unwrap_or_else(|| canonical.to_string())
}

fn ppl_size(store: &Store, index: usize) -> Option<(u32, u32)> {
    let (name, _) = ppl_names(store, index);
    let bytes = store.files.get(&normalize(&name))?;
    let info = ppl_header(bytes)?;
    if info.width == 0 || info.height == 0 || info.width > 4096 || info.height > 4096 {
        return None;
    }
    Some((info.width, info.height))
}

fn ppl_format(store: &Store, index: usize, hdr: bool) -> Option<u32> {
    let (ldr, hdr_name) = ppl_names(store, index);
    let name = if hdr { hdr_name } else { ldr };
    let bytes = store.files.get(&normalize(&name))?;
    let format = ppl_header(bytes)?.format;
    if format == RGB888 || format == RGBA16F {
        Some(format)
    } else {
        None
    }
}

struct PplHeader {
    format: u32,
    width: u32,
    height: u32,
}

fn ppl_header(bytes: &[u8]) -> Option<PplHeader> {
    if bytes.len() < 64 {
        return None;
    }
    let meshes = u32_at(bytes, 12)?;
    if meshes == 0 || meshes > 256 {
        return None;
    }
    Some(PplHeader {
        format: u32_at(bytes, 8)?,
        width: u32_at(bytes, 32 + 12)?,
        height: u32_at(bytes, 32 + 16)?,
    })
}

fn pak_store(pak: &[u8]) -> Store {
    let mut store = Store {
        names: Vec::new(),
        files: HashMap::new(),
    };
    if pak.is_empty() {
        return store;
    }
    let Ok(mut archive) = ZipArchive::new(Cursor::new(pak)) else {
        return store;
    };
    for index in 0..archive.len() {
        let Ok(mut file) = archive.by_index(index) else {
            continue;
        };
        let name = file.name().replace('\\', "/");
        let key = name.to_ascii_lowercase();
        let keep = key.ends_with(".mdl")
            || key.ends_with(".vvd")
            || key.ends_with(".vtx")
            || key.ends_with(".ppl")
            || key.ends_with(".vhv");
        store.names.push(name);
        if !keep || file.size() > 64 * 1024 * 1024 {
            continue;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_ok() {
            store.files.insert(key, bytes);
        }
    }
    store
}

fn static_props(bsp: &[u8]) -> Vec<Static> {
    let Some(game) = lump(bsp, GAME_LUMP) else {
        return Vec::new();
    };
    let Some((version, blob)) = prop_blob(bsp, game) else {
        return Vec::new();
    };
    parse_props(version, &blob).unwrap_or_default()
}

fn prop_blob(bsp: &[u8], game: &[u8]) -> Option<(u16, Vec<u8>)> {
    if game.len() < 4 {
        return None;
    }
    let count = i32_at(game, 0)?;
    if count <= 0 || count > 64 {
        return None;
    }
    for index in 0..count as usize {
        let at = 4 + index * 16;
        let id = game.get(at..at + 4)?;
        if id != b"prps" && id != b"sprp" {
            continue;
        }
        let version = u16_at(game, at + 6)?;
        let offset = i32_at(game, at + 8)?;
        let length = i32_at(game, at + 12)?;
        if offset < 0 || length <= 0 {
            return None;
        }
        let start = offset as usize;
        let end = start + length as usize;
        return Some((version, bsp.get(start..end)?.to_vec()));
    }
    None
}

fn parse_props(version: u16, blob: &[u8]) -> Option<Vec<Static>> {
    let dict = i32_at(blob, 0)?;
    if dict < 0 || dict > 4096 {
        return None;
    }
    let mut at = 4usize;
    let mut names = Vec::with_capacity(dict as usize);
    for _ in 0..dict {
        let bytes = blob.get(at..at + 128)?;
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        names.push(String::from_utf8_lossy(&bytes[..end]).into_owned());
        at += 128;
    }
    let leaves = i32_at(blob, at)?;
    if leaves < 0 {
        return None;
    }
    at += 4 + leaves as usize * 2;
    let count = i32_at(blob, at)?;
    at += 4;
    if count <= 0 || count > 100_000 {
        return None;
    }
    let rest = blob.len().checked_sub(at)?;
    if rest % count as usize != 0 {
        return None;
    }
    let stride = rest / count as usize;
    if stride < 56 {
        return None;
    }
    let mut props = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let prop = &blob[at + index * stride..at + (index + 1) * stride];
        let model = u16_at(prop, 24)? as usize;
        let (flags, lightmap) = if version >= 10 && stride >= 72 {
            let flags = u32_at(prop, 64)?;
            let width = u16_at(prop, 68)? as u32;
            let height = u16_at(prop, 70)? as u32;
            let lightmap =
                (width > 0 && height > 0 && flags & NO_TEXEL == 0).then_some((width, height));
            (flags, lightmap)
        } else {
            (u8_at(prop, 31)? as u32 | NO_TEXEL, None)
        };
        props.push(Static {
            origin: vec3(prop, 0)?,
            angles: vec3(prop, 12)?,
            model: names.get(model).cloned().unwrap_or_default(),
            flags,
            lightmap,
        });
    }
    Some(props)
}

fn lump<'a>(data: &'a [u8], index: usize) -> Option<&'a [u8]> {
    if data.len() < 8 + 64 * 16 || &data[..4] != b"VBSP" {
        return None;
    }
    let at = 8 + index * 16;
    let offset = i32_at(data, at)? as usize;
    let length = i32_at(data, at + 4)? as usize;
    data.get(offset..offset + length)
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

fn vec3(data: &[u8], offset: usize) -> Option<Vec3> {
    Some(Vec3::new(
        f32_at(data, offset)?,
        f32_at(data, offset + 4)?,
        f32_at(data, offset + 8)?,
    ))
}

fn f32_at(data: &[u8], offset: usize) -> Option<f32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(f32::from_le_bytes(bytes.try_into().ok()?))
}

fn i32_at(data: &[u8], offset: usize) -> Option<i32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(i32::from_le_bytes(bytes.try_into().ok()?))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn u8_at(data: &[u8], offset: usize) -> Option<u8> {
    data.get(offset).copied()
}
