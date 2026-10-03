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
    pub color: Vec3,
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

/// One-sided disk. `axis` lies in the disk and fixes the sample layout.
#[derive(Clone, Copy, Debug)]
pub struct Disk {
    pub center: Vec3,
    pub radius: f32,
    pub normal: Vec3,
    pub axis: Vec3,
    pub intensity: f32,
    pub color: Vec3,
}

impl Disk {
    /// Orthonormal axes in the disk plane: `(axis, bitangent)`.
    pub fn frame(self) -> (Vec3, Vec3) {
        let normal = self.normal.normalize_or_zero();
        let mut axis = self.axis - normal * self.axis.dot(normal);
        if axis.length_squared() <= 1.0e-8 {
            let helper = if normal.x.abs() > 0.9 {
                Vec3::Y
            } else {
                Vec3::X
            };
            axis = helper.cross(normal);
        }
        let axis = axis.normalize_or_zero();
        (axis, normal.cross(axis))
    }
}

/// Oriented box. Points inside are the lamp; the shell does not leave a cavity.
#[derive(Clone, Copy, Debug)]
pub struct Volume {
    pub center: Vec3,
    /// Half-extent along the box X axis, in world space.
    pub axis_x: Vec3,
    /// Half-extent along the box Y axis, in world space.
    pub axis_y: Vec3,
    /// Half-extent along the box Z axis, in world space.
    pub axis_z: Vec3,
    pub intensity: f32,
    pub color: Vec3,
}

/// Light at the center of a solid figure. No face, so the hull does not cut the room.
#[derive(Clone, Copy, Debug)]
pub struct Omni {
    pub center: Vec3,
    /// Half-extent along the box X axis. Geometry inside the box is the figure.
    pub axis_x: Vec3,
    pub axis_y: Vec3,
    pub axis_z: Vec3,
    pub intensity: f32,
    pub color: Vec3,
}

/// Emitting patch handed to the solver.
#[derive(Clone, Copy, Debug)]
pub enum Area {
    Rectangle(Rectangle),
    Disk(Disk),
    Volume(Volume),
    Omni(Omni),
}

impl Area {
    pub fn center(self) -> Vec3 {
        match self {
            Area::Rectangle(rectangle) => rectangle.center,
            Area::Disk(disk) => disk.center,
            Area::Volume(volume) => volume.center,
            Area::Omni(omni) => omni.center,
        }
    }

    pub fn normal(self) -> Vec3 {
        match self {
            Area::Rectangle(rectangle) => rectangle.normal,
            Area::Disk(disk) => disk.normal,
            Area::Volume(_) | Area::Omni(_) => Vec3::ZERO,
        }
    }

    pub fn intensity(self) -> f32 {
        match self {
            Area::Rectangle(rectangle) => rectangle.intensity,
            Area::Disk(disk) => disk.intensity,
            Area::Volume(volume) => volume.intensity,
            Area::Omni(omni) => omni.intensity,
        }
    }

    pub fn color(self) -> Vec3 {
        match self {
            Area::Rectangle(rectangle) => rectangle.color,
            Area::Disk(disk) => disk.color,
            Area::Volume(volume) => volume.color,
            Area::Omni(omni) => omni.color,
        }
    }
}
