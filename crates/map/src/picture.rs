//! Base textures for the view. A face keeps its material color when the VTF
//! is missing. Displacement vertex alpha blends `$basetexture2` on top.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

use crate::vpk::{self};
use crate::{vtf, LoadPhase, NO_MATERIAL};

#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub texture: Option<u32>,
    pub texture2: Option<u32>,
    pub detail: Option<u32>,
    pub detail_scale: f32,
    pub detail_factor: f32,
    /// Lightmapped surfaces take the map's light. Unlit ones stay at the texture.
    pub lightmapped: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            texture: None,
            texture2: None,
            detail: None,
            detail_scale: 4.0,
            detail_factor: 0.0,
            lightmapped: true,
        }
    }
}

#[derive(Clone, Default)]
struct Keys {
    base: Option<String>,
    second: Option<String>,
    detail: Option<String>,
    detail_scale: Option<f32>,
    detail_factor: Option<f32>,
    /// UnlitGeneric painted by `$vertexcolor`. The texture is only a white sheet.
    paint_canvas: bool,
    lightmapped: bool,
}

struct Loader {
    pak: HashMap<String, Vec<u8>>,
    packs: Vec<vpk::Pack>,
    map_name: String,
    images: Vec<vtf::Image>,
    images_by_path: HashMap<String, Option<u32>>,
    materials_by_path: HashMap<String, Keys>,
}

pub(crate) struct Catalog {
    loader: Loader,
    materials: Vec<Material>,
    adopted: HashMap<String, u32>,
}

impl Catalog {
    pub(crate) fn world(
        bsp: &[u8],
        map_path: &Path,
        names: &[String],
        used: &[u32],
        report: &(dyn Fn(LoadPhase, u64, u64) + Sync),
    ) -> Self {
        let mut materials = vec![Material::default(); names.len()];
        let mut needed: Vec<u32> = used
            .iter()
            .copied()
            .filter(|index| *index != NO_MATERIAL && (*index as usize) < names.len())
            .collect();
        needed.sort_unstable();
        needed.dedup();
        let total = (needed.len() as u64).max(1);
        report(LoadPhase::Textures, 0, total);
        let map_name = map_path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("map")
            .to_ascii_lowercase();
        let mut loader = Loader {
            pak: pak_assets(pak_slice(bsp)),
            packs: vpk::search_with(map_path, &|_, _| {}),
            map_name,
            images: Vec::new(),
            images_by_path: HashMap::new(),
            materials_by_path: HashMap::new(),
        };
        for (step, index) in needed.iter().copied().enumerate() {
            let name = &names[index as usize];
            materials[index as usize] = material_from(&mut loader, name);
            report(LoadPhase::Textures, step as u64 + 1, total);
        }
        report(LoadPhase::Textures, total, total);
        Self {
            loader,
            materials,
            adopted: HashMap::new(),
        }
    }

    /// A model material, appended after the world list. `None` when its VTF is missing.
    pub(crate) fn adopt(&mut self, name: &str) -> Option<u32> {
        let key = name
            .replace('\\', "/")
            .trim()
            .trim_matches('"')
            .to_ascii_lowercase();
        if key.is_empty() {
            return None;
        }
        if let Some(index) = self.adopted.get(&key) {
            return (*index != u32::MAX).then_some(*index);
        }
        let material = material_from(&mut self.loader, &key);
        if material.texture.is_none() {
            self.adopted.insert(key, u32::MAX);
            return None;
        }
        let index = self.materials.len() as u32;
        self.materials.push(material);
        self.adopted.insert(key, index);
        Some(index)
    }

    pub(crate) fn texel_size(&self, material: u32) -> Option<[f32; 2]> {
        let texture = self.materials.get(material as usize)?.texture?;
        let image = self.loader.images.get(texture as usize)?;
        Some([image.width.max(1) as f32, image.height.max(1) as f32])
    }

    pub(crate) fn finish(self) -> (Vec<Material>, Vec<vtf::Image>) {
        (self.materials, self.loader.images)
    }
}

fn material_from(loader: &mut Loader, name: &str) -> Material {
    let keys = loader.keys_for(name);
    let texture = keys.base.as_deref().and_then(|path| loader.image(path));
    let texture2 = keys.second.as_deref().and_then(|path| loader.image(path));
    let detail = keys.detail.as_deref().and_then(|path| loader.image(path));
    Material {
        texture,
        texture2,
        detail,
        detail_scale: keys.detail_scale.unwrap_or(4.0),
        detail_factor: if detail.is_some() {
            keys.detail_factor.unwrap_or(1.0)
        } else {
            0.0
        },
        lightmapped: keys.lightmapped,
    }
}

