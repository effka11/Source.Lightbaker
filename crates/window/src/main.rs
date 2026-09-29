mod gpu;

use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Response, Sense};
use glam::{Mat4, Vec3};
use lamps::{Kind, Lamp};
use map::Luxel;
use solve::{solve, Area, Disk, Receiver, Role};

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

fn map_path() -> Result<PathBuf, String> {
    if let Some(arg) = std::env::args().nth(1) {
        return Ok(PathBuf::from(arg));
    }
    if let Ok(path) = std::env::var("LIGHTBAKER_MAP") {
        return Ok(PathBuf::from(path));
    }
    let builtin =
        PathBuf::from(r"D:\Steam\steamapps\common\GarrysMod\garrysmod\maps\gm_construct.bsp");
    if builtin.is_file() {
        return Ok(builtin);
    }
    Err("Укажите BSP: window карта.bsp".into())
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
        let projection = Mat4::perspective_rh(0.9, aspect.max(0.1), 4.0, 64_000.0);
        (correction * projection * view).to_cols_array_2d()
    }
}

enum Phase {
    Preview,
    Solve,
    Live,
}

struct Done {
    light: Vec<[f32; 3]>,
    sealed: Vec<usize>,
    ms: u128,
}

struct Lightbaker {
    error: Option<String>,
    path: PathBuf,
    triangles_note: String,
    luxels: Vec<Luxel>,
    receivers: Arc<Vec<Receiver>>,
    triangles: Arc<Vec<solve::Triangle>>,
    file_lamps: Vec<Lamp>,
    kind: Kind,
    x: f32,
    y: f32,
    z: f32,
    x_span: (f32, f32),
    y_span: (f32, f32),
    z_span: (f32, f32),
    colors: Vec<[f32; 3]>,
    sealed: Vec<usize>,
    solves: u32,
    solve_ms: u128,
    phase: Phase,
    solve_rx: Option<mpsc::Receiver<Done>>,
    mesh: Option<Arc<Vec<gpu::Vertex>>>,
    mesh_id: u64,
    camera: Camera,
    snapshot: Option<Arc<map::Snapshot>>,
    bake_rx: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    bake_note: Option<String>,
    bake_failed: bool,
}

