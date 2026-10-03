//! Studio model triangles. Positions come from the VVD, indices from the VTX.
//! Only the body's own vertices are read; the files stay outside the solver.

use glam::Vec3;

const MESH_STRIDE: usize = 116;
const VERTEX_STRIDE: usize = 48;
const VTX_HEADER: usize = 36;
const STRIP_GROUP: usize = 25;
const MESH_HEADER: usize = 9;
const VERTEX_HEADER: usize = 9;

#[derive(Clone, Copy)]
pub struct Vert {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: [f32; 2],
}

pub struct Group {
    pub lod: u32,
    /// Mesh slot in the model's skin table.
    pub material: u32,
    pub verts: Vec<Vert>,
    pub triangles: Vec<[u32; 3]>,
}

pub struct Model {
    pub checksum: u32,
    pub groups: Vec<Group>,
    /// Material paths, without the `materials/` prefix.
    pub textures: Vec<String>,
    skins: Vec<u16>,
    skin_width: usize,
}

impl Model {
    /// VMT path for one mesh. `skin` picks a family; the default family is 0.
    pub fn material_name(&self, skin: i32, slot: u32) -> Option<&str> {
        let slot = slot as usize;
        let index = if self.skin_width == 0 || slot >= self.skin_width || self.skins.is_empty() {
            slot
        } else {
            let families = (self.skins.len() / self.skin_width).max(1);
            let family = (skin.max(0) as usize).min(families - 1);
            let at = family * self.skin_width + slot;
            self.skins
                .get(at)
                .copied()
                .map(|id| id as usize)
                .unwrap_or(slot)
        };
        self.textures
            .get(index)
            .map(String::as_str)
            .filter(|name| !name.is_empty())
    }
}

pub fn load(mdl: &[u8], vvd: &[u8], vtx: &[u8]) -> Option<Model> {
    if mdl.len() < 240 || &mdl[..4] != b"IDST" || &vvd[..4] != b"IDSV" || vtx.len() < VTX_HEADER {
        return None;
    }
    let version = i32_at(mdl, 4)?;
    if !(44..=49).contains(&version) || i32_at(vtx, 0)? != 7 {
        return None;
    }
    let checksum = i32_at(mdl, 8)? as u32;
    if i32_at(vvd, 8)? as u32 != checksum || i32_at(vtx, 16)? as u32 != checksum {
        return None;
    }
    let vertices = fixed_vertices(vvd)?;
    let (textures, skins, skin_width) = read_textures(mdl);
    let mut groups = Vec::new();
    let bodyparts = i32_at(mdl, 232)?;
    let body_at = i32_at(mdl, 236)?;
    if bodyparts <= 0 || bodyparts > 32 {
        return None;
    }
    let vtx_bodies = i32_at(vtx, 28)?;
    let vtx_body_at = i32_at(vtx, 32)?;
    if vtx_bodies != bodyparts || vtx_body_at < 0 {
        return None;
    }
    for body in 0..bodyparts {
        let mdl_body = body_at as usize + body as usize * 16;
        let vtx_body = vtx_body_at as usize + body as usize * 8;
        let models = i32_at(mdl, mdl_body + 4)?;
        let model_at = mdl_body as i32 + i32_at(mdl, mdl_body + 12)?;
        let vtx_models = i32_at(vtx, vtx_body)?;
        let vtx_model_at = vtx_body as i32 + i32_at(vtx, vtx_body + 4)?;
        if models != vtx_models || models <= 0 || models > 32 {
            return None;
        }
        // A bodypart's later models are alternatives. The default body draws the first.
        // An empty option (no hardware, no glass) must not throw the rest of the model away.
        let mdl_model = model_at as usize;
        let vtx_model = vtx_model_at as usize;
        if i32_at(mdl, mdl_model + 72).unwrap_or(0) == 0 {
            continue;
        }
        if read_model(mdl, vtx, &vertices, mdl_model, vtx_model, &mut groups).is_none() {
            continue;
        }
    }
    if groups.is_empty() {
        return None;
    }
    Some(Model {
        checksum,
        groups,
        textures,
        skins,
        skin_width,
    })
}