/// Materials whose picture is a vertex color on a white texture. A brush face
/// has no vertex colors, so drawing one of these is a solid white sheet.
pub(crate) fn paint_canvases(bsp: &[u8], map_path: &Path, names: &[String]) -> Vec<bool> {
    let mut canvas = vec![false; names.len()];
    if names.is_empty() {
        return canvas;
    }
    let map_name = map_path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("map")
        .to_ascii_lowercase();
    let mut loader = Loader {
        pak: pak_assets(pak_slice(bsp)),
        packs: vpk::search_with(map_path, &|_, _| {}),
        map_name,
        images: Vec::new(),
        images_by_path: HashMap::new(),
        materials_by_path: HashMap::new(),
    };
    for (index, name) in names.iter().enumerate() {
        canvas[index] = loader.keys_for(name).paint_canvas;
    }
    canvas
}

impl Loader {
    fn keys_for(&mut self, material: &str) -> Keys {
        let mapped = format!(
            "materials/maps/{}/{}",
            self.map_name,
            vmt_key(material).trim_start_matches("materials/")
        );
        if self.bytes(&mapped).is_some() {
            self.keys_of(&mapped, 0)
        } else {
            self.keys_of(&vmt_key(material), 0)
        }
    }

    fn keys_of(&mut self, path: &str, depth: usize) -> Keys {
        if let Some(keys) = self.materials_by_path.get(path) {
            return keys.clone();
        }
        if depth > 6 {
            return Keys::default();
        }
        let Some(bytes) = self.bytes(path) else {
            self.materials_by_path
                .insert(path.to_string(), Keys::default());
            return Keys::default();
        };
        let text = strip_comments(&String::from_utf8_lossy(&bytes));
        let mut keys = if let Some(include) = token_value(&text, "include") {
            self.keys_of(&vmt_key(include), depth + 1)
        } else {
            Keys {
                lightmapped: true,
                ..Keys::default()
            }
        };
        if let Some(value) = token_value(&text, "$basetexture") {
            keys.base = Some(value.to_string());
        }
        if let Some(value) = token_value(&text, "$basetexture2") {
            keys.second = Some(value.to_string());
        }
        if let Some(value) = token_value(&text, "$detail") {
            keys.detail = Some(value.to_string());
        }
        if let Some(value) = token_value(&text, "$detailscale") {
            if let Ok(scale) = value.parse::<f32>() {
                keys.detail_scale = Some(scale);
            }
        }
        if let Some(value) = token_value(&text, "$detailblendfactor") {
            if let Ok(factor) = value.parse::<f32>() {
                keys.detail_factor = Some(factor);
            }
        }
        let unlit = token(text.trim_start())
            .is_some_and(|shader| shader.eq_ignore_ascii_case("UnlitGeneric"));
        let painted = token_value(&text, "$vertexcolor")
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if unlit && painted {
            keys.paint_canvas = true;
        }
        if let Some(shader) = token(text.trim_start()) {
            if shader.eq_ignore_ascii_case("UnlitGeneric")
                || shader.eq_ignore_ascii_case("UnlitTwoTexture")
            {
                keys.lightmapped = false;
            } else if !shader.eq_ignore_ascii_case("patch") {
                keys.lightmapped = true;
            }
        }
        self.materials_by_path
            .insert(path.to_string(), keys.clone());
        keys
    }

    fn image(&mut self, raw: &str) -> Option<u32> {
        let key = vtf_key(raw);
        if let Some(index) = self.images_by_path.get(&key) {
            return *index;
        }
        let index = self
            .bytes(&key)
            .as_deref()
            .and_then(vtf::decode)
            .map(|image| {
                let index = self.images.len() as u32;
                self.images.push(image);
                index
            });
        self.images_by_path.insert(key, index);
        index
    }

    fn bytes(&self, key: &str) -> Option<Vec<u8>> {
        let key = key.replace('\\', "/").to_ascii_lowercase();
        if let Some(bytes) = self.pak.get(&key) {
            return Some(bytes.clone());
        }
        vpk::read(&self.packs, &key)
    }
}

fn vmt_key(name: &str) -> String {
    let mut path = name
        .replace('\\', "/")
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase();
    if let Some(rest) = path.strip_prefix("materials/") {
        path = rest.to_string();
    }
    if let Some(rest) = path.strip_suffix(".vmt") {
        path = rest.to_string();
    }
    format!("materials/{path}.vmt")
}

