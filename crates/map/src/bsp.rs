//! Source 1 BSP (version 19 and 20). Luxel positions come from the lightmap
//! sizes already stored on each face. Props are left out.

use glam::{Mat3, Vec3};
use solve::{Receiver, Role, Triangle};

use crate::disp::{self, DispVert};
use crate::pak;
use crate::{Error, FaceLight, Luxel, Span};

const LUMPS: usize = 64;
const FACE: usize = 56;
const SKIP_OCCLUDE: i32 = 0x0002 | 0x0004 | 0x0040 | 0x0100 | 0x0200;
const BUMP: i32 = 0x0800;

const PLANES: usize = 1;
const VERTEXES: usize = 3;
const TEXINFO: usize = 6;
const FACES: usize = 7;
const LIGHTING: usize = 8;
const EDGES: usize = 12;
const SURFEDGES: usize = 13;
const DISPINFO: usize = 26;
const DISP_VERTS: usize = 33;
const DISP_SAMPLES: usize = 34;
const PAKFILE: usize = 40;
const TEXDATA: usize = 2;
const TEXDATA_STRING_DATA: usize = 43;
const TEXDATA_STRING_TABLE: usize = 44;
const DISP_TRIS: usize = 48;
const LIGHTING_HDR: usize = 53;

struct Lump {
    offset: usize,
    length: usize,
}

#[derive(Clone, Copy)]
struct Plane {
    normal: Vec3,
    dist: f32,
}

struct Face {
    plane: usize,
    side: u8,
    first_edge: i32,
    edges: i32,
    texinfo: i32,
    dispinfo: i16,
    styles: [u8; 4],
    light_offset: i32,
    mins: [i32; 2],
    sizes: [i32; 2],
}

struct Tex {
    light_s: [f32; 4],
    light_t: [f32; 4],
    flags: i32,
    texdata: i32,
}

struct Disp {
    start: Vec3,
    vert_start: i32,
    tri_start: i32,
    power: i32,
    sample_start: i32,
}

struct Basis {
    origin: Vec3,
    axis_s: Vec3,
    axis_t: Vec3,
}

impl Basis {
    fn at(&self, s: f32, t: f32) -> Vec3 {
        self.origin + self.axis_s * s + self.axis_t * t
    }
}

pub struct Assembled {
    pub triangles: Vec<Triangle>,
    pub luxels: Vec<Luxel>,
    pub faces: Vec<FaceLight>,
    pub lighting: Span,
    pub lighting_hdr: Span,
}

pub fn assemble(data: &[u8]) -> Result<Assembled, Error> {
    let lumps = header(data)?;
    let planes = planes(slice(data, &lumps[PLANES])?)?;
    let vertices = vertices(slice(data, &lumps[VERTEXES])?)?;
    let edges = edges(slice(data, &lumps[EDGES])?)?;
    let surfedges = surfedges(slice(data, &lumps[SURFEDGES])?)?;
    let faces = faces(slice(data, &lumps[FACES])?)?;
    let texinfos = texinfos(slice(data, &lumps[TEXINFO])?)?;
    let disps = disps(slice(data, &lumps[DISPINFO])?)?;
    let disp_verts = disp_verts(slice(data, &lumps[DISP_VERTS])?)?;
    let samples = slice(data, &lumps[DISP_SAMPLES])?;
    let disp_tris = slice(data, &lumps[DISP_TRIS])?;
    let (names, reflectivity) = texdata(data, &lumps)?;
    let albedo = pak::albedos(slice(data, &lumps[PAKFILE])?, &names, &reflectivity);

    let mut triangles = Vec::new();
    let mut luxels = Vec::new();
    let mut slots = Vec::with_capacity(faces.len());
    let mut grids = 0u32;

    for face in &faces {
        let plane = face_plane(planes.get(face.plane), face.side);
        let points = face_points(face, &surfedges, &edges, &vertices);
        let tex = (face.texinfo >= 0)
            .then(|| texinfos.get(face.texinfo as usize))
            .flatten();
        let flags = tex.map(|tex| tex.flags).unwrap_or(0);
        let block = flags & SKIP_OCCLUDE == 0;
        let (width, height) = grid_size(face);
        let has_grid = face.light_offset >= 0 && width > 0;
        if has_grid {
            grids += 1;
        }
        let first = luxels.len() as u32;
        let color = tex
            .and_then(|tex| albedo.get(tex.texdata as usize))
            .copied()
            .unwrap_or(Vec3::splat(0.5));
        let basis = tex.and_then(|tex| {
            plane.and_then(|plane| basis_of(tex.light_s, tex.light_t, plane.normal, plane.dist))
        });

        let disp = (face.dispinfo >= 0)
            .then(|| disps.get(face.dispinfo as usize))
            .flatten();
        let displaced = disp.and_then(|disp| {
            displace(
                disp,
                &points,
                &disp_verts,
                samples,
                disp_tris,
                plane.map(|plane| plane.normal).unwrap_or(Vec3::Z),
                basis.as_ref(),
                face,
                width,
                height,
                color,
                block,
            )
        });

        if let Some((extra, face_luxels)) = displaced {
            triangles.extend(extra);
            if has_grid {
                luxels.extend(face_luxels);
            }
        } else {
            if block {
                fan(&points, &mut triangles);
            }
            if has_grid {
                if let (Some(plane), Some(basis)) = (plane, basis) {
                    flat_luxels(
                        &basis,
                        plane.normal,
                        face,
                        width,
                        height,
                        color,
                        &mut luxels,
                    );
                }
            }
        }

        slots.push(FaceLight {
            light_offset: face.light_offset,
            width: width.max(0) as u32,
            height: height.max(0) as u32,
            styles: face.styles,
            bumped: flags & BUMP != 0,
            first_luxel: first,
            luxel_count: luxels.len() as u32 - first,
        });
    }

    if grids == 0 || luxels.is_empty() {
        return Err(Error::NoLuxelGrid);
    }

    Ok(Assembled {
        triangles,
        luxels,
        faces: slots,
        lighting: span(&lumps[LIGHTING]),
        lighting_hdr: span(&lumps[LIGHTING_HDR]),
    })
}