impl Lightbaker {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        gpu::init(cc);
        let mut app = Self {
            error: None,
            path: PathBuf::new(),
            triangles_note: String::new(),
            luxels: Vec::new(),
            receivers: Arc::new(Vec::new()),
            triangles: Arc::new(Vec::new()),
            file_lamps: Vec::new(),
            kind: Kind::Fluorescent,
            x: 0.0,
            y: 0.0,
            z: 96.0,
            x_span: (-800.0, 800.0),
            y_span: (-800.0, 800.0),
            z_span: (16.0, 400.0),
            colors: Vec::new(),
            sealed: Vec::new(),
            solves: 0,
            solve_ms: 0,
            phase: Phase::Preview,
            solve_rx: None,
            mesh: None,
            mesh_id: 0,
            camera: Camera {
                yaw: 0.8,
                pitch: 0.7,
                distance: 480.0,
                target: Vec3::ZERO,
            },
            snapshot: None,
            bake_rx: None,
            bake_note: None,
            bake_failed: false,
        };
        match map_path().and_then(|path| {
            map::open(&path)
                .map(|opened| (path, opened))
                .map_err(|err| err.to_string())
        }) {
            Ok((path, opened)) => app.load(path, opened),
            Err(err) => app.error = Some(err),
        }
        app
    }

    fn load(&mut self, path: PathBuf, opened: map::Map) {
        let floor = focus(&opened.luxels);
        self.file_lamps = lamps::load_beside(&path);
        self.x = floor.x;
        self.y = floor.y;
        self.z = floor.z + 96.0;
        self.x_span = (floor.x - 800.0, floor.x + 800.0);
        self.y_span = (floor.y - 800.0, floor.y + 800.0);
        self.z_span = (floor.z + 24.0, floor.z + 320.0);
        self.camera.target = floor + Vec3::new(0.0, 0.0, 48.0);
        self.triangles_note = format!(
            "{} · {} люкселей · {} треугольников",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("карта"),
            opened.luxels.len(),
            opened.triangles.len()
        );
        self.receivers = Arc::new(opened.luxels.iter().map(|luxel| luxel.receiver).collect());
        self.triangles = Arc::new(opened.triangles);
        self.snapshot = Some(Arc::new(opened.snapshot));
        self.luxels = opened.luxels;
        self.path = path;
        self.bake_note = None;
        self.bake_failed = false;
    }

    fn lamp(&self) -> Lamp {
        Lamp {
            kind: self.kind,
            position: Vec3::new(self.x, self.y, self.z),
            forward: Vec3::X,
        }
    }

    fn areas(&self) -> Vec<Area> {
        let mut areas: Vec<Area> = self.file_lamps.iter().copied().map(Lamp::area).collect();
        areas.push(self.lamp().area());
        areas
    }

    fn resolve(&mut self) {
        if self.receivers.is_empty() || self.solve_rx.is_some() {
            return;
        }
        let triangles = Arc::clone(&self.triangles);
        let receivers = Arc::clone(&self.receivers);
        let areas = self.areas();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let solved = solve(&triangles, &receivers, &areas, PREVIEW_RAYS);
            let _ = tx.send(Done {
                light: solved.light,
                sealed: solved.sealed,
                ms: started.elapsed().as_millis(),
            });
        });
        self.solve_rx = Some(rx);
        self.phase = Phase::Solve;
    }

    fn poll_solve(&mut self) -> bool {
        let Some(rx) = &self.solve_rx else {
            return true;
        };
        match rx.try_recv() {
            Ok(done) => {
                self.colors = done.light;
                self.sealed = done.sealed;
                self.solves += 1;
                self.solve_ms = done.ms;
                self.mesh = None;
                self.solve_rx = None;
                self.phase = Phase::Live;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.error = Some("расчёт прервался".into());
                self.solve_rx = None;
                true
            }
        }
    }

    fn start_bake(&mut self) {
        if self.bake_rx.is_some() || self.solve_rx.is_some() || self.receivers.is_empty() {
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let areas = self.areas();
        let triangles = Arc::clone(&self.triangles);
        let receivers = Arc::clone(&self.receivers);
        let source = self.path.clone();
        let destination = beside(&self.path);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let outcome = bake::bake(
                &triangles,
                &receivers,
                &areas,
                &snapshot,
                &source,
                &destination,
            )
            .map(|()| destination)
            .map_err(|err| err.to_string());
            let _ = tx.send(outcome);
        });
        self.bake_rx = Some(rx);
        self.bake_note = None;
        self.bake_failed = false;
    }

    fn poll_bake(&mut self) {
        let Some(rx) = &self.bake_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(path)) => {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("bsp");
                self.bake_note = Some(format!("Записано: {name}"));
                self.bake_failed = false;
                self.bake_rx = None;
            }
            Ok(Err(err)) => {
                self.bake_note = Some(err);
                self.bake_failed = true;
                self.bake_rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.bake_note = Some("запекание прервалось".into());
                self.bake_failed = true;
                self.bake_rx = None;
            }
        }
    }

    fn mesh(&mut self) -> Arc<Vec<gpu::Vertex>> {
        if self.mesh.is_none() {
            self.mesh = Some(Arc::new(self.build_vertices()));
            self.mesh_id = self.mesh_id.wrapping_add(1);
        }
        Arc::clone(self.mesh.as_ref().expect("mesh"))
    }

    fn build_vertices(&self) -> Vec<gpu::Vertex> {
        let mut vertices = Vec::with_capacity(self.luxels.len() * 6 + 96);
        for (index, luxel) in self.luxels.iter().enumerate() {
            let color = self
                .colors
                .get(index)
                .copied()
                .map(display_color)
                .unwrap_or_else(|| unlit(luxel.receiver.albedo));
            push_quad(&mut vertices, luxel.corners, color);
        }
        for (index, area) in self.areas().iter().enumerate() {
            let color = marker(*area, self.sealed.contains(&index));
            match area {
                Area::Rectangle(rectangle) => {
                    push_quad(&mut vertices, rectangle.corners(), color);
                }
                Area::Disk(disk) => push_disk(&mut vertices, disk, color),
            }
        }
        vertices
    }
}

impl eframe::App for Lightbaker {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_bake();
        if self.bake_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        match self.phase {
            Phase::Preview if self.error.is_none() => {
                self.resolve();
                ctx.request_repaint();
            }
            Phase::Solve => {
                if !self.poll_solve() {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
            Phase::Live | Phase::Preview => {}
        }

        egui::SidePanel::left("controls")
            .resizable(false)
            .exact_width(280.0)
            .show(ctx, |ui| {
                ui.heading("Карта");
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(214, 96, 78), error);
                    return;
                }
                ui.label(&self.triangles_note);
                if self.file_lamps.is_empty() {
                    ui.label("Лампы: пусто");
                } else {
                    ui.label(format!("Лампы: {}", self.file_lamps.len()));
                }
                ui.separator();
                ui.heading("Вид");
                let live = matches!(self.phase, Phase::Live) && self.bake_rx.is_none();
                ui.add_enabled_ui(live, |ui| {
                    let mut picked = self.kind;
                    for choice in Kind::ALL {
                        ui.selectable_value(&mut picked, choice, kind_name(choice));
                    }
                    if picked != self.kind {
                        self.kind = picked;
                        self.resolve();
                    }
                    let (x0, x1) = self.x_span;
                    let (y0, y1) = self.y_span;
                    let (z0, z1) = self.z_span;
                    let x = ui.add(egui::Slider::new(&mut self.x, x0..=x1).text("X"));
                    let y = ui.add(egui::Slider::new(&mut self.y, y0..=y1).text("Y"));
                    let z = ui.add(egui::Slider::new(&mut self.z, z0..=z1).text("Z"));
                    if released(&x) || released(&y) || released(&z) {
                        self.resolve();
                    }
                });
                ui.separator();
                ui.label("Площадки");
                let kind = self.kind;
                for (index, _) in self.areas().iter().enumerate() {
                    let number = index + 1;
                    if self.sealed.contains(&index) {
                        ui.label(
                            egui::RichText::new(format!("{number} — глухая оболочка"))
                                .color(egui::Color32::from_rgb(214, 96, 78)),
                        );
                    } else if index < self.file_lamps.len() {
                        ui.label(format!("{number} — из файла"));
                    } else {
                        ui.label(format!("{number} — {}", kind_name(kind)));
                    }
                }
                ui.separator();
                if ui.add_enabled(live, egui::Button::new("Запечь")).clicked() {
                    self.start_bake();
                }
                if self.bake_rx.is_some() {
                    ui.label("Запекаю…");
                }
                if let Some(note) = &self.bake_note {
                    if self.bake_failed {
                        ui.colored_label(egui::Color32::from_rgb(214, 96, 78), note);
                    } else {
                        ui.label(note);
                    }
                }
                ui.label(format!("Лучей: {PREVIEW_RAYS}"));
                match self.phase {
                    Phase::Live => {
                        ui.label(format!("Расчётов: {}", self.solves));
                        ui.label(format!("Время: {} мс", self.solve_ms));
                    }
                    _ => {
                        ui.label("Считаю свет…");
                    }
                }
            });

