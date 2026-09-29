mod embree;
mod geom;
mod light;
mod room;

pub use geom::{Receiver, Rectangle, Triangle};
pub use light::solve;
pub use room::{room, Luxel, Room};