fn header(data: &[u8]) -> Result<[Lump; LUMPS], Error> {
    if data.len() < 8 + LUMPS * 16 + 4 || &data[..4] != b"VBSP" {
        return Err(Error::NotAMap);
    }
    let version = i32::from_le_bytes(data[4..8].try_into().unwrap());
    if version != 19 && version != 20 {
        return Err(Error::NotAMap);
    }
    let mut lumps = std::array::from_fn(|_| Lump {
        offset: 0,
        length: 0,
    });
    for index in 0..LUMPS {
        let at = 8 + index * 16;
        let offset = i32::from_le_bytes(data[at..at + 4].try_into().unwrap());
        let length = i32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap());
        if offset < 0 || length < 0 {
            return Err(Error::NotAMap);
        }
        let offset = offset as usize;
        let length = length as usize;
        if offset
            .checked_add(length)
            .map(|end| end > data.len())
            .unwrap_or(true)
        {
            return Err(Error::NotAMap);
        }
        lumps[index] = Lump { offset, length };
    }
    Ok(lumps)
}

fn slice<'a>(data: &'a [u8], lump: &Lump) -> Result<&'a [u8], Error> {
    data.get(lump.offset..lump.offset + lump.length)
        .ok_or(Error::NotAMap)
}

fn span(lump: &Lump) -> Span {
    Span {
        offset: lump.offset as u32,
        length: lump.length as u32,
    }
}

fn planes(data: &[u8]) -> Result<Vec<Plane>, Error> {
    if data.len() % 20 != 0 {
        return Err(Error::NotAMap);
    }
    Ok(data
        .chunks_exact(20)
        .map(|chunk| Plane {
            normal: vec3(chunk, 0),
            dist: f32(chunk, 12),
        })
        .collect())
}

fn vertices(data: &[u8]) -> Result<Vec<Vec3>, Error> {
    if data.len() % 12 != 0 {
        return Err(Error::NotAMap);
    }
    Ok(data.chunks_exact(12).map(|chunk| vec3(chunk, 0)).collect())
}

fn edges(data: &[u8]) -> Result<Vec<(u16, u16)>, Error> {
    if data.len() % 4 != 0 {
        return Err(Error::NotAMap);
    }
    Ok(data
        .chunks_exact(4)
        .map(|chunk| {
            (
                u16::from_le_bytes([chunk[0], chunk[1]]),
                u16::from_le_bytes([chunk[2], chunk[3]]),
            )
        })
        .collect())
}

