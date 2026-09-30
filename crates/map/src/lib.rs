//! Opens a Source 1 BSP: world triangles, face luxels, model triangles, and a
//! snapshot for writing. A prop with its own lightmap adds luxels; one without
//! adds vertices. Both block rays.

mod bsp;
mod cell;
mod disp;
mod pak;
mod props;
mod studio;
mod vpk;

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
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub vertices: [Vec3; 3],
    pub albedo: Vec3,
}

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

pub struct Map {
    pub path: std::path::PathBuf,
    pub triangles: Vec<Triangle>,
    pub surface: Vec<Surface>,
    pub luxels: Vec<Luxel>,
    pub snapshot: Snapshot,
}

/// Stages of [`open_reporting`]. `done` and `total` belong to that stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadPhase {
    File = 0,
    World = 1,
    Packs = 2,
    Props = 3,
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
    let assembled = bsp::assemble_reporting(&bytes, report)?;
    let placed = props::place_reporting(&bytes, path, assembled.luxels.len() as u32, report);
    let mut triangles = assembled.triangles;
    let mut surface = assembled.surface;
    let mut luxels = assembled.luxels;
    triangles.extend(placed.triangles);
    surface.extend(placed.surface);
    luxels.extend(placed.luxels);
    Ok(Map {
        path: path.to_path_buf(),
        triangles,
        surface,
        luxels,
        snapshot: Snapshot {
            bytes,
            lighting: assembled.lighting,
            lighting_hdr: assembled.lighting_hdr,
            faces: assembled.faces,
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
