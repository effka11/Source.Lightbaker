mod embree;
mod geom;
mod light;
mod room;
mod shell;

pub use geom::{Area, Disk, Receiver, Rectangle, Role, Triangle};
pub use light::{solve, Solved};
pub use room::{room, Luxel, Room};