fn surfedges(data: &[u8]) -> Result<Vec<i32>, Error> {
    if data.len() % 4 != 0 {
        return Err(Error::NotAMap);
    }
    Ok(data
        .chunks_exact(4)
        .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap()))
        .collect())
}

fn faces(data: &[u8]) -> Result<Vec<Face>, Error> {
    if data.len() % FACE != 0 {
        return Err(Error::NotAMap);
    }
    Ok(data
        .chunks_exact(FACE)
        .map(|chunk| Face {
            plane: u16::from_le_bytes([chunk[0], chunk[1]]) as usize,
            side: chunk[2],
            first_edge: i32(chunk, 4),
            edges: i16(chunk, 8) as i32,
            texinfo: i16(chunk, 10) as i32,
            dispinfo: i16(chunk, 12),
            styles: [chunk[16], chunk[17], chunk[18], chunk[19]],
            light_offset: i32(chunk, 20),
            mins: [i32(chunk, 28), i32(chunk, 32)],
            sizes: [i32(chunk, 36), i32(chunk, 40)],
        })
        .collect())
}

fn texinfos(data: &[u8]) -> Result<Vec<Tex>, Error> {
    if data.len() % 72 != 0 {
        return Err(Error::NotAMap);
    }
    Ok(data
        .chunks_exact(72)
        .map(|chunk| Tex {
            light_s: [
                f32(chunk, 32),
                f32(chunk, 36),
                f32(chunk, 40),
                f32(chunk, 44),
            ],
            light_t: [
                f32(chunk, 48),
                f32(chunk, 52),
                f32(chunk, 56),
                f32(chunk, 60),
            ],
            flags: i32(chunk, 64),
            texdata: i32(chunk, 68),
        })
        .collect())
}

fn disps(data: &[u8]) -> Result<Vec<Disp>, Error> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data.len() % 176 != 0 {
        return Err(Error::NotAMap);
    }
    Ok(data
        .chunks_exact(176)
        .map(|chunk| Disp {
            start: vec3(chunk, 0),
            vert_start: i32(chunk, 12),
            tri_start: i32(chunk, 16),
            power: i32(chunk, 20),
            sample_start: i32(chunk, 44),
        })
        .collect())
}

fn disp_verts(data: &[u8]) -> Result<Vec<DispVert>, Error> {
    if data.len() % 20 != 0 {
        return Err(Error::NotAMap);
    }
    Ok(data
        .chunks_exact(20)
        .map(|chunk| DispVert {
            vector: vec3(chunk, 0),
            dist: f32(chunk, 12),
        })
        .collect())
}

fn texdata(data: &[u8], lumps: &[Lump; LUMPS]) -> Result<(Vec<String>, Vec<Vec3>), Error> {
    let tex = slice(data, &lumps[TEXDATA])?;
    if tex.len() % 32 != 0 {
        return Err(Error::NotAMap);
    }
    let strings = slice(data, &lumps[TEXDATA_STRING_DATA])?;
    let table = slice(data, &lumps[TEXDATA_STRING_TABLE])?;
    if table.len() % 4 != 0 {
        return Err(Error::NotAMap);
    }
    let mut names = Vec::new();
    let mut reflectivity = Vec::new();
    for chunk in tex.chunks_exact(32) {
        reflectivity.push(vec3(chunk, 0));
        let id = i32(chunk, 12);
        let name = table
            .get(id as usize * 4..id as usize * 4 + 4)
            .map(|bytes| i32::from_le_bytes(bytes.try_into().unwrap()))
            .and_then(|offset| cstr(strings, offset))
            .unwrap_or_default();
        names.push(name);
    }
    Ok((names, reflectivity))
}

