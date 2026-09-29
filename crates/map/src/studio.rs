//! Studio model triangles. Positions come from the VVD, indices from the VTX.
//! Only the body's own vertices are read; the files stay outside the solver.

use glam::Vec3;

const MODEL_STRIDE: usize = 148;
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
    pub verts: Vec<Vert>,
    pub triangles: Vec<[u32; 3]>,
}

pub struct Model {
    pub checksum: u32,
    pub groups: Vec<Group>,
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
        for model in 0..models {
            let mdl_model = model_at as usize + model as usize * MODEL_STRIDE;
            let vtx_model = vtx_model_at as usize + model as usize * 8;
            read_model(mdl, vtx, &vertices, mdl_model, vtx_model, &mut groups)?;
        }
    }
    if groups.is_empty() {
        return None;
    }
    Some(Model { checksum, groups })
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
    group_base: usize,
    groups: &mut Vec<Group>,
) -> Option<()> {
    let num_verts = i32_at(vtx, group_base)?;
    let vert_at = group_base as i32 + i32_at(vtx, group_base + 4)?;
    let num_indices = i32_at(vtx, group_base + 8)?;
    let index_at = group_base as i32 + i32_at(vtx, group_base + 12)?;
    let num_strips = i32_at(vtx, group_base + 16)?;
    let strip_at = group_base as i32 + i32_at(vtx, group_base + 20)?;
    if num_verts <= 0 || num_verts > 65_536 || num_indices <= 0 || num_strips <= 0 || num_strips > 64
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
                triangles.push([
                    index_of(index)?,
                    index_of(index + 1)?,
                    index_of(index + 2)?,
                ]);
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
