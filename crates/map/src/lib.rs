//! Opens a Source 1 BSP: world triangles, face luxels, model triangles, and a
//! snapshot for writing. A prop with its own lightmap adds luxels; one without
//! adds vertices. Both block rays.

mod bsp;
mod cell;
mod disp;
mod door;
mod pak;
mod picture;
mod props;
mod studio;
mod vpk;
mod vtf;

#[cfg(test)]
mod tests;

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use glam::Vec3;
use solve::{Receiver, Triangle};

pub use solve::Role;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    NotAMap,
    NoLuxelGrid,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "файл не читается: {err}"),
            Error::NotAMap => write!(f, "это не карта Source 1"),
            Error::NoLuxelGrid => write!(f, "в карте нет сетки люкселей"),
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

/// One triangle of a face the game draws. Nodraw, sky, trigger, hint and skip
/// stay in [`Map::triangles`] for rays and are left out of this list.
/// `uv` is the texture-space texel of each corner. `blend` is the displacement
/// vertex alpha, 0 for the base texture and 1 for the second one.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub vertices: [Vec3; 3],
    pub albedo: Vec3,
    pub uv: [[f32; 2]; 3],
    pub blend: [f32; 3],
    /// Index into [`Map::materials`], or [`NO_MATERIAL`] when the face has none.
    pub material: u32,
    /// Luxel-grid coordinate of each corner, in texels from the face origin.
    /// Meaningful when [`Self::light_face`] indexes the world faces followed by
    /// [`prop_grids`].
    pub light_uv: [[f32; 2]; 3],
    /// Face whose luxel grid covers this triangle, or `u32::MAX` when it has none.
    pub light_face: u32,
    /// Luxel of each corner for a vertex-lit prop. `u32::MAX` uses [`Self::light_face`].
    pub luxel: [u32; 3],
    /// Prop mesh. The figure tints the side that faces its center.
    pub prop: bool,
    /// Outward unit normal at each corner. A lamp tints a prop only where this
    /// points toward the figure's center.
    pub normal: [Vec3; 3],
}

/// No entry in [`Map::materials`].
pub const NO_MATERIAL: u32 = u32::MAX;

pub use door::{Door, DoorPose};
pub use picture::Material;
pub use vtf::Image;

/// One receiver of the map plus the patch of surface it covers.
#[derive(Clone, Debug)]
pub struct Luxel {
    pub receiver: Receiver,
    /// Convex outline of the luxel on its surface, clipped to the face it
    /// belongs to. Empty when the luxel's cell lies off the face: its light is
    /// still written, but there is nothing to draw.
    pub corners: Vec<Vec3>,
}

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub offset: u32,
    pub length: u32,
}

/// Where a face's light samples sit in the lighting lumps.
#[derive(Clone, Copy, Debug)]
pub struct FaceLight {
    pub light_offset: i32,
    pub width: u32,
    pub height: u32,
    pub styles: [u8; 4],
    pub bumped: bool,
    pub first_luxel: u32,
    pub luxel_count: u32,
}

/// Copy of the BSP plus the ranges a later write replaces with new light.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub bytes: Vec<u8>,
    pub lighting: Span,
    pub lighting_hdr: Span,
    pub faces: Vec<FaceLight>,
    pub props: Vec<PropLight>,
}

/// Prop light that replaces one pair of files inside the copied pak.
/// Samples are contiguous in the light array, starting at `first`.
#[derive(Clone, Debug)]
pub struct PropLight {
    pub ldr: String,
    pub hdr: String,
    pub checksum: u32,
    pub first: u32,
    pub body: PropBody,
}

