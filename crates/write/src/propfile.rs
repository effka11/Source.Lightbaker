//! Replaces prop luxels and vertex colors inside a copied pak.
//! Model and material bytes are copied compressed, unchanged.

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};

use map::{PropBody, PropLight};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::Error;

const RGB888: u32 = 2;
const RGBA16F: u32 = 24;

pub fn repack(pak: &[u8], props: &[PropLight], light: &[[f32; 3]]) -> Result<Vec<u8>, Error> {
    let mut files = Vec::with_capacity(props.len() * 2);
    for prop in props {
        let samples = samples(light, prop)?;
        match &prop.body {
            PropBody::Luxels {
                width,
                height,
                lods,
                ldr_format,
                hdr_format,
            } => {
                let mesh = (*width as usize).saturating_mul(*height as usize);
                if mesh == 0 || samples.len() != mesh * lods.len() {
                    return Err(Error::Light);
                }
                files.push((
                    prop.ldr.clone(),
                    ppl(prop.checksum, *ldr_format, *width, *height, lods, &samples),
                ));
                files.push((
                    prop.hdr.clone(),
                    ppl(prop.checksum, *hdr_format, *width, *height, lods, &samples),
                ));
            }
            PropBody::Vertices { meshes } => {
                let expected: usize = meshes.iter().map(|mesh| mesh.count as usize).sum();
                if samples.len() != expected {
                    return Err(Error::Light);
                }
                let bytes = vhv(prop.checksum, meshes, &samples);
                files.push((prop.ldr.clone(), bytes.clone()));
                files.push((prop.hdr.clone(), bytes));
            }
        }
    }
    write_zip(pak, &files)
}

fn samples<'a>(light: &'a [[f32; 3]], prop: &PropLight) -> Result<&'a [[f32; 3]], Error> {
    let start = prop.first as usize;
    let end = start.checked_add(prop.samples()).ok_or(Error::Light)?;
    light.get(start..end).ok_or(Error::Light)
}

fn write_zip(pak: &[u8], replacements: &[(String, Vec<u8>)]) -> Result<Vec<u8>, Error> {
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut cursor);
    let kept = if pak.is_empty() {
        replacements.to_vec()
    } else {
        let mut archive = ZipArchive::new(Cursor::new(pak)).map_err(|_| Error::NotAMap)?;
        // Source finds the pak by the XZP1 comment. A zip without it does not
        // mount, and the load hangs.
        let comment = archive.comment().to_vec();
        if !comment.is_empty() {
            writer.set_raw_comment(comment.into_boxed_slice());
        }
        let kept: Vec<(String, Vec<u8>)> = replacements
            .iter()
            .filter(|(name, bytes)| may_replace(&mut archive, name, bytes))
            .cloned()
            .collect();
        let skip: HashSet<String> = kept.iter().map(|(name, _)| normalize(name)).collect();
        let names: Vec<String> = archive.file_names().map(|name| name.to_string()).collect();
        for (index, name) in names.iter().enumerate() {
            if skip.contains(&normalize(name)) {
                continue;
            }
            let file = archive.by_index_raw(index).map_err(|_| Error::NotAMap)?;
            writer.raw_copy_file(file).map_err(|_| Error::NotAMap)?;
        }
        kept
    };
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in &kept {
        writer
            .start_file(name, options)
            .map_err(|_| Error::NotAMap)?;
        writer.write_all(bytes).map_err(|_| Error::NotAMap)?;
    }
    writer.finish().map_err(|_| Error::NotAMap)?;
    Ok(cursor.into_inner())
}

/// Replace a lighting file that is already in the pak. The bytes are the mesh
/// of the model the game loads now. The file on disk can be an older model
/// with the same name: keeping it makes Garry's Mod drop the prop once the
/// map is no longer fullbright. A file that was not there is not created.
/// An invented `sp_hdr_` on an LDR map is what the engine then tries to read.
fn may_replace(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str, _bytes: &[u8]) -> bool {
    named(archive, name).is_some()
}

