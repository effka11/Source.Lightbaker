mod embree;
mod geom;
mod light;
mod room;
mod shell;
mod walls;

pub use geom::{Area, Disk, Receiver, Rectangle, Role, Triangle};
pub use light::{dynamic, faces_solid, solve, solve_reporting, Solved, RAY_PASSES};
pub use room::{room, Luxel, Room};
