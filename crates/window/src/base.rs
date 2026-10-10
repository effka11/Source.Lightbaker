//! The light a new object starts from. Saving in the panel rewrites `base_data.rs`.

use std::sync::RwLock;

#[path = "base_data.rs"]
mod data;

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// When set, a new light takes the shape and the strength below, not the fixture's.
    pub complete: bool,
    pub solid: bool,
    pub size: [f32; 3],
    pub rotation: [f32; 3],
    pub corners: f32,
    pub intensity: f32,
    pub reach: f32,
    pub color: [f32; 3],
    pub inside: f32,
    pub rim: f32,
    pub spread: f32,
    pub mesh: f32,
}

impl Params {
    fn compiled() -> Self {
        Self {
            complete: data::COMPLETE,
            solid: data::SOLID,
            size: data::SIZE,
            rotation: data::ROTATION,
            corners: data::CORNERS,
            intensity: data::INTENSITY,
            reach: data::REACH,
            color: data::COLOR,
            inside: data::INSIDE,
            rim: data::RIM,
            spread: data::SPREAD,
            mesh: data::MESH,
        }
    }
}

static LIVE: RwLock<Option<Params>> = RwLock::new(None);

fn source_path() -> &'static std::path::Path {
    std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/base_data.rs"))
}

/// The base used by this process. The open `base_data.rs` wins over the copy
/// compiled into the binary, so an edit of that file applies on the next read.
pub fn live() -> Params {
    let path = source_path();
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Some(params) = parse(&text) {
            *LIVE.write().expect("base") = Some(params);
            return params;
        }
    }
    LIVE.read().expect("base").unwrap_or_else(Params::compiled)
}

/// Write the base into the crate source and use it for the rest of this run.
pub fn store(params: Params) -> Result<(), String> {
    let path = source_path();
    let text = render(&params);
    std::fs::write(path, text).map_err(|err| {
        format!(
            "Не удалось записать базовые параметры в {}: {err}",
            path.display()
        )
    })?;
    *LIVE.write().expect("base") = Some(params);
    Ok(())
}

/// Read the constants `render` writes. A broken file is `None`.
pub fn parse(text: &str) -> Option<Params> {
    Some(Params {
        complete: bool_const(text, "COMPLETE")?,
        solid: bool_const(text, "SOLID")?,
        size: vec3_const(text, "SIZE")?,
        rotation: vec3_const(text, "ROTATION")?,
        corners: f32_const(text, "CORNERS")?,
        intensity: f32_const(text, "INTENSITY")?,
        reach: f32_const(text, "REACH")?,
        color: vec3_const(text, "COLOR")?,
        inside: f32_const(text, "INSIDE")?,
        rim: f32_const(text, "RIM")?,
        spread: f32_const(text, "SPREAD")?,
        mesh: f32_const(text, "MESH")?,
    })
}

fn const_value<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    for line in text.lines() {
        let Some(line) = line.trim().strip_prefix("pub const ") else {
            continue;
        };
        let Some((got, rest)) = line.split_once(':') else {
            continue;
        };
        if got != name {
            continue;
        }
        let Some((_, value)) = rest.split_once('=') else {
            continue;
        };
        let value = value.trim();
        return Some(value.strip_suffix(';').unwrap_or(value).trim());
    }
    None
}

