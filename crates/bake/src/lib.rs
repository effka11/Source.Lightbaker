//! Full-budget solve, then write. The window's ray count is not an argument.

use std::path::Path;

use map::Snapshot;
use solve::{solve, Area, Receiver, Triangle};

const FULL_RAYS: u32 = 256;

pub fn bake(
    triangles: &[Triangle],
    receivers: &[Receiver],
    areas: &[Area],
    snapshot: &Snapshot,
    source: &Path,
    destination: &Path,
) -> Result<(), write::Error> {
    let solved = solve(triangles, receivers, areas, FULL_RAYS);
    write::write(snapshot, &solved.light, source, destination)
}

#[cfg(test)]
mod tests;