#[derive(Clone, Debug)]
pub enum PropBody {
    /// One image per mesh, each `width * height` samples, in file order.
    Luxels {
        width: u32,
        height: u32,
        lods: Vec<u32>,
        ldr_format: u32,
        hdr_format: u32,
    },
    Vertices {
        meshes: Vec<PropVerts>,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct PropVerts {
    pub lod: u32,
    pub count: u32,
}

impl PropLight {
    pub fn samples(&self) -> usize {
        match &self.body {
            PropBody::Luxels {
                width,
                height,
                lods,
                ..
            } => (*width as usize)
                .saturating_mul(*height as usize)
                .saturating_mul(lods.len()),
            PropBody::Vertices { meshes } => meshes
                .iter()
                .map(|mesh| mesh.count as usize)
                .fold(0usize, usize::saturating_add),
        }
    }
}

/// Lightmap images of static props, LOD 0 only, in the same order the meshes
/// were stamped. Vertex-lit props are not grids: each corner names its luxel.
pub fn prop_grids(props: &[PropLight]) -> Vec<FaceLight> {
    let mut faces = Vec::new();
    for prop in props {
        let PropBody::Luxels {
            width,
            height,
            lods,
            ..
        } = &prop.body
        else {
            continue;
        };
        let count = width.saturating_mul(*height);
        if count == 0 {
            continue;
        }
        let mut offset = 0u32;
        for lod in lods {
            if *lod == 0 {
                faces.push(FaceLight {
                    light_offset: -1,
                    width: *width,
                    height: *height,
                    styles: [0, 255, 255, 255],
                    bumped: false,
                    first_luxel: prop.first.saturating_add(offset),
                    luxel_count: count,
                });
            }
            offset = offset.saturating_add(count);
        }
    }
    faces
}

pub struct Map {
    pub path: std::path::PathBuf,
    pub triangles: Vec<Triangle>,
    pub surface: Vec<Surface>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub luxels: Vec<Luxel>,
    pub doors: Vec<Door>,
    pub snapshot: Snapshot,
}

/// Stages of [`open_reporting`]. `done` and `total` belong to that stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadPhase {
    File = 0,
    World = 1,
    Packs = 2,
    Props = 3,
    Textures = 4,
}

pub fn open(path: impl AsRef<Path>) -> Result<Map, Error> {
    open_reporting(path, &|_, _, _| {})
}

pub fn open_reporting(
    path: impl AsRef<Path>,
    report: &(dyn Fn(LoadPhase, u64, u64) + Sync),
) -> Result<Map, Error> {
    let path = path.as_ref();
    let bytes = read_reported(path, report)?;
    let assembled = bsp::assemble_reporting(&bytes, Some(path), report)?;
    let bsp::Assembled {
        triangles,
        mut surface,
        luxels,
        faces,
        lighting,
        lighting_hdr,
        names,
        doors,
    } = assembled;
    let used: Vec<u32> = surface.iter().map(|face| face.material).collect();
    let mut catalog = picture::Catalog::world(&bytes, path, &names, &used, report);
    let placed = props::place_reporting(
        &bytes,
        path,
        luxels.len() as u32,
        faces.len() as u32,
        &mut catalog,
        report,
    );
    let (materials, images) = catalog.finish();
    let mut triangles = triangles;
    surface.extend(placed.surface);
    let mut luxels = luxels;
    triangles.extend(placed.triangles);
    luxels.extend(placed.luxels);
    let mut doors = doors;
    doors.extend(placed.doors);
    for (index, door) in doors.iter_mut().enumerate() {
        if door.name.is_empty() {
            door.name = format!("Дверь {}", index + 1);
        }
    }
    Ok(Map {
        path: path.to_path_buf(),
        triangles,
        surface,
        materials,
        images,
        luxels,
        doors,
        snapshot: Snapshot {
            bytes,
            lighting,
            lighting_hdr,
            faces,
            props: placed.props,
        },
    })
}

fn read_reported(
    path: &Path,
    report: &(dyn Fn(LoadPhase, u64, u64) + Sync),
) -> Result<Vec<u8>, Error> {
    let mut file = File::open(path)?;
    let total = file.metadata().map(|meta| meta.len()).unwrap_or(0).max(1);
    let mut bytes = Vec::new();
    let mut buf = [0u8; 256 * 1024];
    let mut done = 0u64;
    report(LoadPhase::File, 0, total);
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..read]);
        done += read as u64;
        report(LoadPhase::File, done.min(total), total);
    }
    report(LoadPhase::File, total, total);
    Ok(bytes)
}