fn cstr(data: &[u8], offset: i32) -> Option<String> {
    if offset < 0 {
        return None;
    }
    let rest = data.get(offset as usize..)?;
    let end = rest
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(rest.len());
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

fn face_plane(plane: Option<&Plane>, side: u8) -> Option<Plane> {
    let plane = plane?;
    if side == 0 {
        Some(Plane {
            normal: plane.normal,
            dist: plane.dist,
        })
    } else {
        Some(Plane {
            normal: -plane.normal,
            dist: -plane.dist,
        })
    }
}

fn face_points(
    face: &Face,
    surfedges: &[i32],
    edges: &[(u16, u16)],
    vertices: &[Vec3],
) -> Vec<Vec3> {
    let mut points = Vec::new();
    if face.edges < 3 {
        return points;
    }
    for step in 0..face.edges {
        let Some(se) = surfedges.get((face.first_edge + step) as usize).copied() else {
            return Vec::new();
        };
        let edge_index = se.unsigned_abs() as usize;
        let Some(&(a, b)) = edges.get(edge_index) else {
            return Vec::new();
        };
        let index = if se >= 0 { a } else { b };
        let Some(point) = vertices.get(index as usize).copied() else {
            return Vec::new();
        };
        points.push(point);
    }
    points
}

fn grid_size(face: &Face) -> (i32, i32) {
    if face.sizes[0] < 0 || face.sizes[1] < 0 {
        return (0, 0);
    }
    let width = face.sizes[0] + 1;
    let height = face.sizes[1] + 1;
    if width > 4096 || height > 4096 {
        return (0, 0);
    }
    (width, height)
}

fn basis_of(light_s: [f32; 4], light_t: [f32; 4], normal: Vec3, dist: f32) -> Option<Basis> {
    let vs = Vec3::new(light_s[0], light_s[1], light_s[2]);
    let vt = Vec3::new(light_t[0], light_t[1], light_t[2]);
    let length = normal.length();
    if length < 1.0e-8 {
        return None;
    }
    let normal = normal / length;
    let dist = dist / length;
    let matrix = Mat3::from_cols(vs, vt, normal).transpose();
    let det = matrix.determinant();
    if !det.is_finite() || det.abs() < 1.0e-8 {
        return None;
    }
    let inverse = matrix.inverse();
    let basis = Basis {
        origin: inverse * Vec3::new(-light_s[3], -light_t[3], dist),
        axis_s: inverse * Vec3::X,
        axis_t: inverse * Vec3::Y,
    };
    if !basis.origin.is_finite() || !basis.axis_s.is_finite() || !basis.axis_t.is_finite() {
        return None;
    }
    Some(basis)
}

fn displace(
    disp: &Disp,
    points: &[Vec3],
    verts: &[DispVert],
    samples: &[u8],
    tags: &[u8],
    face_normal: Vec3,
    basis: Option<&Basis>,
    face: &Face,
    width: i32,
    height: i32,
    albedo: Vec3,
    block: bool,
) -> Option<(Vec<Triangle>, Vec<Luxel>)> {
    if points.len() < 4 || !(2..=4).contains(&disp.power) || disp.vert_start < 0 {
        return None;
    }
    let side = ((1 << disp.power) + 1) as usize;
    let start = disp.vert_start as usize;
    let end = start.checked_add(side * side)?;
    let slice = verts.get(start..end)?;
    let corners = disp::orient([points[0], points[1], points[2], points[3]], disp.start);
    let surface = disp::surface(corners, slice, disp.power)?;
    let tris = disp::triangulation(disp.power);
    let face_normal = face_normal.normalize_or_zero();
    let mut occluders = Vec::new();
    if block {
        for (index, tri) in tris.iter().enumerate() {
            if removed(tags, disp.tri_start, index) {
                continue;
            }
            let corners = tri.map(|slot| surface[slot as usize]);
            let mut wound = corners;
            let normal = (wound[1] - wound[0]).cross(wound[2] - wound[0]);
            if normal.dot(face_normal) < 0.0 {
                wound.swap(0, 1);
            }
            push_tri(&mut occluders, wound);
        }
    }

    let mut luxels = Vec::new();
    if width > 0 && height > 0 && face.light_offset >= 0 {
        let count = (width * height) as usize;
        let decoded = if disp.sample_start >= 0 {
            disp::read_samples(samples, disp.sample_start as usize, count, tris.len())
        } else {
            vec![None; count]
        };
        for t in 0..height {
            for s in 0..width {
                let slot = (t * width + s) as usize;
                let placed = decoded.get(slot).and_then(|sample| {
                    sample.and_then(|(tri, bary)| {
                        sample_point(&surface, &tris[tri], bary, face_normal)
                    })
                });
                let (position, normal) = placed.unwrap_or_else(|| {
                    let u = (s as f32 + 0.5) / width as f32;
                    let v = (t as f32 + 0.5) / height as f32;
                    (disp::bilinear(corners, u, v), face_normal)
                });
                let normal = if normal == Vec3::ZERO {
                    face_normal
                } else {
                    normal
                };
                let corners = luxel_corners(basis, face, s, t, position, normal);
                push_luxel(&mut luxels, position, normal, albedo, corners);
            }
        }
    }
    Some((occluders, luxels))
}

fn sample_point(
    surface: &[Vec3],
    tri: &[u32; 3],
    bary: [f32; 3],
    face_normal: Vec3,
) -> Option<(Vec3, Vec3)> {
    let v0 = *surface.get(tri[0] as usize)?;
    let v1 = *surface.get(tri[1] as usize)?;
    let v2 = *surface.get(tri[2] as usize)?;
    let sum = bary[0] + bary[1] + bary[2];
    if sum < 1.0e-3 {
        return None;
    }
    let position = (v0 * bary[0] + v1 * bary[1] + v2 * bary[2]) / sum;
    if !position.is_finite() {
        return None;
    }
    let mut normal = (v1 - v0).cross(v2 - v0).normalize_or_zero();
    if normal.dot(face_normal) < 0.0 {
        normal = -normal;
    }
    Some((position, normal))
}

fn removed(tags: &[u8], tri_start: i32, index: usize) -> bool {
    if tri_start < 0 {
        return false;
    }
    let offset = (tri_start as usize + index) * 2;
    let Some(bytes) = tags.get(offset..offset + 2) else {
        return false;
    };
    let tag = u16::from_le_bytes([bytes[0], bytes[1]]);
    tag & (1 << 5) != 0
}

fn flat_luxels(
    basis: &Basis,
    normal: Vec3,
    face: &Face,
    width: i32,
    height: i32,
    albedo: Vec3,
    luxels: &mut Vec<Luxel>,
) {
    let normal = normal.normalize_or_zero();
    for t in 0..height {
        for s in 0..width {
            let s0 = (face.mins[0] + s) as f32;
            let t0 = (face.mins[1] + t) as f32;
            let position = basis.at(s0 + 0.5, t0 + 0.5);
            let corners = [
                basis.at(s0, t0),
                basis.at(s0 + 1.0, t0),
                basis.at(s0 + 1.0, t0 + 1.0),
                basis.at(s0, t0 + 1.0),
            ];
            push_luxel(luxels, position, normal, albedo, corners);
        }
    }
}

fn luxel_corners(
    basis: Option<&Basis>,
    face: &Face,
    s: i32,
    t: i32,
    position: Vec3,
    normal: Vec3,
) -> [Vec3; 4] {
    let Some(basis) = basis else {
        return tangent_quad(position, normal, 8.0);
    };
    let s0 = (face.mins[0] + s) as f32;
    let t0 = (face.mins[1] + t) as f32;
    let shift = position - basis.at(s0 + 0.5, t0 + 0.5);
    let mut corners = [
        basis.at(s0, t0) + shift,
        basis.at(s0 + 1.0, t0) + shift,
        basis.at(s0 + 1.0, t0 + 1.0) + shift,
        basis.at(s0, t0 + 1.0) + shift,
    ];
    if normal != Vec3::ZERO {
        for corner in &mut corners {
            let drop = (*corner - position).dot(normal);
            *corner -= normal * drop;
        }
    }
    corners
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

fn push_luxel(
    luxels: &mut Vec<Luxel>,
    position: Vec3,
    normal: Vec3,
    albedo: Vec3,
    corners: [Vec3; 4],
) {
    if !position.is_finite() || !normal.is_finite() {
        return;
    }
    luxels.push(Luxel {
        receiver: Receiver {
            position,
            normal,
            albedo,
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

fn fan(points: &[Vec3], triangles: &mut Vec<Triangle>) {
    if points.len() < 3 {
        return;
    }
    for index in 1..points.len() - 1 {
        push_tri(triangles, [points[0], points[index], points[index + 1]]);
    }
}

fn push_tri(triangles: &mut Vec<Triangle>, corners: [Vec3; 3]) {
    let normal = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
    if normal.length_squared() < 1.0e-6 {
        return;
    }
    triangles.push(Triangle { vertices: corners });
}

fn vec3(data: &[u8], offset: usize) -> Vec3 {
    Vec3::new(
        f32(data, offset),
        f32(data, offset + 4),
        f32(data, offset + 8),
    )
}

fn f32(data: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn i32(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn i16(data: &[u8], offset: usize) -> i16 {
    i16::from_le_bytes([data[offset], data[offset + 1]])
}
