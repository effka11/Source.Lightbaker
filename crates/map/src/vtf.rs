//! Source VTF image. Mips are decoded to straight RGBA, largest first.
//! The bytes are the file's sRGB texels; the window uploads them as sRGB.

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
}

pub fn decode(data: &[u8]) -> Option<Image> {
    if data.len() < 64 || &data[..4] != b"VTF\0" {
        return None;
    }
    let major = u32_at(data, 4)?;
    let minor = u32_at(data, 8)?;
    if major != 7 {
        return None;
    }
    let header = u32_at(data, 12)? as usize;
    let width = u16_at(data, 16)? as u32;
    let height = u16_at(data, 18)? as u32;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return None;
    }
    let frames = (u16_at(data, 24)? as usize).max(1);
    let format = i32_at(data, 52)?;
    let mips = (*data.get(56)? as usize).max(1);
    let depth = if minor >= 2 && data.len() >= 65 {
        (u16_at(data, 63)? as usize).max(1)
    } else {
        1
    };
    let mut cursor = if minor >= 3 {
        high_res_offset(data).unwrap_or(header)
    } else {
        header
    };
    if minor < 3 {
        let low_format = i32_at(data, 57).unwrap_or(-1);
        let low_w = *data.get(61)? as u32;
        let low_h = *data.get(62)? as u32;
        if low_w > 0 && low_h > 0 && low_format >= 0 {
            cursor = cursor.checked_add(byte_size(low_format, low_w, low_h)?)?;
        }
    }
    // The file stores the smallest mip first.
    let mut sizes = Vec::with_capacity(mips);
    let mut mip_w = width;
    let mut mip_h = height;
    for _ in 0..mips {
        sizes.push((mip_w, mip_h));
        mip_w = (mip_w / 2).max(1);
        mip_h = (mip_h / 2).max(1);
    }
    sizes.reverse();
    let mut levels = Vec::with_capacity(mips);
    for (mip_w, mip_h) in sizes {
        let size = byte_size(format, mip_w, mip_h)?;
        let group = size.checked_mul(frames)?.checked_mul(depth)?;
        let raw = data.get(cursor..cursor.checked_add(size)?)?;
        levels.push(decode_level(format, mip_w, mip_h, raw)?);
        cursor = cursor.checked_add(group)?;
    }
    levels.reverse();
    // The file's own small mips are not a downsample of the base. On a floor,
    // which is seen edge-on, the hardware picks those and the surface becomes
    // one flat color — the same color as the material average.
    let base = levels.into_iter().next()?;
    Some(Image {
        width,
        height,
        mips: downsample_chain(base, width, height),
    })
}

fn downsample_chain(base: Vec<u8>, mut width: u32, mut height: u32) -> Vec<Vec<u8>> {
    let mut levels = vec![base];
    while width > 1 || height > 1 {
        let next_w = (width / 2).max(1);
        let next_h = (height / 2).max(1);
        let src = levels.last().unwrap();
        let mut dst = vec![0u8; (next_w as usize) * (next_h as usize) * 4];
        for y in 0..next_h {
            for x in 0..next_w {
                let mut acc = [0u32; 4];
                for oy in 0..2 {
                    for ox in 0..2 {
                        let sx = (x * 2 + ox).min(width - 1);
                        let sy = (y * 2 + oy).min(height - 1);
                        let at = ((sy * width + sx) as usize) * 4;
                        for channel in 0..4 {
                            acc[channel] += src[at + channel] as u32;
                        }
                    }
                }
                let at = ((y * next_w + x) as usize) * 4;
                for channel in 0..4 {
                    dst[at + channel] = (acc[channel] / 4) as u8;
                }
            }
        }
        levels.push(dst);
        width = next_w;
        height = next_h;
    }
    levels
}

fn high_res_offset(data: &[u8]) -> Option<usize> {
    if data.len() < 72 {
        return None;
    }
    let count = u32_at(data, 68)? as usize;
    let mut pos = 72usize;
    for _ in 0..count {
        let tag = *data.get(pos)?;
        let flags = *data.get(pos + 3)?;
        let offset = u32_at(data, pos + 4)? as usize;
        if tag == 0x30 && flags & 0x02 == 0 {
            return Some(offset);
        }
        pos = pos.checked_add(8)?;
    }
    None
}

