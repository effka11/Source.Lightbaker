//! Source 1 BSP (version 19 and 20). Luxel positions come from the lightmap
//! sizes already stored on each face. Props are left out.

use glam::{Mat3, Vec2, Vec3};
use solve::{faces_solid, Receiver, Role, Triangle};

use crate::cell::Outline;
use crate::disp::{self, DispVert};
use crate::pak;
use crate::{Error, FaceLight, Luxel, Span, Surface};

const LUMPS: usize = 64;
const FACE: usize = 56;
const SKIP_OCCLUDE: i32 = 0x0002 | 0x0004 | 0x0040 | 0x0100 | 0x0200;
const NODRAW: i32 = 0x0080;
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

impl Tex {
    /// Lightmap coordinates of a world point.
    fn luxel(&self, point: Vec3) -> Vec2 {
        let s = Vec3::new(self.light_s[0], self.light_s[1], self.light_s[2]);
        let t = Vec3::new(self.light_t[0], self.light_t[1], self.light_t[2]);
        Vec2::new(
            point.dot(s) + self.light_s[3],
            point.dot(t) + self.light_t[3],
        )
    }
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
    pub surface: Vec<Surface>,
    pub luxels: Vec<Luxel>,
    pub faces: Vec<FaceLight>,
    pub lighting: Span,
    pub lighting_hdr: Span,
}

#[allow(dead_code)] // unit tests assemble a map without reporting progress
pub fn assemble(data: &[u8]) -> Result<Assembled, Error> {
    assemble_reporting(data, &|_, _, _| {})
}

