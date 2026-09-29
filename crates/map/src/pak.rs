//! Albedo for bounce. A material color packed in the BSP zip wins.
//! When the pak only has a cubemap patch, the texdata reflectivity — the
//! color vbsp already took from that material — is what the face keeps.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use glam::Vec3;

pub fn albedos(pak: &[u8], names: &[String], reflectivity: &[Vec3]) -> Vec<Vec3> {
    let files = vmt_files(pak);
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let fallback = reflectivity.get(index).copied().unwrap_or(Vec3::splat(0.5));
            packed_color(&files, name)
                .unwrap_or(fallback)
                .clamp(Vec3::ZERO, Vec3::ONE)
        })
        .collect()
}

fn vmt_files(pak: &[u8]) -> HashMap<String, String> {
    let mut files = HashMap::new();
    if pak.is_empty() {
        return files;
    }
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(pak)) else {
        return files;
    };
    for index in 0..archive.len() {
        let Ok(mut file) = archive.by_index(index) else {
            continue;
        };
        let name = file.name().replace('\\', "/").to_ascii_lowercase();
        if !name.ends_with(".vmt") {
            continue;
        }
        let mut text = String::new();
        if file.read_to_string(&mut text).is_ok() {
            files.insert(name, text);
        }
    }
    files
}

fn packed_color(files: &HashMap<String, String>, material: &str) -> Option<Vec3> {
    let key = format!("materials/{}.vmt", material.replace('\\', "/")).to_ascii_lowercase();
    let text = files.get(&key)?;
    if let Some(color) = vmt_color(text) {
        return Some(color);
    }
    let include = vmt_value(text, "include")?;
    let include = include.replace('\\', "/").to_ascii_lowercase();
    let nested = files.get(&include).or_else(|| {
        let file = include.rsplit('/').next().unwrap_or(&include);
        files
            .iter()
            .find(|(path, _)| path.ends_with(file))
            .map(|(_, body)| body)
    })?;
    vmt_color(nested)
}

fn vmt_color(text: &str) -> Option<Vec3> {
    let raw = vmt_value(text, "$color").or_else(|| vmt_value(text, "$color2"))?;
    parse_color(raw)
}

fn vmt_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let pos = find_key(text, key)?;
    let after = &text[pos + key.len()..];
    let bytes = after.as_bytes();
    let mut index = 0;
    if index < bytes.len() && bytes[index] == b'"' {
        index += 1;
    }
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b'"' {
        return None;
    }
    index += 1;
    let start = index;
    while index < bytes.len() && bytes[index] != b'"' {
        index += 1;
    }
    Some(&after[start..index])
}

fn find_key(text: &str, key: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(&key) {
        let abs = from + rel;
        let end = abs + key.len();
        let next = lower.as_bytes().get(end).copied().unwrap_or(b' ');
        if !next.is_ascii_alphanumeric() {
            return Some(abs);
        }
        from = abs + key.len();
    }
    None
}

fn parse_color(text: &str) -> Option<Vec3> {
    let mut nums = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || ch == '.' || (ch == '-' && current.is_empty()) {
            current.push(ch);
        } else if !current.is_empty() {
            nums.push(current.parse::<f32>().ok()?);
            current.clear();
        }
    }
    if !current.is_empty() {
        nums.push(current.parse::<f32>().ok()?);
    }
    if nums.len() < 3 {
        return None;
    }
    let mut color = Vec3::new(nums[0], nums[1], nums[2]);
    if color.max_element() > 1.5 {
        color /= 255.0;
    }
    Some(color)
}
