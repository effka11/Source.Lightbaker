//! Full-budget solve, then write. The window's ray count is not an argument.

use std::path::Path;

use map::Snapshot;
use solve::{solve_reporting, Area, Receiver, Triangle};

const FULL_RAYS: u32 = 256;

pub fn bake(
    triangles: &[Triangle],
    receivers: &[Receiver],
    areas: &[Area],
    snapshot: &Snapshot,
    source: &Path,
    destination: &Path,
) -> Result<(), write::Error> {
    bake_reporting(
        triangles,
        receivers,
        areas,
        snapshot,
        source,
        destination,
        &|_| {},
    )
}

/// Same bake. `report` is how many receivers finished a light pass.
/// Writing the file does not advance it.
pub fn bake_reporting(
    triangles: &[Triangle],
    receivers: &[Receiver],
    areas: &[Area],
    snapshot: &Snapshot,
    source: &Path,
    destination: &Path,
    report: &(dyn Fn(u64) + Sync),
) -> Result<(), write::Error> {
    let solved = solve_reporting(triangles, receivers, areas, FULL_RAYS, report);
    let lights = placed_lights(areas);
    write::write(snapshot, &solved.light, &lights, source, destination)
}

/// One worldlight per lamp. A solid figure is a volume plus an omni at the
/// same point; the brighter of the two is the entity.
fn placed_lights(areas: &[Area]) -> Vec<write::PlacedLight> {
    let mut lights: Vec<write::PlacedLight> = Vec::new();
    for area in areas {
        let origin = area.center();
        let intensity = area.intensity();
        if intensity <= 0.0 || !origin.is_finite() {
            continue;
        }
        let color = area.color();
        if let Some(found) = lights.iter_mut().find(|light| {
            let delta = [
                light.origin[0] - origin.x,
                light.origin[1] - origin.y,
                light.origin[2] - origin.z,
            ];
            delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2] < 1.0
        }) {
            if intensity > found.intensity {
                found.color = [color.x, color.y, color.z];
                found.intensity = intensity;
            }
            continue;
        }
        lights.push(write::PlacedLight {
            origin: [origin.x, origin.y, origin.z],
            color: [color.x, color.y, color.z],
            intensity,
        });
    }
    lights
}

#[cfg(test)]
mod tests;