fn read_model(
    mdl: &[u8],
    vtx: &[u8],
    vertices: &[Vert],
    mdl_model: usize,
    vtx_model: usize,
    groups: &mut Vec<Group>,
) -> Option<()> {
    let meshes = i32_at(mdl, mdl_model + 72)?;
    let mesh_at = mdl_model as i32 + i32_at(mdl, mdl_model + 76)?;
    let vertex_index = i32_at(mdl, mdl_model + 84)?;
    if vertex_index < 0 || vertex_index as usize % VERTEX_STRIDE != 0 {
        return None;
    }
    let base = vertex_index as usize / VERTEX_STRIDE;
    let lods = i32_at(vtx, vtx_model)?;
    let lod_at = vtx_model as i32 + i32_at(vtx, vtx_model + 4)?;
    if meshes <= 0 || meshes > 64 || lods <= 0 || lods > 8 {
        return None;
    }
    for lod in 0..lods {
        let lod_base = lod_at as usize + lod as usize * 12;
        let lod_meshes = i32_at(vtx, lod_base)?;
        let mesh_header = lod_base as i32 + i32_at(vtx, lod_base + 4)?;
        if lod_meshes != meshes {
            return None;
        }
        for mesh in 0..meshes {
            let mesh_base = mesh_at as usize + mesh as usize * MESH_STRIDE;
            let slot = i32_at(mdl, mesh_base)?.max(0) as u32;
            let count = i32_at(mdl, mesh_base + 8)?;
            let offset = i32_at(mdl, mesh_base + 12)?;
            if count < 0 || offset < 0 {
                return None;
            }
            let vtx_mesh = mesh_header as usize + mesh as usize * MESH_HEADER;
            let strip_groups = i32_at(vtx, vtx_mesh)?;
            let group_at = vtx_mesh as i32 + i32_at(vtx, vtx_mesh + 4)?;
            if strip_groups < 0 || strip_groups > 16 {
                return None;
            }
            for group in 0..strip_groups {
                let group_base = group_at as usize + group as usize * STRIP_GROUP;
                push_group(
                    vtx,
                    vertices,
                    base,
                    offset as usize,
                    count as usize,
                    lod as u32,
                    slot,
                    group_base,
                    groups,
                )?;
            }
        }
    }
    Some(())
}

fn push_group(
    vtx: &[u8],
    vertices: &[Vert],
    base: usize,
    mesh_offset: usize,
    mesh_count: usize,
    lod: u32,
    material: u32,
    group_base: usize,
    groups: &mut Vec<Group>,
) -> Option<()> {
    let num_verts = i32_at(vtx, group_base)?;
    let vert_at = group_base as i32 + i32_at(vtx, group_base + 4)?;
    let num_indices = i32_at(vtx, group_base + 8)?;
    let index_at = group_base as i32 + i32_at(vtx, group_base + 12)?;
    let num_strips = i32_at(vtx, group_base + 16)?;
    let strip_at = group_base as i32 + i32_at(vtx, group_base + 20)?;
    if num_verts <= 0
        || num_verts > 65_536
        || num_indices <= 0
        || num_strips <= 0
        || num_strips > 64
    {
        return None;
    }
    let mut verts = Vec::with_capacity(num_verts as usize);
    for index in 0..num_verts as usize {
        let at = vert_at as usize + index * VERTEX_HEADER;
        let id = u16_at(vtx, at + 4)? as usize;
        if id >= mesh_count {
            return None;
        }
        let source = base + mesh_offset + id;
        verts.push(vertices.get(source).cloned()?);
    }
    let mut triangles = Vec::new();
    for strip in 0..num_strips as usize {
        let at = strip_at as usize + strip * 27;
        let count = i32_at(vtx, at)?;
        let first = i32_at(vtx, at + 4)?;
        let flags = *vtx.get(at + 18)?;
        if count <= 0 || first < 0 {
            return None;
        }
        let index_of = |slot: i32| -> Option<u32> {
            let id = u16_at(vtx, index_at as usize + (first + slot) as usize * 2)? as usize;
            if id >= num_verts as usize {
                return None;
            }
            Some(id as u32)
        };
        if flags & 0x01 != 0 {
            let mut index = 0;
            while index + 2 < count {
                triangles.push([index_of(index)?, index_of(index + 1)?, index_of(index + 2)?]);
                index += 3;
            }
        } else if flags & 0x02 != 0 {
            for index in 0..count.saturating_sub(2) {
                let mut second = index + 1;
                let mut third = index + 2;
                if index % 2 == 1 {
                    std::mem::swap(&mut second, &mut third);
                }
                triangles.push([index_of(index)?, index_of(second)?, index_of(third)?]);
            }
        }
    }
    if triangles.is_empty() {
        return Some(());
    }
    groups.push(Group {
        lod,
        material,
        verts,
        triangles,
    });
    Some(())
}