fn named(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let want = normalize(name);
    let index = archive
        .file_names()
        .position(|candidate| normalize(candidate) == want)?;
    let mut file = archive.by_index(index).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

fn vhv(checksum: u32, meshes: &[map::PropVerts], colors: &[[f32; 3]]) -> Vec<u8> {
    let header = 40 + meshes.len() * 28;
    let mut file = vec![0u8; align_up(header, 512)];
    write_i32(&mut file, 0, 2);
    write_u32(&mut file, 4, checksum);
    write_u32(&mut file, 8, 4);
    write_u32(&mut file, 12, 4);
    write_u32(&mut file, 16, colors.len() as u32);
    write_u32(&mut file, 20, meshes.len() as u32);
    let mut cursor = file.len();
    let mut color_at = 0usize;
    for (index, mesh) in meshes.iter().enumerate() {
        let at = 40 + index * 28;
        write_u32(&mut file, at, mesh.lod);
        write_u32(&mut file, at + 4, mesh.count);
        write_u32(&mut file, at + 8, cursor as u32);
        for _ in 0..mesh.count {
            let color = colors.get(color_at).copied().unwrap_or([0.0; 3]);
            file.extend(vertex_color(color));
            color_at += 1;
            cursor += 4;
        }
    }
    pad(&mut file, 512);
    file
}

fn ppl(
    checksum: u32,
    format: u32,
    width: u32,
    height: u32,
    lods: &[u32],
    colors: &[[f32; 3]],
) -> Vec<u8> {
    let format = if format == RGBA16F { RGBA16F } else { RGB888 };
    let header = 32 + lods.len() * 32;
    let mut file = vec![0u8; align_up(header, 512)];
    write_i32(&mut file, 0, 0);
    write_u32(&mut file, 4, checksum);
    write_u32(&mut file, 8, format);
    write_u32(&mut file, 12, lods.len() as u32);
    let mesh_texels = (width as usize).saturating_mul(height as usize);
    for (index, lod) in lods.iter().enumerate() {
        let base = colors
            .get(index * mesh_texels..(index + 1) * mesh_texels)
            .unwrap_or(&[]);
        let image = encode_chain(base, width, height, format);
        let at = 32 + index * 32;
        write_u32(&mut file, at, *lod);
        let offset = file.len() as u32;
        write_u32(&mut file, at + 4, offset);
        write_u32(&mut file, at + 8, image.len() as u32);
        write_u32(&mut file, at + 12, width);
        write_u32(&mut file, at + 16, height);
        file.extend_from_slice(&image);
    }
    pad(&mut file, 512);
    file
}

fn encode_chain(base: &[[f32; 3]], width: u32, height: u32, format: u32) -> Vec<u8> {
    let mut level = if base.len() == (width as usize) * (height as usize) {
        super::soften(base, width as usize, height as usize)
    } else {
        vec![[0.0; 3]; (width as usize) * (height as usize)]
    };
    let mut bytes = Vec::new();
    let mut w = width;
    let mut h = height;
    loop {
        bytes.extend(encode_level(&level, format));
        if w <= 1 && h <= 1 {
            break;
        }
        let next_w = (w / 2).max(1);
        let next_h = (h / 2).max(1);
        level = downsample(&level, w, h, next_w, next_h);
        w = next_w;
        h = next_h;
    }
    bytes
}

fn encode_level(colors: &[[f32; 3]], format: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(colors.len() * if format == RGBA16F { 8 } else { 3 });
    for color in colors {
        let color = super::screen(*color);
        if format == RGBA16F {
            bytes.extend(f16(color[0]));
            bytes.extend(f16(color[1]));
            bytes.extend(f16(color[2]));
            bytes.extend(f16(1.0));
        } else {
            bytes.push(super::shaded_byte(color[0]));
            bytes.push(super::shaded_byte(color[1]));
            bytes.push(super::shaded_byte(color[2]));
        }
    }
    bytes
}

fn downsample(src: &[[f32; 3]], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity((dw as usize) * (dh as usize));
    for y in 0..dh {
        for x in 0..dw {
            let mut sum = [0.0f32; 3];
            let mut count = 0.0f32;
            for oy in 0..2u32 {
                for ox in 0..2u32 {
                    let sx = (x * 2 + ox).min(sw.saturating_sub(1)) as usize;
                    let sy = (y * 2 + oy).min(sh.saturating_sub(1)) as usize;
                    let color = src.get(sy * sw as usize + sx).copied().unwrap_or([0.0; 3]);
                    sum[0] += finite(color[0]);
                    sum[1] += finite(color[1]);
                    sum[2] += finite(color[2]);
                    count += 1.0;
                }
            }
            out.push(sum.map(|channel| channel / count));
        }
    }
    out
}

fn vertex_color(color: [f32; 3]) -> [u8; 4] {
    let color = super::screen(color);
    [
        super::shaded_byte(color[2]),
        super::shaded_byte(color[1]),
        super::shaded_byte(color[0]),
        255,
    ]
}

fn f16(value: f32) -> [u8; 2] {
    let value = finite(value).max(0.0);
    if value == 0.0 {
        return [0, 0];
    }
    if value > 65504.0 {
        return 0x7c00u16.to_le_bytes();
    }
    let bits = value.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127;
    if exp < -14 {
        return [0, 0];
    }
    let frac = ((bits & 0x7f_ffff) >> 13) as u16;
    let bits = ((exp + 15) as u16) << 10 | frac;
    bits.to_le_bytes()
}

fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn align_up(value: usize, align: usize) -> usize {
    value.div_ceil(align) * align
}

fn pad(bytes: &mut Vec<u8>, align: usize) {
    let extra = (align - bytes.len() % align) % align;
    bytes.extend(std::iter::repeat(0).take(extra));
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_i32(bytes: &mut [u8], offset: usize, value: i32) {
    write_u32(bytes, offset, value as u32);
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use map::{PropBody, PropLight, PropVerts};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    use super::repack;

    fn vhv(checksum: u32, count: u32) -> Vec<u8> {
        let mut file = vec![0u8; 512];
        file[0..4].copy_from_slice(&2i32.to_le_bytes());
        file[4..8].copy_from_slice(&checksum.to_le_bytes());
        file[8..12].copy_from_slice(&4u32.to_le_bytes());
        file[12..16].copy_from_slice(&4u32.to_le_bytes());
        file[16..20].copy_from_slice(&count.to_le_bytes());
        file[20..24].copy_from_slice(&1u32.to_le_bytes());
        file[44..48].copy_from_slice(&count.to_le_bytes());
        file[48..52].copy_from_slice(&512u32.to_le_bytes());
        file.extend(std::iter::repeat(0).take(count as usize * 4));
        file
    }

    fn pak(comment: &[u8], files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(&mut cursor);
        zip.set_raw_comment(comment.to_vec().into_boxed_slice());
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, bytes) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        cursor.into_inner()
    }

    fn prop(checksum: u32, count: u32) -> PropLight {
        PropLight {
            ldr: "sp_0.vhv".into(),
            hdr: "sp_hdr_0.vhv".into(),
            checksum,
            first: 0,
            body: PropBody::Vertices {
                meshes: vec![PropVerts { lod: 0, count }],
            },
        }
    }

    fn names(pak: &[u8]) -> Vec<String> {
        let mut archive = ZipArchive::new(Cursor::new(pak)).unwrap();
        let mut out = Vec::new();
        for index in 0..archive.len() {
            out.push(archive.by_index(index).unwrap().name().to_string());
        }
        out
    }

    #[test]
    fn a_stale_prop_file_is_replaced_and_gains_no_hdr_twin() {
        let comment = b"XZP1 0\0\0\0\0".to_vec();
        let original = vhv(9, 4);
        let source = pak(&comment, &[("sp_0.vhv", &original)]);
        let written = repack(&source, &[prop(1, 1)], &[[1.0, 0.0, 0.0]]).unwrap();
        assert_eq!(names(&written), vec!["sp_0.vhv".to_string()]);
        let mut archive = ZipArchive::new(Cursor::new(written.as_slice())).unwrap();
        assert_eq!(archive.comment(), comment.as_slice());
        let mut kept = Vec::new();
        archive
            .by_name("sp_0.vhv")
            .unwrap()
            .read_to_end(&mut kept)
            .unwrap();
        assert_ne!(kept, original);
        assert_eq!(&kept[4..8], &1u32.to_le_bytes());
    }

    #[test]
    fn an_existing_hdr_file_is_replaced_and_a_missing_one_is_not_created() {
        let source = pak(
            b"XZP1 0",
            &[("sp_0.vhv", &vhv(1, 1)), ("sp_hdr_0.vhv", &vhv(1, 1))],
        );
        let written = repack(&source, &[prop(1, 1)], &[[1.0, 0.0, 0.0]]).unwrap();
        let got = names(&written);
        assert!(got.iter().any(|name| name == "sp_0.vhv"));
        assert!(got.iter().any(|name| name == "sp_hdr_0.vhv"));

        let ldr_only = pak(b"XZP1 0", &[("sp_0.vhv", &vhv(1, 1))]);
        let written = repack(&ldr_only, &[prop(1, 1)], &[[1.0, 0.0, 0.0]]).unwrap();
        assert_eq!(names(&written), vec!["sp_0.vhv".to_string()]);
    }
}