fn vtf_key(name: &str) -> String {
    let mut path = name
        .replace('\\', "/")
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase();
    if !path.starts_with("materials/") {
        path = format!("materials/{path}");
    }
    if !path.ends_with(".vtf") {
        path.push_str(".vtf");
    }
    path
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let keep = if let Some(mark) = line.find("//") {
            let quotes = line[..mark].bytes().filter(|byte| *byte == b'"').count();
            if quotes % 2 == 0 {
                &line[..mark]
            } else {
                line
            }
        } else {
            line
        };
        out.push_str(keep);
        out.push('\n');
    }
    out
}

fn token_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    let key = key.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find(&key) {
        let abs = from + rel;
        let end = abs + key.len();
        let prev = abs == 0 || !lower.as_bytes()[abs - 1].is_ascii_alphanumeric();
        let next = lower.as_bytes().get(end).copied().unwrap_or(b' ');
        if prev && !next.is_ascii_alphanumeric() {
            let mut rest = &text[end..];
            if let Some(stripped) = rest.strip_prefix('"') {
                rest = stripped;
            }
            return token(rest);
        }
        from = abs + 1;
    }
    None
}

fn token(after: &str) -> Option<&str> {
    let bytes = after.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index >= bytes.len() {
        return None;
    }
    if bytes[index] == b'"' {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index] != b'"' {
            index += 1;
        }
        return Some(&after[start..index]);
    }
    let start = index;
    while index < bytes.len()
        && !bytes[index].is_ascii_whitespace()
        && bytes[index] != b'{'
        && bytes[index] != b'}'
    {
        index += 1;
    }
    (start < index).then_some(&after[start..index])
}

fn pak_slice(data: &[u8]) -> &[u8] {
    let at = 8 + 40 * 16;
    if data.len() < at + 8 {
        return &[];
    }
    let offset = i32::from_le_bytes(data[at..at + 4].try_into().unwrap_or([0; 4]));
    let length = i32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap_or([0; 4]));
    if offset < 0 || length < 0 {
        return &[];
    }
    data.get(offset as usize..(offset as usize).saturating_add(length as usize))
        .unwrap_or(&[])
}

fn pak_assets(pak: &[u8]) -> HashMap<String, Vec<u8>> {
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
        if !name.ends_with(".vmt") && !name.ends_with(".vtf") {
            continue;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_ok() {
            files.insert(name, bytes);
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_keeps_the_included_base_texture() {
        let mut pak = HashMap::new();
        pak.insert(
            "materials/brick/wall.vmt".into(),
            b"\"LightmappedGeneric\"\n{\n\t\"$basetexture\" \"brick/wall\"\n\t\"$detail\" detail\\noise\n\t\"$detailscale\" 5\n}".to_vec(),
        );
        pak.insert(
            "materials/maps/gm_construct/brick/wall.vmt".into(),
            b"\"patch\"\n{\n\t\"include\" \"materials/brick/wall.vmt\"\n\t\"replace\"\n\t{\n\t\t\"$envmap\" \"maps/gm_construct/c0_0_0\"\n\t}\n}".to_vec(),
        );
        let mut loader = Loader {
            pak,
            packs: Vec::new(),
            map_name: "gm_construct".into(),
            images: Vec::new(),
            images_by_path: HashMap::new(),
            materials_by_path: HashMap::new(),
        };
        let keys = loader.keys_for("BRICK/WALL");
        assert_eq!(keys.base.as_deref(), Some("brick/wall"));
        assert_eq!(keys.detail.as_deref(), Some("detail\\noise"));
        assert_eq!(keys.detail_scale, Some(5.0));
        assert!(!keys.paint_canvas);
        assert!(keys.lightmapped);
    }

    #[test]
    fn an_unlit_vertex_color_is_a_paint_canvas() {
        let mut pak = HashMap::new();
        pak.insert(
            "materials/gm_construct/color_room.vmt".into(),
            b"\"UnlitGeneric\"\n{\n\t\"$basetexture\" \"color/white\"\n\t\"$vertexcolor\" \"1\"\n}"
                .to_vec(),
        );
        let mut loader = Loader {
            pak,
            packs: Vec::new(),
            map_name: "gm_construct".into(),
            images: Vec::new(),
            images_by_path: HashMap::new(),
            materials_by_path: HashMap::new(),
        };
        let keys = loader.keys_for("GM_CONSTRUCT/COLOR_ROOM");
        assert!(keys.paint_canvas);
        assert!(!keys.lightmapped);
    }
}