pub(crate) fn assemble_reporting(
    data: &[u8],
    report: &(dyn Fn(crate::LoadPhase, u64, u64) + Sync),
) -> Result<Assembled, Error> {
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
    let mut surface = Vec::new();
    let mut luxels = Vec::new();
    let mut slots = Vec::with_capacity(faces.len());
    let mut grids = 0u32;
    let world_steps = faces.len() as u64 + 1;

    for (index, face) in faces.iter().enumerate() {
        let plane = face_plane(planes.get(face.plane), face.side);
        let points = face_points(face, &surfedges, &edges, &vertices);
        let tex = (face.texinfo >= 0)
            .then(|| texinfos.get(face.texinfo as usize))
            .flatten();
        let flags = tex.map(|tex| tex.flags).unwrap_or(0);
        let block = flags & SKIP_OCCLUDE == 0;
        let draw = flags & (SKIP_OCCLUDE | NODRAW) == 0;
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
                draw,
                &mut triangles,
                &mut surface,
            )
        });

        if let Some(face_luxels) = displaced {
            if has_grid {
                luxels.extend(face_luxels);
            }
        } else {
            if block {
                fan(&points, &mut triangles);
            }
            if draw {
                fan_surface(&points, color, &mut surface);
            }
            if has_grid {
                if let (Some(plane), Some(basis), Some(tex)) = (plane, basis, tex) {
                    flat_luxels(
                        &basis,
                        plane.normal,
                        tex,
                        &points,
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
        report(crate::LoadPhase::World, index as u64 + 1, world_steps);
    }
    turn_faces(
        &triangles,
        &mut luxels,
        &slots
            .iter()
            .map(|face| (face.first_luxel, face.luxel_count))
            .collect::<Vec<_>>(),
    );
    report(crate::LoadPhase::World, world_steps, world_steps);

    if grids == 0 || luxels.is_empty() {
        return Err(Error::NoLuxelGrid);
    }

    Ok(Assembled {
        triangles,
        surface,
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
    draw: bool,
    triangles: &mut Vec<Triangle>,
    drawn: &mut Vec<Surface>,
) -> Option<Vec<Luxel>> {
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
    if block || draw {
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
            if block {
                push_tri(triangles, wound);
            }
            if draw {
                push_surface(drawn, wound, albedo);
            }
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
                    let u = s as f32 / (width - 1).max(1) as f32;
                    let v = t as f32 / (height - 1).max(1) as f32;
                    (disp::bilinear(corners, u, v), face_normal)
                });
                let normal = if normal == Vec3::ZERO {
                    face_normal
                } else {
                    normal
                };
                let corners = disp::cell(&surface, disp.power, s, t, width, height)
                    .map(|corners| corners.to_vec())
                    .unwrap_or_else(|| luxel_corners(basis, face, s, t, position, normal));
                push_luxel(&mut luxels, position, normal, albedo, corners);
            }
        }
    }
    Some(luxels)
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

/// Luxel `(s, t)` is one texel. Its square runs from `(mins + s, mins + t)`
/// to the next integer, and the engine shows that texel at the square's
/// center. Light is gathered where the square meets the face, so a square a
/// wall cuts samples the face, not the solid beside it. A square off the face
/// borrows the nearest point inside and draws nothing.
#[allow(clippy::too_many_arguments)]
fn flat_luxels(
    basis: &Basis,
    normal: Vec3,
    tex: &Tex,
    points: &[Vec3],
    face: &Face,
    width: i32,
    height: i32,
    albedo: Vec3,
    luxels: &mut Vec<Luxel>,
) {
    let normal = normal.normalize_or_zero();
    let outline: Vec<Vec2> = points.iter().map(|point| tex.luxel(*point)).collect();
    let outline = Outline::new(&outline);
    for t in 0..height {
        for s in 0..width {
            let center = Vec2::new(
                (face.mins[0] + s) as f32 + 0.5,
                (face.mins[1] + t) as f32 + 0.5,
            );
            let (sample, piece) = match &outline {
                Some(outline) => {
                    let cell = outline.cell(center);
                    (cell.sample, cell.piece)
                }
                None => (center, Vec::new()),
            };
            let corners = piece
                .iter()
                .map(|point| basis.at(point.x, point.y))
                .collect();
            push_luxel(
                luxels,
                basis.at(sample.x, sample.y),
                normal,
                albedo,
                corners,
            );
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
) -> Vec<Vec3> {
    let Some(basis) = basis else {
        return tangent_quad(position, normal, 8.0);
    };
    let s0 = (face.mins[0] + s) as f32;
    let t0 = (face.mins[1] + t) as f32;
    let shift = position - basis.at(s0, t0);
    let mut corners = vec![
        basis.at(s0 - 0.5, t0 - 0.5) + shift,
        basis.at(s0 + 0.5, t0 - 0.5) + shift,
        basis.at(s0 + 0.5, t0 + 0.5) + shift,
        basis.at(s0 - 0.5, t0 + 0.5) + shift,
    ];
    if normal != Vec3::ZERO {
        for corner in &mut corners {
            let drop = (*corner - position).dot(normal);
            *corner -= normal * drop;
        }
    }
    corners
}

fn tangent_quad(position: Vec3, normal: Vec3, half: f32) -> Vec<Vec3> {
    let normal = normal.normalize_or_zero();
    let helper = if normal.z.abs() > 0.9 {
        Vec3::X
    } else {
        Vec3::Z
    };
    let tangent = helper.cross(normal).normalize_or_zero() * half;
    let bitangent = normal.cross(tangent.normalize_or_zero()) * half;
    vec![
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
    corners: Vec<Vec3>,
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

/// One probe per face. A face that looks into its own slab is turned toward
/// the open side, so a lamp in the room reaches that wall.
pub(crate) fn turn_faces(triangles: &[Triangle], luxels: &mut [Luxel], faces: &[(u32, u32)]) {
    let mut probes = Vec::new();
    let mut spans = Vec::new();
    for &(first, count) in faces {
        if count == 0 {
            continue;
        }
        let index = first as usize + count as usize / 2;
        let receiver = &luxels[index].receiver;
        probes.push((receiver.position, receiver.normal));
        spans.push((first, count));
    }
    for (into_solid, (first, count)) in faces_solid(triangles, &probes).into_iter().zip(spans) {
        if !into_solid {
            continue;
        }
        let start = first as usize;
        let end = start + count as usize;
        for luxel in &mut luxels[start..end] {
            luxel.receiver.normal = -luxel.receiver.normal;
            luxel.receiver.role = role_of(luxel.receiver.normal);
        }
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

fn fan_surface(points: &[Vec3], albedo: Vec3, drawn: &mut Vec<Surface>) {
    if points.len() < 3 {
        return;
    }
    for index in 1..points.len() - 1 {
        push_surface(
            drawn,
            [points[0], points[index], points[index + 1]],
            albedo,
        );
    }
}

fn push_tri(triangles: &mut Vec<Triangle>, corners: [Vec3; 3]) {
    if !stands(corners) {
        return;
    }
    triangles.push(Triangle { vertices: corners });
}

fn push_surface(drawn: &mut Vec<Surface>, corners: [Vec3; 3], albedo: Vec3) {
    if !stands(corners) {
        return;
    }
    drawn.push(Surface {
        vertices: corners,
        albedo,
    });
}

fn stands(corners: [Vec3; 3]) -> bool {
    let normal = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
    normal.length_squared() >= 1.0e-6
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

/// Longest drawn edge of each displacement, divided by one grid step of its quad.
#[cfg(test)]
pub(crate) fn terrain_stretch(data: &[u8]) -> Vec<f32> {
    let lumps = header(data).expect("header");
    let vertices = vertices(slice(data, &lumps[VERTEXES]).unwrap_or(&[])).unwrap_or_default();
    let edges = edges(slice(data, &lumps[EDGES]).unwrap_or(&[])).unwrap_or_default();
    let surfedges = surfedges(slice(data, &lumps[SURFEDGES]).unwrap_or(&[])).unwrap_or_default();
    let faces = faces(slice(data, &lumps[FACES]).unwrap_or(&[])).unwrap_or_default();
    let disps = disps(slice(data, &lumps[DISPINFO]).unwrap_or(&[])).unwrap_or_default();
    let disp_verts = disp_verts(slice(data, &lumps[DISP_VERTS]).unwrap_or(&[])).unwrap_or_default();
    let mut ratios = Vec::new();
    for face in &faces {
        if face.dispinfo < 0 {
            continue;
        }
        let Some(disp) = disps.get(face.dispinfo as usize) else {
            continue;
        };
        let points = face_points(face, &surfedges, &edges, &vertices);
        if points.len() < 4 || !(2..=4).contains(&disp.power) || disp.vert_start < 0 {
            continue;
        }
        let corners = disp::orient([points[0], points[1], points[2], points[3]], disp.start);
        let side = (1 << disp.power) + 1;
        let count = (side * side) as usize;
        let start = disp.vert_start as usize;
        let Some(slice) = disp_verts.get(start..start + count) else {
            continue;
        };
        let Some(surface) = disp::surface(corners, slice, disp.power) else {
            continue;
        };
        let step = (corners[1] - corners[0])
            .length()
            .max((corners[3] - corners[0]).length())
            / (side - 1) as f32;
        if step < 1.0 {
            continue;
        }
        let mut max_edge = 0.0f32;
        for tri in disp::triangulation(disp.power) {
            let a = surface[tri[0] as usize];
            let b = surface[tri[1] as usize];
            let c = surface[tri[2] as usize];
            max_edge = max_edge
                .max((a - b).length())
                .max((b - c).length())
                .max((c - a).length());
        }
        ratios.push(max_edge / step);
    }
    ratios
}

/// Median gap along brush edges that two displacements both own.
#[cfg(test)]
pub(crate) fn shared_seam_median(data: &[u8]) -> f32 {
    let lumps = header(data).expect("header");
    let vertices = vertices(slice(data, &lumps[VERTEXES]).unwrap_or(&[])).unwrap_or_default();
    let edges = edges(slice(data, &lumps[EDGES]).unwrap_or(&[])).unwrap_or_default();
    let surfedges = surfedges(slice(data, &lumps[SURFEDGES]).unwrap_or(&[])).unwrap_or_default();
    let faces = faces(slice(data, &lumps[FACES]).unwrap_or(&[])).unwrap_or_default();
    let disps = disps(slice(data, &lumps[DISPINFO]).unwrap_or(&[])).unwrap_or_default();
    let disp_verts = disp_verts(slice(data, &lumps[DISP_VERTS]).unwrap_or(&[])).unwrap_or_default();
    let mut quads = Vec::new();
    for face in &faces {
        if face.dispinfo < 0 {
            continue;
        }
        let Some(disp) = disps.get(face.dispinfo as usize) else {
            continue;
        };
        let points = face_points(face, &surfedges, &edges, &vertices);
        if points.len() < 4 || !(2..=4).contains(&disp.power) || disp.vert_start < 0 {
            continue;
        }
        let corners = disp::orient([points[0], points[1], points[2], points[3]], disp.start);
        let side = (1 << disp.power) + 1;
        let count = (side * side) as usize;
        let start = disp.vert_start as usize;
        let Some(slice) = disp_verts.get(start..start + count) else {
            continue;
        };
        let Some(surface) = disp::surface(corners, slice, disp.power) else {
            continue;
        };
        quads.push((corners, surface, side));
    }
    let mut gaps = Vec::new();
    for left in 0..quads.len() {
        for right in left + 1..quads.len() {
            for edge in 0..4 {
                let a0 = quads[left].0[edge];
                let a1 = quads[left].0[(edge + 1) % 4];
                for other in 0..4 {
                    let b0 = quads[right].0[other];
                    let b1 = quads[right].0[(other + 1) % 4];
                    let forward = (a0 - b0).length_squared() < 4.0 && (a1 - b1).length_squared() < 4.0;
                    let backward =
                        (a0 - b1).length_squared() < 4.0 && (a1 - b0).length_squared() < 4.0;
                    if !forward && !backward {
                        continue;
                    }
                    gaps.push(edge_gap(&quads[left], edge, &quads[right], other, backward));
                }
            }
        }
    }
    gaps.sort_by(|left, right| left.total_cmp(right));
    gaps[gaps.len() / 2]
}

#[cfg(test)]
fn edge_gap(
    left: &([Vec3; 4], Vec<Vec3>, i32),
    left_edge: usize,
    right: &([Vec3; 4], Vec<Vec3>, i32),
    right_edge: usize,
    backward: bool,
) -> f32 {
    let sample = |surface: &[Vec3], side: i32, edge: usize, t: f32| -> Vec3 {
        let size = side - 1;
        let along = (t.clamp(0.0, 1.0) * size as f32).round() as i32;
        let (x, y) = match edge {
            0 => (along, 0),
            1 => (size, along),
            2 => (size - along, size),
            _ => (0, size - along),
        };
        surface[(y * side + x) as usize]
    };
    let mut gap = 0.0f32;
    for step in 0..=8 {
        let t = step as f32 / 8.0;
        let p = sample(&left.1, left.2, left_edge, t);
        let q = sample(&right.1, right.2, right_edge, if backward { 1.0 - t } else { t });
        gap = gap.max((p - q).length());
    }
    gap
}