fn bool_const(text: &str, name: &str) -> Option<bool> {
    match const_value(text, name)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn f32_const(text: &str, name: &str) -> Option<f32> {
    let value: f32 = const_value(text, name)?.parse().ok()?;
    value.is_finite().then_some(value)
}

fn vec3_const(text: &str, name: &str) -> Option<[f32; 3]> {
    let raw = const_value(text, name)?;
    let raw = raw.strip_prefix('[')?.strip_suffix(']')?;
    let mut nums = raw.split(',');
    let component = |text: &str| {
        let value: f32 = text.trim().parse().ok()?;
        value.is_finite().then_some(value)
    };
    Some([
        component(nums.next()?)?,
        component(nums.next()?)?,
        component(nums.next()?)?,
    ])
}

pub fn render(params: &Params) -> String {
    format!(
        r#"//! Defaults for a new light. The parameters panel rewrites this file.

pub const COMPLETE: bool = {complete};
pub const SOLID: bool = {solid};
pub const SIZE: [f32; 3] = {size};
pub const ROTATION: [f32; 3] = {rotation};
pub const CORNERS: f32 = {corners};
pub const INTENSITY: f32 = {intensity};
pub const REACH: f32 = {reach};
pub const COLOR: [f32; 3] = {color};
pub const INSIDE: f32 = {inside};
pub const RIM: f32 = {rim};
pub const SPREAD: f32 = {spread};
pub const MESH: f32 = {mesh};
"#,
        complete = rust_bool(params.complete),
        solid = rust_bool(params.solid),
        size = rust_vec3(params.size),
        rotation = rust_vec3(params.rotation),
        corners = rust_f32(params.corners),
        intensity = rust_f32(params.intensity),
        reach = rust_f32(params.reach),
        color = rust_vec3(params.color),
        inside = rust_f32(params.inside),
        rim = rust_f32(params.rim),
        spread = rust_f32(params.spread),
        mesh = rust_f32(params.mesh),
    )
}

fn rust_bool(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn rust_vec3(value: [f32; 3]) -> String {
    format!(
        "[{}, {}, {}]",
        rust_f32(value[0]),
        rust_f32(value[1]),
        rust_f32(value[2])
    )
}

fn rust_f32(value: f32) -> String {
    if !value.is_finite() {
        return "0.0".to_owned();
    }
    let mut text = format!("{value:.5}");
    if let Some(dot) = text.find('.') {
        while text.ends_with('0') && text.len() > dot + 2 {
            text.pop();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_base_is_rust_source() {
        let text = render(&Params {
            complete: true,
            solid: true,
            size: [4.1, 7.8, 13.1],
            rotation: [0.0, 10.0, 0.0],
            corners: 25.0,
            intensity: 18000.0,
            reach: 160.0,
            color: [1.0, 0.925, 0.824],
            inside: 80.0,
            rim: 50.0,
            spread: 75.0,
            mesh: 22.0,
        });
        assert!(text.contains("pub const COMPLETE: bool = true;"));
        assert!(text.contains("pub const SOLID: bool = true;"));
        assert!(text.contains("pub const SIZE: [f32; 3] = [4.1, 7.8, 13.1];"));
        assert!(text.contains("pub const INSIDE: f32 = 80.0;"));
        assert!(text.contains("pub const REACH: f32 = 160.0;"));
        assert!(text.contains("pub const COLOR: [f32; 3] = [1.0, 0.925, 0.824];"));
        let parsed = parse(&text).expect("rendered base");
        assert!(parsed.complete);
        assert!((parsed.inside - 80.0).abs() < 1.0e-4);
        assert!((parsed.size[1] - 7.8).abs() < 1.0e-4);
        assert!((parsed.color[1] - 0.925).abs() < 1.0e-4);
    }

    #[test]
    fn the_open_file_is_the_base_the_app_uses() {
        let text = std::fs::read_to_string(source_path()).expect("base_data.rs");
        let parsed = parse(&text).expect("constants");
        let live = live();
        assert_eq!(live.complete, parsed.complete);
        assert_eq!(live.solid, parsed.solid);
        assert!((live.intensity - parsed.intensity).abs() < 1.0e-3);
        assert!((live.reach - parsed.reach).abs() < 1.0e-3);
        assert!((live.inside - parsed.inside).abs() < 1.0e-3);
        assert!((live.rim - parsed.rim).abs() < 1.0e-3);
        assert!((live.spread - parsed.spread).abs() < 1.0e-3);
        assert!((live.mesh - parsed.mesh).abs() < 1.0e-3);
        assert!((live.color[0] - parsed.color[0]).abs() < 1.0e-4);
        assert!((live.color[1] - parsed.color[1]).abs() < 1.0e-4);
        assert!((live.color[2] - parsed.color[2]).abs() < 1.0e-4);
    }
}