fn fixed_vertices(vvd: &[u8]) -> Option<Vec<Vert>> {
    if i32_at(vvd, 4)? != 4 {
        return None;
    }
    let count = i32_at(vvd, 16)?;
    let fixups = i32_at(vvd, 48)?;
    let fixup_at = i32_at(vvd, 52)?;
    let vertex_at = i32_at(vvd, 56)?;
    if count <= 0 || count > 200_000 || vertex_at < 0 {
        return None;
    }
    let read = |index: usize| -> Option<Vert> {
        let at = vertex_at as usize + index * VERTEX_STRIDE;
        let position = vec3(vvd, at + 16)?;
        let normal = vec3(vvd, at + 28)?;
        let uv = [f32_at(vvd, at + 40)?, f32_at(vvd, at + 44)?];
        if !position.is_finite() || !normal.is_finite() || !uv[0].is_finite() || !uv[1].is_finite()
        {
            return None;
        }
        Some(Vert {
            position,
            normal,
            uv,
        })
    };
    if fixups == 0 {
        let mut out = Vec::with_capacity(count as usize);
        for index in 0..count as usize {
            out.push(read(index)?);
        }
        return Some(out);
    }
    if fixups < 0 || fixups > 10_000 || fixup_at < 0 {
        return None;
    }
    let mut out = Vec::with_capacity(count as usize);
    for fixup in 0..fixups as usize {
        let at = fixup_at as usize + fixup * 12;
        let lod = i32_at(vvd, at)?;
        let source = i32_at(vvd, at + 4)?;
        let num = i32_at(vvd, at + 8)?;
        if lod < 0 || source < 0 || num < 0 {
            return None;
        }
        for index in 0..num as usize {
            out.push(read(source as usize + index)?);
        }
    }
    if out.len() != count as usize {
        return None;
    }
    Some(out)
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

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

/// Texture names and the skin table. A bad table leaves the mesh gray.
fn read_textures(mdl: &[u8]) -> (Vec<String>, Vec<u16>, usize) {
    let Some(count) = i32_at(mdl, 204) else {
        return (Vec::new(), Vec::new(), 0);
    };
    if count <= 0 || count > 256 {
        return (Vec::new(), Vec::new(), 0);
    }
    let Some(index) = i32_at(mdl, 208) else {
        return (Vec::new(), Vec::new(), 0);
    };
    if index < 0 {
        return (Vec::new(), Vec::new(), 0);
    }
    let dirs = cd_dirs(mdl);
    let mut textures = Vec::with_capacity(count as usize);
    for item in 0..count as usize {
        let at = index as usize + item * 64;
        let Some(name_at) = i32_at(mdl, at) else {
            return (Vec::new(), Vec::new(), 0);
        };
        let start = at as i32 + name_at;
        if start < 0 {
            return (Vec::new(), Vec::new(), 0);
        }
        let Some(name) = cstring(mdl, start as usize) else {
            return (Vec::new(), Vec::new(), 0);
        };
        textures.push(material_path(&dirs, &name));
    }
    let skin_width = i32_at(mdl, 220).unwrap_or(0).max(0) as usize;
    let families = i32_at(mdl, 224).unwrap_or(0).max(0) as usize;
    let skin_at = i32_at(mdl, 228).unwrap_or(-1);
    if skin_width == 0 || skin_width > 256 || families == 0 || families > 64 || skin_at < 0 {
        return (textures, Vec::new(), 0);
    }
    let total = skin_width * families;
    let mut skins = Vec::with_capacity(total);
    for item in 0..total {
        let Some(id) = u16_at(mdl, skin_at as usize + item * 2) else {
            return (textures, Vec::new(), 0);
        };
        skins.push(id);
    }
    (textures, skins, skin_width)
}

fn cd_dirs(mdl: &[u8]) -> Vec<String> {
    let Some(count) = i32_at(mdl, 212) else {
        return Vec::new();
    };
    let Some(index) = i32_at(mdl, 216) else {
        return Vec::new();
    };
    if count <= 0 || count > 32 || index < 0 {
        return Vec::new();
    }
    let mut dirs = Vec::new();
    for item in 0..count as usize {
        let Some(rel) = i32_at(mdl, index as usize + item * 4) else {
            break;
        };
        let from_file = (rel >= 0).then(|| cstring(mdl, rel as usize)).flatten();
        let from_table = index
            .checked_add(rel)
            .filter(|at| *at >= 0)
            .and_then(|at| cstring(mdl, at as usize));
        let Some(dir) = [from_file, from_table]
            .into_iter()
            .flatten()
            .max_by_key(|text| {
                text.bytes()
                    .filter(|byte| *byte == b'/' || *byte == b'\\')
                    .count()
            })
        else {
            continue;
        };
        dirs.push(dir);
    }
    dirs
}

fn material_path(dirs: &[String], name: &str) -> String {
    let name = name.replace('\\', "/");
    if name.contains('/') || dirs.is_empty() {
        return name.trim_matches('/').to_string();
    }
    let dir = dirs[0].replace('\\', "/");
    let dir = dir.trim_matches('/');
    if dir.is_empty() {
        name
    } else {
        format!("{dir}/{name}")
    }
}

fn cstring(data: &[u8], offset: usize) -> Option<String> {
    let rest = data.get(offset..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    if end == 0 || end > 260 {
        return None;
    }
    let text = std::str::from_utf8(&rest[..end]).ok()?;
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| (32..127).contains(&byte)) {
        return None;
    }
    Some(text.to_string())
}