fn byte_size(format: i32, width: u32, height: u32) -> Option<usize> {
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;
    let pixels = width.checked_mul(height)?;
    let blocks = |bytes: usize| {
        let bw = width.div_ceil(4);
        let bh = height.div_ceil(4);
        bw.checked_mul(bh)?.checked_mul(bytes)
    };
    match format {
        0 | 1 | 11 | 12 | 16 | 22 => pixels.checked_mul(4),
        2 | 3 | 9 | 10 => pixels.checked_mul(3),
        5 | 8 => Some(pixels),
        6 => pixels.checked_mul(2),
        13 | 20 => blocks(8),
        14 | 15 => blocks(16),
        _ => None,
    }
}

fn decode_level(format: i32, width: u32, height: u32, raw: &[u8]) -> Option<Vec<u8>> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    match format {
        13 | 20 => decode_dxt1(raw, width, height),
        14 => decode_dxt3(raw, width, height),
        15 => decode_dxt5(raw, width, height),
        0 => take_rgba(raw, pixels, |p| [p[0], p[1], p[2], p[3]]),
        1 => take_rgba(raw, pixels, |p| [p[3], p[2], p[1], p[0]]),
        2 | 9 => take_rgb(raw, pixels, |p| [p[0], p[1], p[2], 255]),
        3 | 10 => take_rgb(raw, pixels, |p| [p[2], p[1], p[0], 255]),
        5 | 8 => {
            if raw.len() < pixels {
                return None;
            }
            let mut out = Vec::with_capacity(pixels * 4);
            for byte in &raw[..pixels] {
                let alpha = if format == 8 { *byte } else { 255 };
                let gray = if format == 8 { 255 } else { *byte };
                out.extend_from_slice(&[gray, gray, gray, alpha]);
            }
            Some(out)
        }
        6 => {
            if raw.len() < pixels * 2 {
                return None;
            }
            let mut out = Vec::with_capacity(pixels * 4);
            for pixel in raw[..pixels * 2].chunks_exact(2) {
                out.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
            Some(out)
        }
        11 => take_rgba(raw, pixels, |p| [p[1], p[2], p[3], p[0]]),
        12 => take_rgba(raw, pixels, |p| [p[2], p[1], p[0], p[3]]),
        16 => take_rgba(raw, pixels, |p| [p[2], p[1], p[0], 255]),
        22 => take_rgba(raw, pixels, |p| [p[0], p[1], p[2], p[3]]),
        _ => None,
    }
}

fn take_rgba(raw: &[u8], pixels: usize, map: fn(&[u8]) -> [u8; 4]) -> Option<Vec<u8>> {
    if raw.len() < pixels * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(pixels * 4);
    for pixel in raw[..pixels * 4].chunks_exact(4) {
        out.extend_from_slice(&map(pixel));
    }
    Some(out)
}

fn take_rgb(raw: &[u8], pixels: usize, map: fn(&[u8]) -> [u8; 4]) -> Option<Vec<u8>> {
    if raw.len() < pixels * 3 {
        return None;
    }
    let mut out = Vec::with_capacity(pixels * 4);
    for pixel in raw[..pixels * 3].chunks_exact(3) {
        out.extend_from_slice(&map(pixel));
    }
    Some(out)
}

fn decode_dxt1(raw: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    decode_blocks(raw, width, height, 8, |block, x, y, out| {
        let colors = dxt1_colors(block)?;
        let bits = u32::from_le_bytes(block[4..8].try_into().ok()?);
        put_block(out, width, height, x, y, |px, py| {
            let index = ((bits >> (2 * (py * 4 + px))) & 3) as usize;
            colors[index]
        });
        Some(())
    })
}

fn decode_dxt3(raw: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    decode_blocks(raw, width, height, 16, |block, x, y, out| {
        let colors = dxt1_colors(&block[8..16])?;
        let bits = u32::from_le_bytes(block[12..16].try_into().ok()?);
        put_block(out, width, height, x, y, |px, py| {
            let index = ((bits >> (2 * (py * 4 + px))) & 3) as usize;
            let mut color = colors[index];
            let alpha_bit = (py * 4 + px) as usize;
            let alpha = (block[alpha_bit / 2] >> ((alpha_bit % 2) * 4)) & 0x0f;
            color[3] = alpha * 17;
            color
        });
        Some(())
    })
}