        if self.error.is_some() {
            return;
        }

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
                    self.camera.distance = (self.camera.distance - scroll).clamp(40.0, 12_000.0);
                }
            }

            let pixels = ui.ctx().pixels_per_point();
            let width = (rect.width() * pixels).round().max(1.0) as u32;
            let height = (rect.height() * pixels).round().max(1.0) as u32;
            let aspect = width as f32 / height as f32;
            let vertices = self.mesh();
            let mesh_id = self.mesh_id;
            let view_proj = self.camera.view_proj(aspect);
            ui.painter()
                .add(eframe::egui_wgpu::Callback::new_paint_callback(
                    rect,
                    gpu::RoomCallback {
                        vertices,
                        mesh_id,
                        view_proj,
                        width,
                        height,
                    },
                ));
        });
    }
}

fn beside(source: &std::path::Path) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("map");
    source.with_file_name(format!("{stem}_light.bsp"))
}

fn focus(luxels: &[Luxel]) -> Vec3 {
    let mut floors: Vec<Vec3> = luxels
        .iter()
        .filter(|luxel| {
            let point = luxel.receiver.position;
            luxel.receiver.role == Role::Floor
                && point.x.abs() < 6_000.0
                && point.y.abs() < 6_000.0
                && (-500.0..1_000.0).contains(&point.z)
        })
        .map(|luxel| luxel.receiver.position)
        .collect();
    if floors.is_empty() {
        return Vec3::new(0.0, 0.0, 0.0);
    }
    let mid = floors.len() / 2;
    floors.select_nth_unstable_by(mid, |left, right| left.x.total_cmp(&right.x));
    let x = floors[mid].x;
    floors.select_nth_unstable_by(mid, |left, right| left.y.total_cmp(&right.y));
    let y = floors[mid].y;
    floors.select_nth_unstable_by(mid, |left, right| left.z.total_cmp(&right.z));
    Vec3::new(x, y, floors[mid].z)
}

fn released(response: &Response) -> bool {
    response.drag_stopped() || (response.changed() && !response.dragged())
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Fluorescent => "Люминесцент",
        Kind::Bulb => "Колба",
        Kind::Sconce => "Бра",
        Kind::Camera => "Камера",
        Kind::Emergency => "Аварийная",
    }
}

fn marker(area: Area, sealed: bool) -> [f32; 3] {
    if sealed {
        return [0.86, 0.28, 0.22];
    }
    (Vec3::new(1.0, 0.72, 0.3) * area.color()).to_array()
}

fn display_color(linear: [f32; 3]) -> [f32; 3] {
    linear.map(|channel| (channel * EXPOSURE).clamp(0.0, 1.0))
}

fn unlit(albedo: Vec3) -> [f32; 3] {
    (albedo * 0.55).clamp(Vec3::ZERO, Vec3::ONE).to_array()
}

fn push_disk(vertices: &mut Vec<gpu::Vertex>, disk: &Disk, color: [f32; 3]) {
    let (axis, bitangent) = disk.frame();
    let steps = 16u32;
    for step in 0..steps {
        let a0 = step as f32 / steps as f32 * std::f32::consts::TAU;
        let a1 = (step + 1) as f32 / steps as f32 * std::f32::consts::TAU;
        let p0 = disk.center + (axis * a0.cos() + bitangent * a0.sin()) * disk.radius;
        let p1 = disk.center + (axis * a1.cos() + bitangent * a1.sin()) * disk.radius;
        push_triangle(vertices, [disk.center, p0, p1], color);
    }
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
