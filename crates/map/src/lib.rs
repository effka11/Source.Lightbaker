//! Opens a Source 1 BSP: world triangles, face luxels, model triangles, and a
//! snapshot for writing. A prop with its own lightmap adds luxels; one without
//! adds vertices. Both block rays.

mod bsp;
mod disp;
mod pak;
mod props;
mod studio;
mod vpk;

#[cfg(test)]
mod tests;

use std::fmt;
use std::fs;
use std::io;
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

#[derive(Clone, Copy, Debug)]
pub struct Luxel {
    pub receiver: Receiver,
    pub corners: [Vec3; 4],
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
    pub luxels: Vec<Luxel>,
    pub snapshot: Snapshot,
}

pub fn open(path: impl AsRef<Path>) -> Result<Map, Error> {
    let path = path.as_ref();
    let bytes = fs::read(path)?;
    let assembled = bsp::assemble(&bytes)?;
    let placed = props::place(&bytes, path, assembled.luxels.len() as u32);
    let mut triangles = assembled.triangles;
    let mut luxels = assembled.luxels;
    triangles.extend(placed.triangles);
    luxels.extend(placed.luxels);
    Ok(Map {
        path: path.to_path_buf(),
        triangles,
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
