mod gpu;

use eframe::egui::{self, Sense};
use glam::{Mat4, Vec3};
use solve::{room, solve, Rectangle, Room};

const PREVIEW_RAYS: u32 = 16;
const EXPOSURE: f32 = 0.75;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Source Lightbaker",
        options,
        Box::new(|cc| Ok(Box::new(Lightbaker::new(cc)))),
    )
}

struct Camera {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
}

impl Camera {
    fn eye(&self) -> Vec3 {
        let pitch = self.pitch.clamp(-1.2, 1.2);
        let horizontal = pitch.cos();
        let offset = Vec3::new(
            self.yaw.sin() * horizontal,
            self.yaw.cos() * horizontal,
            pitch.sin(),
        );
        self.target + offset * self.distance
    }

    fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        let correction = Mat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 1.0,
        ]);
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Z);
        let projection = Mat4::perspective_rh(0.9, aspect.max(0.1), 1.0, 8_000.0);
        (correction * projection * view).to_cols_array_2d()
    }
}

struct Lightbaker {
    room: Room,
    x: f32,
    y: f32,
    colors: Vec<[f32; 3]>,
    sealed: Vec<usize>,
    solves: u32,
    camera: Camera,
}

impl Lightbaker {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        gpu::init(cc);
        let fixture = room();
        let x = fixture.areas[0].center.x;
        let y = fixture.areas[0].center.y;
        let mut app = Self {
            room: fixture,
            x,
            y,
            colors: Vec::new(),
            sealed: Vec::new(),
            solves: 0,
            camera: Camera {
                yaw: 0.7,
                pitch: 0.95,
                distance: 450.0,
                target: Vec3::new(128.0, 128.0, 40.0),
            },
        };
        app.resolve();
        app
    }

    fn areas(&self) -> Vec<Rectangle> {
        let mut areas = self.room.areas.clone();
        areas[0].center.x = self.x;
        areas[0].center.y = self.y;
        areas
    }

    fn resolve(&mut self) {
        let receivers: Vec<_> = self
            .room
            .luxels
            .iter()
            .map(|luxel| luxel.receiver)
            .collect();
        let solved = solve(
            &self.room.triangles,
            &receivers,
            &self.areas(),
            PREVIEW_RAYS,
        );
        self.colors = solved.light;
        self.sealed = solved.sealed;
        self.solves += 1;
    }

    fn vertices(&self) -> Vec<gpu::Vertex> {
        let mut vertices = Vec::new();
        for (luxel, color) in self.room.luxels.iter().zip(&self.colors) {
            push_quad(&mut vertices, luxel.corners, display_color(*color));
        }
        for triangle in &self.room.grate {
            push_triangle(&mut vertices, triangle.vertices, [0.34, 0.36, 0.4]);
        }
        for triangle in &self.room.shell {
            let normal = (triangle.vertices[1] - triangle.vertices[0])
                .cross(triangle.vertices[2] - triangle.vertices[0]);
            if normal.z.abs() > normal.length() * 0.9 {
                continue;
            }
            push_triangle(&mut vertices, triangle.vertices, [0.55, 0.24, 0.2]);
        }
        for (index, area) in self.areas().iter().enumerate() {
            let color = if self.sealed.contains(&index) {
                [0.86, 0.28, 0.22]
            } else {
                [1.0, 0.72, 0.3]
            };
            push_quad(&mut vertices, area.corners(), color);
        }
        vertices
    }
}

impl eframe::App for Lightbaker {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::SidePanel::left("controls")
            .resizable(false)
            .exact_width(260.0)
            .show(ctx, |ui| {
                ui.heading("Первая площадка");
                let x_changed = ui
                    .add(egui::Slider::new(&mut self.x, 40.0..=216.0).text("X"))
                    .changed();
                let y_changed = ui
                    .add(egui::Slider::new(&mut self.y, 40.0..=216.0).text("Y"))
                    .changed();
                if x_changed || y_changed {
                    self.resolve();
                }
                ui.separator();
                ui.label("Площадки");
                for (index, _) in self.areas().iter().enumerate() {
                    let number = index + 1;
                    if self.sealed.contains(&index) {
                        ui.label(
                            egui::RichText::new(format!("{number} — глухая оболочка"))
                                .color(egui::Color32::from_rgb(214, 96, 78)),
                        );
                    } else {
                        ui.label(format!("{number} — светит"));
                    }
                }
                ui.label(format!("Лучей: {PREVIEW_RAYS}"));
                ui.label(format!("Расчётов: {}", self.solves));
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            let size = ui.available_size();
            let (rect, response) = ui.allocate_exact_size(size, Sense::drag());
            if response.dragged() {
                let delta = response.drag_delta();
                self.camera.yaw -= delta.x * 0.005;
                self.camera.pitch = (self.camera.pitch + delta.y * 0.005).clamp(-1.2, 1.2);
            }
            if response.hovered() {
                let scroll = ui.input(|input| input.raw_scroll_delta.y);
                if scroll != 0.0 {
                    self.camera.distance = (self.camera.distance - scroll).clamp(80.0, 1_200.0);
                }
            }

            let pixels = ui.ctx().pixels_per_point();
            let width = (rect.width() * pixels).round().max(1.0) as u32;
            let height = (rect.height() * pixels).round().max(1.0) as u32;
            let aspect = width as f32 / height as f32;
            ui.painter()
                .add(eframe::egui_wgpu::Callback::new_paint_callback(
                    rect,
                    gpu::RoomCallback {
                        vertices: self.vertices(),
                        view_proj: self.camera.view_proj(aspect),
                        width,
                        height,
                    },
                ));
        });
    }
}

fn display_color(linear: [f32; 3]) -> [f32; 3] {
    linear.map(|channel| (channel * EXPOSURE).clamp(0.0, 1.0))
}

fn push_quad(vertices: &mut Vec<gpu::Vertex>, corners: [Vec3; 4], color: [f32; 3]) {
    push_triangle(vertices, [corners[0], corners[1], corners[2]], color);
    push_triangle(vertices, [corners[0], corners[2], corners[3]], color);
}

fn push_triangle(vertices: &mut Vec<gpu::Vertex>, corners: [Vec3; 3], color: [f32; 3]) {
    for corner in corners {
        vertices.push(gpu::Vertex {
            position: corner.to_array(),
            color,
        });
    }
}