fn decode_dxt5(raw: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    decode_blocks(raw, width, height, 16, |block, x, y, out| {
        let alphas = dxt5_alphas(block)?;
        let colors = dxt1_colors(&block[8..16])?;
        let bits = u32::from_le_bytes(block[12..16].try_into().ok()?);
        put_block(out, width, height, x, y, |px, py| {
            let slot = py * 4 + px;
            let index = ((bits >> (2 * slot)) & 3) as usize;
            let mut color = colors[index];
            color[3] = alphas[slot as usize];
            color
        });
        Some(())
    })
}

fn decode_blocks(
    raw: &[u8],
    width: u32,
    height: u32,
    block_bytes: usize,
    mut paint: impl FnMut(&[u8], u32, u32, &mut [u8]) -> Option<()>,
) -> Option<Vec<u8>> {
    let bw = width.div_ceil(4);
    let bh = height.div_ceil(4);
    let need = (bw as usize)
        .checked_mul(bh as usize)?
        .checked_mul(block_bytes)?;
    if raw.len() < need {
        return None;
    }
    let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let at = ((by * bw + bx) as usize) * block_bytes;
            paint(&raw[at..at + block_bytes], bx * 4, by * 4, &mut out)?;
        }
    }
    Some(out)
}

fn put_block(
    out: &mut [u8],
    width: u32,
    height: u32,
    origin_x: u32,
    origin_y: u32,
    pixel: impl Fn(u32, u32) -> [u8; 4],
) {
    for py in 0..4 {
        for px in 0..4 {
            let x = origin_x + px;
            let y = origin_y + py;
            if x >= width || y >= height {
                continue;
            }
            let at = ((y * width + x) as usize) * 4;
            out[at..at + 4].copy_from_slice(&pixel(px, py));
        }
    }
}

fn dxt1_colors(block: &[u8]) -> Option<[[u8; 4]; 4]> {
    let c0 = u16::from_le_bytes(block.get(..2)?.try_into().ok()?);
    let c1 = u16::from_le_bytes(block.get(2..4)?.try_into().ok()?);
    let a = rgb565(c0);
    let b = rgb565(c1);
    let mut colors = [a, b, [0; 4], [0; 4]];
    if c0 > c1 {
        colors[2] = mix(a, b, 2, 1);
        colors[3] = mix(a, b, 1, 2);
    } else {
        colors[2] = mix(a, b, 1, 1);
        colors[3] = [0, 0, 0, 0];
    }
    Some(colors)
}

fn dxt5_alphas(block: &[u8]) -> Option<[u8; 16]> {
    let a0 = *block.first()?;
    let a1 = *block.get(1)?;
    let mut bits = 0u64;
    for (index, byte) in block.get(2..8)?.iter().enumerate() {
        bits |= (*byte as u64) << (8 * index);
    }
    let mut table = [0u8; 8];
    table[0] = a0;
    table[1] = a1;
    if a0 > a1 {
        for step in 1..7 {
            table[step + 1] = (((7 - step) as u16 * a0 as u16 + step as u16 * a1 as u16) / 7) as u8;
        }
    } else {
        for step in 1..5 {
            table[step + 1] = (((5 - step) as u16 * a0 as u16 + step as u16 * a1 as u16) / 5) as u8;
        }
        table[6] = 0;
        table[7] = 255;
    }
    let mut out = [0u8; 16];
    for pixel in 0..16 {
        out[pixel] = table[((bits >> (3 * pixel)) & 7) as usize];
    }
    Some(out)
}

fn rgb565(packed: u16) -> [u8; 4] {
    let expand5 = |value: u16| {
        let value = (value & 31) as u8;
        (value << 3) | (value >> 2)
    };
    let expand6 = |value: u16| {
        let value = (value & 63) as u8;
        (value << 2) | (value >> 4)
    };
    [
        expand5(packed >> 11),
        expand6(packed >> 5),
        expand5(packed),
        255,
    ]
}

