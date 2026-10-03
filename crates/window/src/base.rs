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
            color: data::COLOR,
            inside: data::INSIDE,
            rim: data::RIM,
            spread: data::SPREAD,
            mesh: data::MESH,
        }
    }
}

static LIVE: RwLock<Option<Params>> = RwLock::new(None);

/// The base used by this process. A save updates it before the next build.
pub fn live() -> Params {
    let mut slot = LIVE.write().expect("base");
    if slot.is_none() {
        *slot = Some(Params::compiled());
    }
    slot.expect("base")
}

/// Write the base into the crate source and use it for the rest of this run.
pub fn store(params: Params) -> Result<(), String> {
    let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/base_data.rs"));
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

pub fn render(params: &Params) -> String {
    format!(
        r#"//! Defaults for a new light. The parameters panel rewrites this file.

pub const COMPLETE: bool = {complete};
pub const SOLID: bool = {solid};
pub const SIZE: [f32; 3] = {size};
pub const ROTATION: [f32; 3] = {rotation};
pub const CORNERS: f32 = {corners};
pub const INTENSITY: f32 = {intensity};
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
        assert!(text.contains("pub const COLOR: [f32; 3] = [1.0, 0.925, 0.824];"));
    }
}
