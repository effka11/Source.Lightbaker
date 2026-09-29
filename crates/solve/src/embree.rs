//! Thin Embree 4.4.1 binding. The shipped Windows build defines
//! `RTC_GEOMETRY_INSTANCE_ARRAY`, so `RTCHit` carries both `instID` and
//! `instPrimID` and is padded to 48 bytes.

use std::ffi::{c_char, CStr};
use std::sync::OnceLock;

use crate::geom::Triangle;
use glam::Vec3;

const FORMAT_UINT3: i32 = 0x5003;
const FORMAT_FLOAT3: i32 = 0x9003;
const BUFFER_INDEX: i32 = 0;
const BUFFER_VERTEX: i32 = 1;
const GEOMETRY_TRIANGLE: i32 = 0;
const INVALID_GEOMETRY: u32 = u32::MAX;

#[repr(C)]
struct DeviceOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct SceneOpaque {
    _private: [u8; 0],
}
#[repr(C)]
struct GeometryOpaque {
    _private: [u8; 0],
}

type Device = *mut DeviceOpaque;
type ScenePtr = *mut SceneOpaque;
type Geometry = *mut GeometryOpaque;

#[repr(C, align(16))]
struct RayHit {
    org_x: f32,
    org_y: f32,
    org_z: f32,
    tnear: f32,
    dir_x: f32,
    dir_y: f32,
    dir_z: f32,
    time: f32,
    tfar: f32,
    mask: u32,
    id: u32,
    flags: u32,
    ng_x: f32,
    ng_y: f32,
    ng_z: f32,
    u: f32,
    v: f32,
    prim_id: u32,
    geom_id: u32,
    inst_id: u32,
    inst_prim_id: u32,
    _pad: [u32; 3],
}

const _: () = assert!(std::mem::size_of::<RayHit>() == 96);
const _: () = assert!(std::mem::align_of::<RayHit>() == 16);

unsafe extern "C" {
    fn rtcNewDevice(config: *const c_char) -> Device;
    fn rtcGetDeviceError(device: Device) -> i32;
    fn rtcGetDeviceLastErrorMessage(device: Device) -> *const c_char;

    fn rtcNewScene(device: Device) -> ScenePtr;
    fn rtcReleaseScene(scene: ScenePtr);
    fn rtcCommitScene(scene: ScenePtr);
    fn rtcAttachGeometry(scene: ScenePtr, geometry: Geometry) -> u32;

    fn rtcNewGeometry(device: Device, geom_type: i32) -> Geometry;
    fn rtcReleaseGeometry(geometry: Geometry);
    fn rtcCommitGeometry(geometry: Geometry);
    fn rtcSetSharedGeometryBuffer(
        geometry: Geometry,
        buffer_type: i32,
        slot: u32,
        format: i32,
        ptr: *const (),
        byte_offset: usize,
        byte_stride: usize,
        item_count: usize,
    );

    fn rtcIntersect1(scene: ScenePtr, rayhit: *mut RayHit, args: *mut ());
}

struct SharedDevice(Device);

unsafe impl Send for SharedDevice {}
unsafe impl Sync for SharedDevice {}

fn device() -> Device {
    static DEVICE: OnceLock<SharedDevice> = OnceLock::new();
    let shared = DEVICE.get_or_init(|| {
        let device = unsafe { rtcNewDevice(std::ptr::null()) };
        if device.is_null() {
            panic!("Embree rtcNewDevice returned null");
        }
        SharedDevice(device)
    });
    shared.0
}

fn check(device: Device) {
    let code = unsafe { rtcGetDeviceError(device) };
    if code == 0 {
        return;
    }
    let message = unsafe { CStr::from_ptr(rtcGetDeviceLastErrorMessage(device)) };
    panic!("Embree error {code}: {}", message.to_string_lossy());
}

pub(crate) struct Hit {
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
    pub prim: usize,
}

/// Committed triangle scene. `rtcIntersect1` is safe to call from many threads.
pub struct Scene {
    scene: ScenePtr,
    triangles: Vec<Triangle>,
    _vertices: Vec<f32>,
    _indices: Vec<u32>,
}

unsafe impl Send for Scene {}
unsafe impl Sync for Scene {}

impl Scene {
    pub fn build(triangles: &[Triangle]) -> Self {
        let device = device();
        let scene = unsafe { rtcNewScene(device) };
        if scene.is_null() {
            check(device);
            panic!("Embree rtcNewScene returned null");
        }

        let mut vertices = Vec::with_capacity(triangles.len() * 9);
        let mut indices = Vec::with_capacity(triangles.len() * 3);
        for triangle in triangles {
            let base = (vertices.len() / 3) as u32;
            for vertex in triangle.vertices {
                vertices.extend_from_slice(&[vertex.x, vertex.y, vertex.z]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }

        if !triangles.is_empty() {
            let geometry = unsafe { rtcNewGeometry(device, GEOMETRY_TRIANGLE) };
            unsafe {
                rtcSetSharedGeometryBuffer(
                    geometry,
                    BUFFER_VERTEX,
                    0,
                    FORMAT_FLOAT3,
                    vertices.as_ptr().cast(),
                    0,
                    12,
                    vertices.len() / 3,
                );
                rtcSetSharedGeometryBuffer(
                    geometry,
                    BUFFER_INDEX,
                    0,
                    FORMAT_UINT3,
                    indices.as_ptr().cast(),
                    0,
                    12,
                    indices.len() / 3,
                );
                rtcCommitGeometry(geometry);
                rtcAttachGeometry(scene, geometry);
                rtcReleaseGeometry(geometry);
            }
        }

        unsafe { rtcCommitScene(scene) };
        check(device);

        Self {
            scene,
            triangles: triangles.to_vec(),
            _vertices: vertices,
            _indices: indices,
        }
    }

    pub fn occluded(&self, origin: Vec3, direction: Vec3, tfar: f32) -> bool {
        self.hit(origin, direction, tfar).is_some()
    }

    pub(crate) fn hit(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Hit> {
        if max_distance <= 0.0 || self.triangles.is_empty() {
            return None;
        }
        let mut ray = RayHit {
            org_x: origin.x,
            org_y: origin.y,
            org_z: origin.z,
            tnear: 0.0,
            dir_x: direction.x,
            dir_y: direction.y,
            dir_z: direction.z,
            time: 0.0,
            tfar: max_distance,
            mask: u32::MAX,
            id: 0,
            flags: 0,
            ng_x: 0.0,
            ng_y: 0.0,
            ng_z: 0.0,
            u: 0.0,
            v: 0.0,
            prim_id: 0,
            geom_id: INVALID_GEOMETRY,
            inst_id: INVALID_GEOMETRY,
            inst_prim_id: INVALID_GEOMETRY,
            _pad: [0, 0, 0],
        };
        unsafe { rtcIntersect1(self.scene, &mut ray, std::ptr::null_mut()) };
        if ray.geom_id == INVALID_GEOMETRY {
            return None;
        }
        let prim = ray.prim_id as usize;
        let tri = self.triangles.get(prim)?;
        let mut normal = (tri.vertices[1] - tri.vertices[0])
            .cross(tri.vertices[2] - tri.vertices[0])
            .normalize_or_zero();
        if normal.dot(direction) > 0.0 {
            normal = -normal;
        }
        let distance = ray.tfar;
        Some(Hit {
            point: origin + direction * distance,
            normal,
            distance,
            prim,
        })
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        unsafe { rtcReleaseScene(self.scene) };
    }
}
