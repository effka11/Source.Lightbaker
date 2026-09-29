use glam::Vec3;

#[derive(Clone, Copy, Debug)]
pub struct Triangle {
    pub vertices: [Vec3; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Wall,
    Floor,
    Other,
}

#[derive(Clone, Copy, Debug)]
pub struct Receiver {
    pub position: Vec3,
    pub normal: Vec3,
    pub albedo: Vec3,
    pub role: Role,
}

/// One-sided rectangle. `half_u` and `half_v` run from the center to the edges.
#[derive(Clone, Copy, Debug)]
pub struct Rectangle {
    pub center: Vec3,
    pub half_u: Vec3,
    pub half_v: Vec3,
    pub normal: Vec3,
    pub intensity: f32,
}

impl Rectangle {
    pub fn corners(self) -> [Vec3; 4] {
        let center = self.center;
        [
            center - self.half_u - self.half_v,
            center + self.half_u - self.half_v,
            center + self.half_u + self.half_v,
            center - self.half_u + self.half_v,
        ]
    }
}
