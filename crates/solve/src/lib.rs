mod embree;
mod geom;
mod light;
mod room;
mod shell;
mod walls;

pub use geom::{Area, Disk, Omni, Receiver, Rectangle, Role, Triangle, Volume};
pub use light::{
    broad, broad_light, dynamic, faces_solid, solve, solve_reporting, Cover, Solved, LAMP_REACH,
    RAY_PASSES,
};
pub use room::{room, Luxel, Room};