fn mix(a: [u8; 4], b: [u8; 4], wa: u16, wb: u16) -> [u8; 4] {
    let div = wa + wb;
    let channel = |left: u8, right: u8| ((wa * left as u16 + wb * right as u16) / div) as u8;
    [
        channel(a[0], b[0]),
        channel(a[1], b[1]),
        channel(a[2], b[2]),
        255,
    ]
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn i32_at(data: &[u8], offset: usize) -> Option<i32> {
    Some(u32_at(data, offset)? as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxt1_block_of_white_decodes_to_white() {
        let mut data = vec![0u8; 80];
        data[0..4].copy_from_slice(b"VTF\0");
        data[4..8].copy_from_slice(&7u32.to_le_bytes());
        data[8..12].copy_from_slice(&2u32.to_le_bytes());
        data[12..16].copy_from_slice(&80u32.to_le_bytes());
        data[16..18].copy_from_slice(&4u16.to_le_bytes());
        data[18..20].copy_from_slice(&4u16.to_le_bytes());
        data[24..26].copy_from_slice(&1u16.to_le_bytes());
        data[52..56].copy_from_slice(&13i32.to_le_bytes());
        data[56] = 1;
        data[57..61].copy_from_slice(&(-1i32).to_le_bytes());
        data[63..65].copy_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&[0xff, 0xff, 0x00, 0x00, 0, 0, 0, 0]);
        let image = decode(&data).expect("vtf");
        assert_eq!(image.width, 4);
        assert_eq!(image.mips[0].len(), 64);
        assert!(image.mips[0]
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 255, 255, 255]));
    }

    #[test]
    fn bgr_texel_keeps_its_channels() {
        let mut data = vec![0u8; 80];
        data[0..4].copy_from_slice(b"VTF\0");
        data[4..8].copy_from_slice(&7u32.to_le_bytes());
        data[8..12].copy_from_slice(&2u32.to_le_bytes());
        data[12..16].copy_from_slice(&80u32.to_le_bytes());
        data[16..18].copy_from_slice(&1u16.to_le_bytes());
        data[18..20].copy_from_slice(&1u16.to_le_bytes());
        data[24..26].copy_from_slice(&1u16.to_le_bytes());
        data[52..56].copy_from_slice(&3i32.to_le_bytes());
        data[56] = 1;
        data[57..61].copy_from_slice(&(-1i32).to_le_bytes());
        data[63..65].copy_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&[0, 0, 255]);
        let image = decode(&data).expect("vtf");
        assert_eq!(&image.mips[0][..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn small_mips_come_from_the_base_not_the_file() {
        let mut data = vec![0u8; 80];
        data[0..4].copy_from_slice(b"VTF\0");
        data[4..8].copy_from_slice(&7u32.to_le_bytes());
        data[8..12].copy_from_slice(&2u32.to_le_bytes());
        data[12..16].copy_from_slice(&80u32.to_le_bytes());
        data[16..18].copy_from_slice(&8u16.to_le_bytes());
        data[18..20].copy_from_slice(&8u16.to_le_bytes());
        data[24..26].copy_from_slice(&1u16.to_le_bytes());
        data[52..56].copy_from_slice(&0i32.to_le_bytes());
        data[56] = 2;
        data[57..61].copy_from_slice(&(-1i32).to_le_bytes());
        data[63..65].copy_from_slice(&1u16.to_le_bytes());
        // Smallest mip first: a black 4x4 that must not survive.
        data.extend(std::iter::repeat(0u8).take(4 * 4 * 4));
        for _ in 0..8 * 8 {
            data.extend_from_slice(&[20, 40, 60, 255]);
        }
        let image = decode(&data).expect("vtf");
        assert_eq!(image.mips[0].len(), 8 * 8 * 4);
        assert!(image.mips[1]
            .chunks_exact(4)
            .all(|pixel| pixel == [20, 40, 60, 255]));
        assert!(image
            .mips
            .last()
            .unwrap()
            .chunks_exact(4)
            .all(|pixel| pixel == [20, 40, 60, 255]));
    }
}
