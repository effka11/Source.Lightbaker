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
    write::write(snapshot, &solved.light, source, destination)
}

#[cfg(test)]
mod tests;
