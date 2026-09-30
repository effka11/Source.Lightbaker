mod gpu;
mod project;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, CursorIcon, Id, Pos2, Rect, Response, RichText, Sense, Stroke, Vec2,
};
use glam::{Mat4, Vec3, Vec4};
use lamps::Kind;
use map::Luxel;
use project::{
    base_label, kind_label, read_project, relapse_fixtures, write_project, Base, LightObject,
    Space, RELAPSE,
};
use solve::{dynamic, solve_reporting, Area, Receiver, Role, Triangle, RAY_PASSES};

const PREVIEW_RAYS: u32 = 16;
const LOOK_SPEED: f32 = 0.0025;
const FLY_BOOST: f32 = 3.0;
const PICK_PAD: f32 = 4.0;
const SEAT_GAP: f32 = 1.0;
const SELECTION: [f32; 3] = [0.35, 0.92, 1.0];

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        // Decorations are off so the custom bar owns minimize, maximize, and close.
        // A fixed inner size fights maximize, so only a minimum size is set.
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_maximized(true)
            .with_resizable(true)
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Source Lightbaker",
        options,
        Box::new(|cc| Ok(Box::new(Lightbaker::new(cc)))),
    )
}

fn load_map(path: PathBuf, progress: &AtomicU64, stage: &AtomicU8) -> Result<Opened, String> {
    let publish = |phase: map::LoadPhase, done: u64, total: u64| {
        let (start, end) = match phase {
            map::LoadPhase::File => (0.0, 0.08),
            map::LoadPhase::World => (0.08, 0.25),
            map::LoadPhase::Packs => (0.25, 0.80),
            map::LoadPhase::Props => (0.80, 0.95),
        };
        let t = if total == 0 {
            1.0
        } else {
            (done as f32 / total as f32).clamp(0.0, 1.0)
        };
        let ticks = ((start + (end - start) * t) * 1000.0).round() as u64;
        progress.fetch_max(ticks, Ordering::Relaxed);
        stage.store(phase as u8, Ordering::Relaxed);
    };
    let opened = map::open_reporting(&path, &publish).map_err(|err| err.to_string())?;
    stage.store(4, Ordering::Relaxed);
    progress.store(1000, Ordering::Relaxed);
    Ok(Opened { path, map: opened })
}

fn draw_loading(ctx: &egui::Context, opening: &Opening) {
    let ticks = opening.done.load(Ordering::Relaxed).min(1000) as f32 / 1000.0;
    let percent = (ticks * 100.0).round() as u32;
    egui::CentralPanel::default().show(ctx, |ui| {
        let width = 460.0;
        ui.add_space((ui.available_height() * 0.42).max(0.0));
        ui.vertical_centered(|ui| {
            ui.set_max_width(width);
            ui.heading("Загрузка карты");
            ui.label(&opening.name);
            ui.label(load_phase_name(opening.phase.load(Ordering::Relaxed)));
            ui.add_space(8.0);
            ui.add(
                egui::ProgressBar::new(ticks)
                    .text(format!("{percent}%"))
                    .desired_width(width),
            );
        });
    });
}

fn load_phase_name(phase: u8) -> &'static str {
    match phase {
        0 => "Читаю карту",
        1 => "Собираю мир",
        2 => "Читаю модели",
        3 => "Ставлю пропы",
        _ => "Считаю свет",
    }
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

struct Opening {
    rx: mpsc::Receiver<Result<Opened, String>>,
    done: Arc<AtomicU64>,
    phase: Arc<AtomicU8>,
    name: String,
}

struct Opened {
    path: PathBuf,
    map: map::Map,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrthoKind {
    Top,
    Front,
    Side,
}

struct Pane {
    kind: OrthoKind,
    target: Vec3,
    span: f32,
}

const PANE_DISTANCE: f32 = 12_000.0;

impl Pane {
    /// Direction from the target back to the eye. Front looks toward +Y.
    fn view_axis(&self) -> Vec3 {
        match self.kind {
            OrthoKind::Top => Vec3::Z,
            OrthoKind::Front => -Vec3::Y,
            OrthoKind::Side => Vec3::X,
        }
    }

    fn eye(&self) -> Vec3 {
        self.target + self.view_axis() * PANE_DISTANCE
    }

    fn axes(&self) -> (Vec3, Vec3) {
        match self.kind {
            OrthoKind::Top => (Vec3::X, Vec3::Y),
            OrthoKind::Front => (Vec3::X, Vec3::Z),
            OrthoKind::Side => (Vec3::Y, Vec3::Z),
        }
    }

    fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        let half_height = self.span.max(20.0);
        let aspect = aspect.max(0.1);
        // Same projection family as the perspective view. A long lens keeps
        // the pane nearly flat while still landing in the depth range the
        // working camera uses.
        let fov = 2.0 * (half_height / PANE_DISTANCE).atan();
        let view = Mat4::look_at_rh(self.eye(), self.target, self.axes().1);
        let projection = Mat4::perspective_rh(fov, aspect, 20.0, PANE_DISTANCE * 4.0);
        (clip_correction() * projection * view).to_cols_array_2d()
    }
}

fn clip_correction() -> Mat4 {
    Mat4::from_cols_array(&[
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.5, 1.0,
    ])
}

fn ortho_label(kind: OrthoKind) -> &'static str {
    match kind {
        OrthoKind::Top => "Сверху",
        OrthoKind::Front => "Спереди",
        OrthoKind::Side => "Сбоку",
    }
}

struct Camera {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
    noclip: bool,
}

impl Camera {
    /// Direction from the orbit point to the eye.
    fn offset(&self) -> Vec3 {
        let pitch = self.pitch.clamp(-1.2, 1.2);
        let horizontal = pitch.cos();
        Vec3::new(
            self.yaw.sin() * horizontal,
            self.yaw.cos() * horizontal,
            pitch.sin(),
        )
    }

    fn eye(&self) -> Vec3 {
        self.target + self.offset() * self.distance
    }

    /// Turn around the orbit point. The eye moves, the point stays.
    fn orbit(&mut self, delta: egui::Vec2) {
        self.yaw -= delta.x * LOOK_SPEED;
        self.pitch = (self.pitch + delta.y * LOOK_SPEED).clamp(-1.2, 1.2);
    }

    /// Turn the view in place. The eye stays, the orbit point swings around it.
    /// Horizontal is opposite to orbiting.
    fn look(&mut self, delta: egui::Vec2) {
        let eye = self.eye();
        self.orbit(egui::vec2(-delta.x, delta.y));
        self.target = eye - self.offset() * self.distance;
    }

    /// Slides the orbit point. On the ground the step stays flat. In noclip it follows the view,
    /// Space and Ctrl climb in world Z, and Shift multiplies the speed.
    fn translate(&mut self, forward: f32, strafe: f32, vertical: f32, dt: f32, fast: bool) -> bool {
        let climb = if self.noclip {
            Vec3::Z * vertical
        } else {
            Vec3::ZERO
        };
        if dt <= 0.0 || (forward == 0.0 && strafe == 0.0 && climb == Vec3::ZERO) {
            return false;
        }
        let ahead = if self.noclip {
            -self.offset()
        } else {
            Vec3::new(-self.yaw.sin(), -self.yaw.cos(), 0.0)
        };
        let right = Vec3::new(-self.yaw.cos(), self.yaw.sin(), 0.0);
        let step = (ahead * forward + right * strafe + climb).normalize_or_zero();
        if step == Vec3::ZERO {
            return false;
        }
        let mut speed = (self.distance * 1.2).clamp(80.0, 4_000.0);
        if fast {
            speed *= FLY_BOOST;
        }
        self.target += step * speed * dt.min(0.1);
        true
    }

    fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Z);
        let projection = Mat4::perspective_rh(0.9, aspect.max(0.1), 4.0, 64_000.0);
        (clip_correction() * projection * view).to_cols_array_2d()
    }
}

#[derive(Clone)]
struct Revision {
    objects: Vec<LightObject>,
    categories: Vec<String>,
    selected: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picture {
    Dynamic,
    Real,
}

struct Done {
    light: Vec<[f32; 3]>,
    sealed: Vec<usize>,
    ms: u128,
    generation: u64,
}

struct Lightbaker {
    error: Option<String>,
    path: PathBuf,
    triangles_note: String,
    luxels: Vec<Luxel>,
    receivers: Arc<Vec<Receiver>>,
    triangles: Arc<Vec<solve::Triangle>>,
    surface: Vec<map::Surface>,
    project_path: Option<PathBuf>,
    categories: Vec<String>,
    objects: Vec<LightObject>,
    draft_category: String,
    search: String,
    export_open: bool,
    export_path: String,
    export_started: Option<Instant>,
    dock_height: f32,
    panes: [Pane; 3],
    pane_share: [f32; 3],
    pane_drag: Option<usize>,
    picture: Picture,
    generation: u64,
    dynamic_colors: Vec<[f32; 3]>,
    traced: Vec<[f32; 3]>,
    traced_generation: u64,
    colors: Vec<[f32; 3]>,
    sealed: Vec<usize>,
    solves: u32,
    solve_ms: u128,
    solve_rx: Option<mpsc::Receiver<Done>>,
    solve_done: Arc<AtomicU64>,
    solve_units: u64,
    positions: Option<Arc<Vec<[f32; 3]>>>,
    position_id: u64,
    shown: Option<Arc<Vec<[f32; 3]>>>,
    color_id: u64,
    marker_positions: Option<Arc<Vec<[f32; 3]>>>,
    marker_colors: Option<Arc<Vec<[f32; 3]>>>,
    marker_id: u64,
    camera: Camera,
    cursor_held: bool,
    selected: Option<usize>,
    object_menu: Option<(usize, Pos2)>,
    undo: Vec<Revision>,
    redo: Vec<Revision>,
    undo_group: bool,
    dragging_lamp: bool,
    lamp_moved: bool,
    snapshot: Option<Arc<map::Snapshot>>,
    opening: Option<Opening>,
    bake_rx: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    bake_done: Arc<AtomicU64>,
    bake_units: u64,
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
            surface: Vec::new(),
            project_path: None,
            categories: vec![RELAPSE.to_owned()],
            objects: Vec::new(),
            draft_category: String::new(),
            search: String::new(),
            export_open: false,
            export_path: String::new(),
            export_started: None,
            dock_height: 240.0,
            panes: [
                Pane {
                    kind: OrthoKind::Top,
                    target: Vec3::ZERO,
                    span: 900.0,
                },
                Pane {
                    kind: OrthoKind::Front,
                    target: Vec3::ZERO,
                    span: 900.0,
                },
                Pane {
                    kind: OrthoKind::Side,
                    target: Vec3::ZERO,
                    span: 900.0,
                },
            ],
            pane_share: [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
            pane_drag: None,
            picture: Picture::Dynamic,
            generation: 0,
            dynamic_colors: Vec::new(),
            traced: Vec::new(),
            traced_generation: 0,
            colors: Vec::new(),
            sealed: Vec::new(),
            solves: 0,
            solve_ms: 0,
            solve_rx: None,
            solve_done: Arc::new(AtomicU64::new(0)),
            solve_units: 0,
            positions: None,
            position_id: 0,
            shown: None,
            color_id: 0,
            marker_positions: None,
            marker_colors: None,
            marker_id: 0,
            camera: Camera {
                yaw: 0.8,
                pitch: 0.7,
                distance: 480.0,
                target: Vec3::ZERO,
                noclip: false,
            },
            cursor_held: false,
            selected: None,
            object_menu: None,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_group: false,
            dragging_lamp: false,
            lamp_moved: false,
            snapshot: None,
            opening: None,
            bake_rx: None,
            bake_done: Arc::new(AtomicU64::new(0)),
            bake_units: 0,
            bake_note: None,
            bake_failed: false,
        };
        match map_path() {
            Ok(path) => app.begin_open(path),
            Err(err) => app.error = Some(err),
        }
        app
    }

    fn begin_open(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("карта")
            .to_string();
        let done = Arc::new(AtomicU64::new(0));
        let phase = Arc::new(AtomicU8::new(0));
        let (tx, rx) = mpsc::channel();
        let progress = Arc::clone(&done);
        let stage = Arc::clone(&phase);
        std::thread::spawn(move || {
            let _ = tx.send(load_map(path, &progress, &stage));
        });
        self.error = None;
        self.opening = Some(Opening {
            rx,
            done,
            phase,
            name,
        });
    }

    fn poll_opening(&mut self) {
        let message = self
            .opening
            .as_ref()
            .and_then(|opening| match opening.rx.try_recv() {
                Ok(message) => Some(message),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("загрузка прервалась".into())),
            });
        let Some(message) = message else {
            return;
        };
        self.opening = None;
        match message {
            Ok(opened) => self.load(opened.path, opened.map),
            Err(err) => self.error = Some(err),
        }
    }

    fn load(&mut self, path: PathBuf, opened: map::Map) {
        let floor = focus(&opened.luxels);
        if self.objects.is_empty() {
            self.objects
                .push(LightObject::fluorescent(floor + Vec3::new(0.0, 0.0, 96.0)));
        }
        self.camera.target = floor + Vec3::new(0.0, 0.0, 48.0);
        for pane in &mut self.panes {
            pane.target = floor;
            pane.span = 900.0;
        }
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
        self.surface = opened.surface;
        self.snapshot = Some(Arc::new(opened.snapshot));
        self.luxels = opened.luxels;
        self.picture = Picture::Dynamic;
        self.generation = self.generation.wrapping_add(1);
        self.traced.clear();
        self.traced_generation = 0;
        self.dynamic_colors = dynamic(&self.receivers, &self.areas());
        self.colors.clone_from(&self.dynamic_colors);
        self.sealed.clear();
        self.positions = None;
        self.shown = None;
        self.marker_positions = None;
        self.marker_colors = None;
        if self
            .selected
            .is_some_and(|index| index >= self.objects.len())
        {
            self.selected = None;
        }
        self.dragging_lamp = false;
        self.lamp_moved = false;
        self.path = path;
        self.bake_note = None;
        self.bake_failed = false;
        self.undo.clear();
        self.redo.clear();
        self.undo_group = false;
    }

    fn areas(&self) -> Vec<Area> {
        self.objects.iter().flat_map(LightObject::areas).collect()
    }

    fn lamps_moved(&mut self) {
        self.preview_lamp();
        if self.picture == Picture::Real && self.solve_rx.is_none() {
            self.resolve();
        }
    }

    fn preview_lamp(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.sealed.clear();
        self.dynamic_colors = dynamic(&self.receivers, &self.areas());
        self.present();
    }

    fn editing(&self) -> bool {
        self.solve_rx.is_none() && self.bake_rx.is_none()
    }

    fn showing_real(&self) -> bool {
        self.picture == Picture::Real && self.traced_generation == self.generation
    }

    fn present(&mut self) {
        if self.showing_real() {
            self.colors.clone_from(&self.traced);
        } else {
            self.colors.clone_from(&self.dynamic_colors);
        }
    }

    fn resolve(&mut self) {
        if self.receivers.is_empty() || self.solve_rx.is_some() {
            return;
        }
        let triangles = Arc::clone(&self.triangles);
        let receivers = Arc::clone(&self.receivers);
        let areas = self.areas();
        let generation = self.generation;
        self.solve_done.store(0, Ordering::Relaxed);
        self.solve_units = receivers.len() as u64 * RAY_PASSES;
        let done = Arc::clone(&self.solve_done);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let solved = solve_reporting(&triangles, &receivers, &areas, PREVIEW_RAYS, &|step| {
                done.fetch_add(step, Ordering::Relaxed);
            });
            let _ = tx.send(Done {
                light: solved.light,
                sealed: solved.sealed,
                ms: started.elapsed().as_millis(),
                generation,
            });
        });
        self.solve_rx = Some(rx);
        self.present();
    }

    fn poll_solve(&mut self) -> bool {
        let Some(rx) = &self.solve_rx else {
            return true;
        };
        match rx.try_recv() {
            Ok(done) => {
                self.solves += 1;
                self.solve_ms = done.ms;
                self.solve_rx = None;
                if done.generation == self.generation {
                    self.traced = done.light;
                    self.traced_generation = done.generation;
                    self.sealed = done.sealed;
                    self.present();
                } else if self.picture == Picture::Real {
                    self.resolve();
                }
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
        let destination = if self.export_path.trim().is_empty() {
            beside(&self.path)
        } else {
            ensure_ext(PathBuf::from(self.export_path.trim()), "bsp")
        };
        self.export_started = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        self.bake_done.store(0, Ordering::Relaxed);
        self.bake_units = receivers.len() as u64 * RAY_PASSES;
        let done = Arc::clone(&self.bake_done);
        std::thread::spawn(move || {
            let outcome = bake::bake_reporting(
                &triangles,
                &receivers,
                &areas,
                &snapshot,
                &source,
                &destination,
                &|step| {
                    done.fetch_add(step, Ordering::Relaxed);
                },
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

    fn drive_camera(&mut self, ctx: &egui::Context) {
        if self.error.is_none() && !ctx.wants_keyboard_input() {
            self.move_camera(ctx);
        }
        self.aim(ctx);
        self.hold_cursor(ctx);
    }

    fn move_camera(&mut self, ctx: &egui::Context) {
        let (forward, strafe, vertical, toggle_noclip, fast, dt) = ctx.input(|input| {
            let mut forward = 0.0;
            let mut strafe = 0.0;
            let mut vertical = 0.0;
            if input.key_down(egui::Key::W) {
                forward += 1.0;
            }
            if input.key_down(egui::Key::S) {
                forward -= 1.0;
            }
            if input.key_down(egui::Key::D) {
                strafe += 1.0;
            }
            if input.key_down(egui::Key::A) {
                strafe -= 1.0;
            }
            if input.key_down(egui::Key::Space) {
                vertical += 1.0;
            }
            if input.modifiers.ctrl {
                vertical -= 1.0;
            }
            (
                forward,
                strafe,
                vertical,
                input.key_pressed(egui::Key::V),
                input.modifiers.shift,
                input.stable_dt,
            )
        });
        if toggle_noclip {
            self.camera.noclip = !self.camera.noclip;
            ctx.request_repaint();
        }
        if self.camera.translate(forward, strafe, vertical, dt, fast) {
            ctx.request_repaint();
        }
    }

    /// In noclip the cursor stays put, so the turn comes from the raw mouse motion.
    fn aim(&mut self, ctx: &egui::Context) {
        if !self.camera.noclip || self.error.is_some() {
            return;
        }
        let (delta, middle) = ctx.input(|input| {
            let scale = input.pixels_per_point.max(0.1);
            let delta = match input.pointer.motion() {
                Some(motion) => motion / scale,
                None => input.pointer.delta(),
            };
            (
                delta,
                input.pointer.button_down(egui::PointerButton::Middle),
            )
        });
        if delta == egui::Vec2::ZERO {
            return;
        }
        if middle {
            self.camera.orbit(delta);
        } else {
            self.camera.look(delta);
        }
    }

    /// Freeze the cursor in the window for as long as noclip is on and the window is focused.
    /// The hide command is sent every frame: egui shows the cursor again when it sets the icon.
    fn hold_cursor(&mut self, ctx: &egui::Context) {
        let hold = self.camera.noclip && self.error.is_none() && ctx.input(|input| input.focused);
        if hold && !self.cursor_held {
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(
                ctx.screen_rect().center(),
            ));
        }
        if hold {
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::Locked));
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(false));
        } else if self.cursor_held {
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(egui::CursorGrab::None));
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
        }
        self.cursor_held = hold;
    }

    /// Double-click selects an object. A click that misses it clears the selection.
    /// Dragging the selected object seats its hitbox on the surface under the cursor.
    fn steer_lamp(
        &mut self,
        ui: &egui::Ui,
        response: &Response,
        rect: egui::Rect,
        view_proj: [[f32; 4]; 4],
        eye: Vec3,
    ) {
        let primary = egui::PointerButton::Primary;
        if self.camera.noclip {
            if self.dragging_lamp {
                self.finish_lamp_drag();
            }
            return;
        }
        let pos = response.interact_pointer_pos();
        let picked = pos.and_then(|pos| self.object_under(rect, view_proj, eye, pos));
        if response.double_clicked_by(primary) {
            self.finish_lamp_drag();
            self.selected = picked;
            self.object_menu = None;
        } else if response.secondary_clicked() {
            self.object_menu =
                picked.map(|index| (index, response.interact_pointer_pos().unwrap_or(Pos2::ZERO)));
            if let Some((index, _)) = self.object_menu {
                self.selected = Some(index);
            }
        } else if response.clicked_by(primary) && picked != self.selected {
            self.selected = None;
            self.object_menu = None;
        }
        if response.drag_started_by(primary) && self.editing() {
            let pressed = ui
                .input(|input| input.pointer.press_origin())
                .and_then(|pos| self.object_under(rect, view_proj, eye, pos));
            self.dragging_lamp = self.selected.is_some() && pressed == self.selected;
        }
        if self.dragging_lamp && response.dragged_by(primary) {
            if let Some(pos) = pos {
                self.drag_lamp(rect, view_proj, eye, pos);
            }
        }
        if response.drag_stopped_by(primary) && self.dragging_lamp {
            self.finish_lamp_drag();
        }
    }

    fn finish_lamp_drag(&mut self) {
        self.dragging_lamp = false;
        if self.lamp_moved {
            self.lamp_moved = false;
            if self.picture == Picture::Real && self.solve_rx.is_none() {
                self.resolve();
            }
        }
    }

    fn object_under(
        &self,
        rect: egui::Rect,
        view_proj: [[f32; 4]; 4],
        eye: Vec3,
        pos: egui::Pos2,
    ) -> Option<usize> {
        let (origin, dir) = matrix_ray_at(view_proj, eye, rect, pos)?;
        self.pick_object(origin, dir)
    }

    fn pick_object(&self, origin: Vec3, dir: Vec3) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (index, object) in self.objects.iter().enumerate() {
            let (min, max) = object.bounds();
            let center = (min + max) * 0.5;
            let half = ((max - min) * 0.5).max(Vec3::splat(PICK_PAD));
            let Some(distance) = ray_box(origin, dir, center, half) else {
                continue;
            };
            if best.is_none_or(|(so_far, _)| distance < so_far) {
                best = Some((distance, index));
            }
        }
        best.map(|(_, index)| index)
    }

    fn drag_lamp(
        &mut self,
        rect: egui::Rect,
        view_proj: [[f32; 4]; 4],
        eye: Vec3,
        pos: egui::Pos2,
    ) {
        let Some(index) = self.selected else {
            return;
        };
        let Some((origin, dir)) = matrix_ray_at(view_proj, eye, rect, pos) else {
            return;
        };
        let Some((hit, normal)) = nearest_surface(origin, dir, &self.triangles) else {
            return;
        };
        let (min, max) = self.objects[index].bounds();
        let placed = seat_on(hit, normal, (max - min) * 0.5);
        if (placed - self.objects[index].position).length_squared() < 1.0e-6 {
            return;
        }
        if !self.lamp_moved {
            self.remember();
            self.undo_group = false;
        }
        self.objects[index].position = placed;
        self.preview_lamp();
        self.lamp_moved = true;
    }

    fn positions(&mut self) -> (Arc<Vec<[f32; 3]>>, u64) {
        if self.positions.is_none() {
            let mut positions = Vec::with_capacity(self.surface.len() * 3);
            for tri in &self.surface {
                push_triangle_positions(&mut positions, tri.vertices);
            }
            self.positions = Some(Arc::new(positions));
            self.position_id = self.position_id.wrapping_add(1);
        }
        (
            Arc::clone(self.positions.as_ref().expect("surface positions")),
            self.position_id,
        )
    }

    fn shown_colors(&mut self) -> (Arc<Vec<[f32; 3]>>, u64) {
        if self.shown.is_none() {
            let mut colors = Vec::with_capacity(self.surface.len() * 3);
            for tri in &self.surface {
                let color = surface_color(tri.albedo);
                push_repeat(&mut colors, color, 3);
            }
            self.shown = Some(Arc::new(colors));
            self.color_id = self.color_id.wrapping_add(1);
        }
        (
            Arc::clone(self.shown.as_ref().expect("surface colors")),
            self.color_id,
        )
    }

    fn markers(&mut self) -> (Arc<Vec<[f32; 3]>>, Arc<Vec<[f32; 3]>>, u64) {
        let mut positions = Vec::new();
        let mut colors = Vec::new();
        let mut cursor = 0usize;
        for (index, object) in self.objects.iter().enumerate() {
            let count = object.areas().len();
            let sealed = self.showing_real()
                && (cursor..cursor + count).any(|area| self.sealed.contains(&area));
            cursor += count;
            let chosen = self.selected == Some(index);
            let color = if chosen {
                SELECTION
            } else if sealed {
                [0.86, 0.28, 0.22]
            } else {
                object.color.min(Vec3::ONE).to_array()
            };
            let shape = object.shape_vertices();
            push_repeat(&mut colors, color, shape.len());
            positions.extend(shape);
            if chosen {
                let (min, max) = object.bounds();
                push_box_frame(
                    &mut positions,
                    &mut colors,
                    (min + max) * 0.5,
                    selection_half((max - min) * 0.5),
                    SELECTION,
                );
            }
        }
        let same = self
            .marker_positions
            .as_ref()
            .is_some_and(|cached| cached.as_slice() == positions)
            && self
                .marker_colors
                .as_ref()
                .is_some_and(|cached| cached.as_slice() == colors);
        if !same {
            self.marker_positions = Some(Arc::new(positions));
            self.marker_colors = Some(Arc::new(colors));
            self.marker_id = self.marker_id.wrapping_add(1);
        }
        (
            Arc::clone(self.marker_positions.as_ref().expect("marker positions")),
            Arc::clone(self.marker_colors.as_ref().expect("marker colors")),
            self.marker_id,
        )
    }
}

impl eframe::App for Lightbaker {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_opening();
        self.shortcuts(ctx);
        self.chrome(ctx);
        if self.opening.is_some() {
            draw_loading(ctx, self.opening.as_ref().expect("opening"));
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        self.poll_bake();
        self.drive_camera(ctx);
        if self.bake_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.solve_rx.is_some() {
            let finished = self.poll_solve();
            if self.solve_rx.is_some() {
                if finished {
                ctx.request_repaint();
                } else {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
        }

        self.left_panel(ctx);
        if self.luxels.is_empty() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                if let Some(error) = &self.error {
                        ui.colored_label(Color32::from_rgb(214, 96, 78), error);
                    } else {
                        ui.label("Откройте карту через Файл → Импорт карты");
                }
                });
            });
                } else {
            self.workspace(ctx);
            self.inspectors(ctx);
        }
        self.export_window(ctx);
        self.object_actions(ctx);
    }
}

impl Lightbaker {
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (save_as, save) = ctx.input_mut(|input| {
            let save_as = input.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::S,
            );
            let save = input.consume_key(egui::Modifiers::COMMAND, egui::Key::S);
            (save_as, save)
        });
        if save_as {
            self.save_project_as();
        } else if save {
            self.save_project();
        }
        if !ctx.wants_keyboard_input() {
            let (redo, undo) = ctx.input_mut(|input| {
                let redo = input.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                );
                let undo = input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
                (redo, undo)
            });
            if redo {
                self.redo_edit();
            } else if undo {
                self.undo_edit();
            }
        }
        let delete = !ctx.wants_keyboard_input()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Delete));
        if delete {
            if let Some(index) = self.selected {
                self.delete_object(index);
            }
        }
    }

    fn chrome(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("chrome")
            .exact_height(40.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(22, 22, 24))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                let drag = ui.interact(bar, Id::new("chrome-drag"), Sense::click_and_drag());
                if drag.double_clicked_by(egui::PointerButton::Primary) {
                    let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if drag.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                let icon = Rect::from_center_size(
                    Pos2::new(bar.left() + 18.0, bar.center().y),
                    Vec2::splat(16.0),
                );
                ui.painter()
                    .rect_filled(icon, 4.0, Color32::from_rgb(58, 58, 64));
                ui.painter().rect_stroke(
                    icon,
                    4.0,
                    Stroke::new(1.0_f32, Color32::from_rgb(82, 82, 90)),
                    egui::StrokeKind::Inside,
                );

                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(Rect::from_min_size(
                        Pos2::new(bar.left() + 36.0, bar.top() + 6.0),
                        Vec2::new(84.0, 28.0),
                    )),
                    |ui| {
                        flat_widgets(ui);
                        let _ = ui.menu_button("Файл", |ui| {
                            ui.set_min_width(230.0);
                            if ui.button("Открыть проект").clicked() {
                                self.open_project();
                            }
                            if ui.button("Импорт карты").clicked() {
                                self.import_map();
                            }
                ui.separator();
                            if menu_line(ui, "Сохранить", "Ctrl+S") {
                                self.save_project();
                            }
                            if menu_line(ui, "Сохранить как…", "Ctrl+Shift+S") {
                                self.save_project_as();
                            }
                            ui.separator();
                            if ui.button("Экспорт в .bsp").clicked() {
                                self.open_export();
                            }
                        });
                    },
                );

                let search_width = (bar.width() * 0.34).clamp(220.0, 460.0);
                let search_rect =
                    Rect::from_center_size(bar.center(), Vec2::new(search_width, 26.0));
                ui.painter()
                    .rect_filled(search_rect, 8.0, Color32::from_rgb(38, 38, 42));
                ui.painter().rect_stroke(
                    search_rect,
                    8.0,
                    Stroke::new(1.0_f32, Color32::from_rgb(64, 64, 70)),
                    egui::StrokeKind::Inside,
                );
                let _ = ui.put(
                    search_rect.shrink2(Vec2::new(10.0, 3.0)),
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Поиск объектов")
                        .frame(false)
                        .desired_width(search_rect.width() - 20.0),
                );
                self.search_popup(ctx, search_rect);

                let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                let button = 46.0;
                let close = Rect::from_min_max(
                    Pos2::new(bar.right() - button, bar.top()),
                    bar.right_bottom(),
                );
                let max = Rect::from_min_max(
                    Pos2::new(close.left() - button, bar.top()),
                    Pos2::new(close.left(), bar.bottom()),
                );
                let min = Rect::from_min_max(
                    Pos2::new(max.left() - button, bar.top()),
                    Pos2::new(max.left(), bar.bottom()),
                );
                if caption(ui, min, Caption::Min).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                if caption(
                    ui,
                    max,
                    if maximized {
                        Caption::Restore
                    } else {
                        Caption::Max
                    },
                )
                .clicked()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if caption(ui, close, Caption::Close).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ui.painter().hline(
                    bar.x_range(),
                    bar.bottom() - 0.5,
                    Stroke::new(1.0_f32, Color32::from_rgb(46, 46, 50)),
                );
            });
    }

    fn search_popup(&mut self, ctx: &egui::Context, search_rect: Rect) {
        if self.search.trim().is_empty() {
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.search.clear();
            return;
        }
        let query = self.search.trim().to_lowercase();
        let hits: Vec<usize> = self
            .objects
            .iter()
            .enumerate()
            .filter(|(_, object)| object.name.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .take(12)
            .collect();
        if hits.is_empty() {
            return;
        }
        let mut pick = None;
        egui::Area::new(Id::new("object-search"))
            .order(egui::Order::Foreground)
            .fixed_pos(search_rect.left_bottom() + Vec2::new(0.0, 4.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(32, 32, 36))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_min_width(search_rect.width());
                        for index in hits {
                            let label = if self.objects[index].category.is_empty() {
                                self.objects[index].name.clone()
                            } else {
                                format!(
                                    "{} · {}",
                                    self.objects[index].name, self.objects[index].category
                                )
                            };
                            if ui
                                .selectable_label(self.selected == Some(index), label)
                                .clicked()
                            {
                                pick = Some(index);
                            }
                        }
                    });
            });
        if let Some(index) = pick {
            self.selected = Some(index);
            self.search.clear();
        }
    }

    fn left_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("controls")
            .resizable(true)
            .default_width(280.0)
            .width_range(220.0..=420.0)
            .show(ctx, |ui| {
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::from_rgb(214, 96, 78), error);
                }
                let title = self
                    .path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Нет карты");
                ui.heading(title);
                if !self.triangles_note.is_empty() {
                    ui.label(RichText::new(&self.triangles_note).size(12.0));
                }
                ui.separator();
                ui.label("Режим");
                if ui
                    .selectable_label(self.picture == Picture::Dynamic, "Динамический свет")
                    .clicked()
                    && self.picture != Picture::Dynamic
                {
                    self.picture = Picture::Dynamic;
                    self.present();
                }
                if ui
                    .selectable_label(self.picture == Picture::Real, "Собираемый свет")
                    .clicked()
                    && self.picture != Picture::Real
                {
                    self.picture = Picture::Real;
                    if self.traced_generation != self.generation && self.solve_rx.is_none() {
                        self.resolve();
                    } else {
                        self.present();
                    }
                }
                ui.separator();
                ui.label("Объекты");
                let at = self.camera.target;
                let editing = self.editing();
                egui::CollapsingHeader::new("Создать объект").show(ui, |ui| {
                    ui.add_enabled_ui(editing, |ui| {
                        egui::CollapsingHeader::new("База")
                            .default_open(true)
                            .show(ui, |ui| {
                                for base in [Base::Square, Base::Circle, Base::Cube, Base::Sphere] {
                                    if ui.button(base_label(base)).clicked() {
                                        self.add_object(LightObject::from_base(base, at));
                                    }
                                }
                            });
                        egui::CollapsingHeader::new("Relapse")
                            .default_open(true)
                            .show(ui, |ui| {
                                for fixture in relapse_fixtures() {
                                    if ui.button(fixture.name).clicked() {
                                        if !self.categories.iter().any(|item| item == RELAPSE) {
                                            self.categories.insert(0, RELAPSE.to_owned());
                                        }
                                        self.add_object(LightObject::from_fixture(fixture, at));
                                    }
                                }
                            });
                        egui::CollapsingHeader::new("Шаблоны")
                            .default_open(true)
                            .show(ui, |ui| {
                                for kind in Kind::ALL {
                                    if ui.button(kind_label(kind)).clicked() {
                                        self.add_object(LightObject::from_template(kind, at));
                                    }
                                }
                            });
                    });
                });
                ui.separator();
                let list_height = (ui.available_height() - 92.0).max(48.0);
                egui::ScrollArea::vertical()
                    .id_salt("objects")
                    .max_height(list_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.objects.is_empty() {
                            ui.label(
                                RichText::new("Пока пусто").color(Color32::from_rgb(150, 150, 156)),
                            );
                        }
                        for index in 0..self.objects.len() {
                            let title = if self.objects[index].category.is_empty() {
                                self.objects[index].name.clone()
                    } else {
                                format!(
                                    "{} · {}",
                                    self.objects[index].name, self.objects[index].category
                                )
                            };
                            let item = ui.selectable_label(self.selected == Some(index), title);
                            if item.clicked() || item.secondary_clicked() {
                                self.selected = Some(index);
                                self.object_menu = None;
                            }
                            let mut remove = false;
                            item.context_menu(|ui| {
                                ui.set_min_width(160.0);
                                if ui
                                    .add_enabled(self.editing(), egui::Button::new("Удалить"))
                                    .clicked()
                                {
                                    remove = true;
                                }
                            });
                            if remove {
                                self.delete_object(index);
                                break;
                            }
                        }
                    });
                ui.separator();
                if self.solve_rx.is_some() {
                    let done = self.solve_done.load(Ordering::Relaxed);
                    let units = self.solve_units.max(1);
                    let fraction = (done as f32 / units as f32).clamp(0.0, 1.0);
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .text(format!("{}%", (fraction * 100.0).round() as u32)),
                    );
                } else if self.showing_real() {
                ui.label(format!("Лучей: {PREVIEW_RAYS}"));
                        ui.label(format!("Расчётов: {}", self.solves));
                        ui.label(format!("Время: {} мс", self.solve_ms));
                    }
            });
    }

    fn workspace(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let full = ui.max_rect();
            let limit = (full.height() - 160.0).max(140.0);
            self.dock_height = self.dock_height.clamp(140.0, limit);
            let split_y = full.bottom() - self.dock_height;
            let splitter = Rect::from_min_max(
                Pos2::new(full.left(), split_y - 3.0),
                Pos2::new(full.right(), split_y + 3.0),
            );
            let split = ui.interact(splitter, Id::new("dock-split"), Sense::drag());
            if split.dragged() {
                self.dock_height = (self.dock_height - split.drag_delta().y).clamp(140.0, limit);
            }
            split.on_hover_cursor(CursorIcon::ResizeVertical);
            ui.painter()
                .rect_filled(splitter, 0.0, Color32::from_rgb(28, 28, 30));
            let main = Rect::from_min_max(full.left_top(), Pos2::new(full.right(), split_y - 3.0));
            self.perspective(ui, main);
            let dock =
                Rect::from_min_max(Pos2::new(full.left(), split_y + 3.0), full.right_bottom());
            self.dock(ui, dock);
        });
    }

    fn perspective(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let response = ui.interact(rect, Id::new("perspective"), Sense::click_and_drag());
                let delta = response.drag_delta();
        if !self.camera.noclip && response.dragged_by(egui::PointerButton::Middle) {
            self.camera.orbit(delta);
        } else if !self.camera.noclip && response.dragged_by(egui::PointerButton::Secondary) {
            self.camera.look(delta);
            }
            if response.hovered() {
                let scroll = ui.input(|input| input.raw_scroll_delta.y);
                if scroll != 0.0 {
                    self.camera.distance = (self.camera.distance - scroll).clamp(40.0, 12_000.0);
            }
        }
        let aspect = aspect_of(ui, rect);
        let view_proj = self.camera.view_proj(aspect);
        let eye = self.camera.eye();
        self.steer_lamp(ui, &response, rect, view_proj, eye);
        self.paint_scene(ui, rect, 0, view_proj);
        if self.camera.noclip {
            noclip_hint(ui, rect);
        }
    }

    fn dock(&mut self, ui: &mut egui::Ui, dock: Rect) {
        let gap = 5.0;
        let inner = (dock.width() - gap * 2.0).max(1.0);
        let mut widths = [inner * self.pane_share[0], inner * self.pane_share[1], 0.0];
        widths[2] = (inner - widths[0] - widths[1]).max(40.0);
        let mut rects = [Rect::NOTHING; 3];
        let mut x = dock.left();
        for index in 0..3 {
            let pane = Rect::from_min_size(
                Pos2::new(x, dock.top()),
                Vec2::new(widths[index], dock.height()),
            );
            rects[index] = pane;
            self.ortho_pane(ui, index, pane);
            x += widths[index];
            if index < 2 {
                let split =
                    Rect::from_min_size(Pos2::new(x, dock.top()), Vec2::new(gap, dock.height()));
                let response = ui.interact(
                    split.expand2(Vec2::new(3.0, 0.0)),
                    Id::new(("pane-split", index)),
                    Sense::drag(),
                );
                if response.dragged() {
                    let delta = response.drag_delta().x / dock.width().max(1.0);
                    self.pane_share[index] = (self.pane_share[index] + delta).max(0.15);
                    self.pane_share[index + 1] = (self.pane_share[index + 1] - delta).max(0.15);
                    let sum = self.pane_share.iter().sum::<f32>();
                    for share in &mut self.pane_share {
                        *share /= sum;
                    }
                }
                response.on_hover_cursor(CursorIcon::ResizeHorizontal);
                ui.painter()
                    .rect_filled(split, 0.0, Color32::from_rgb(28, 28, 30));
                x += gap;
            }
        }
        if let Some(from) = self.pane_drag {
            if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary)) {
                self.pane_drag = None;
                if let Some(pos) = ui.input(|input| input.pointer.interact_pos()) {
                    if let Some(to) = rects.iter().position(|rect| rect.contains(pos)) {
                        if to != from {
                            self.panes.swap(from, to);
                        }
                    }
                }
            }
        }
    }

    fn ortho_pane(&mut self, ui: &mut egui::Ui, index: usize, rect: Rect) {
        let header =
            Rect::from_min_max(rect.left_top(), Pos2::new(rect.right(), rect.top() + 26.0));
        let body = Rect::from_min_max(Pos2::new(rect.left(), header.bottom()), rect.right_bottom());
        let header_id = Id::new(("pane-header", index));
        let header_response = ui.interact(header, header_id, Sense::drag());
        if header_response.drag_started_by(egui::PointerButton::Primary) {
            self.pane_drag = Some(index);
        }
        header_response.on_hover_cursor(CursorIcon::Grab);
        let fill = if self.pane_drag == Some(index) {
            Color32::from_rgb(48, 72, 82)
        } else {
            Color32::from_rgb(32, 32, 36)
        };
        ui.painter().rect_filled(header, 0.0, fill);
        ui.painter().text(
            header.left_center() + Vec2::new(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            ortho_label(self.panes[index].kind),
            egui::FontId::proportional(13.0),
            Color32::from_rgb(220, 220, 224),
        );

        let response = ui.interact(body, Id::new(("pane-body", index)), Sense::click_and_drag());
        let (middle, origin, delta, scroll, hover) = ui.input(|input| {
            (
                input.pointer.button_down(egui::PointerButton::Middle),
                input.pointer.press_origin(),
                input.pointer.delta(),
                input.raw_scroll_delta.y,
                input.pointer.hover_pos(),
            )
        });
        let panning = !self.camera.noclip && middle && origin.is_some_and(|pos| body.contains(pos));
        if panning && delta != Vec2::ZERO {
            let (right, up) = self.panes[index].axes();
            let scale = self.panes[index].span * 2.0 / body.height().max(1.0);
            self.panes[index].target -= (right * delta.x + up * delta.y) * scale;
        }
        let response = if panning {
            response.on_hover_cursor(CursorIcon::AllScroll)
        } else {
            response
        };
        if hover.is_some_and(|pos| body.contains(pos)) && scroll != 0.0 {
            let zoom = (-scroll * 0.0016_f32).exp();
            self.panes[index].span = (self.panes[index].span * zoom).clamp(40.0, 20_000.0);
        }
        let aspect = aspect_of(ui, body);
        let view_proj = self.panes[index].view_proj(aspect);
        let eye = self.panes[index].eye();
        ui.painter()
            .rect_filled(body, 0.0, Color32::from_rgb(14, 14, 16));
        self.steer_lamp(ui, &response, body, view_proj, eye);
        self.paint_scene(ui, body, index + 1, view_proj);
        draw_view_grid(ui.painter(), body, view_proj, &self.panes[index]);
    }

    fn paint_scene(&mut self, ui: &egui::Ui, rect: Rect, slot: usize, view_proj: [[f32; 4]; 4]) {
            let pixels = ui.ctx().pixels_per_point();
            let width = (rect.width() * pixels).round().max(1.0) as u32;
            let height = (rect.height() * pixels).round().max(1.0) as u32;
        let (positions, position_id) = self.positions();
        let (colors, color_id) = self.shown_colors();
        let (marker_positions, marker_colors, marker_id) = self.markers();
            ui.painter()
                .add(eframe::egui_wgpu::Callback::new_paint_callback(
                    rect,
                    gpu::RoomCallback {
                    positions,
                    position_id,
                    colors,
                    color_id,
                    marker_positions,
                    marker_colors,
                    marker_id,
                        view_proj,
                        width,
                        height,
                    slot,
                    },
                ));
    }

    fn inspectors(&mut self, ctx: &egui::Context) {
        let Some(index) = self.selected else {
            return;
        };
        if index >= self.objects.len() {
            self.selected = None;
            return;
        }
        let mut object = self.objects[index].clone();
        let categories = self.categories.clone();
        let mut draft = std::mem::take(&mut self.draft_category);
        let mut preview = false;
        let mut commit = false;
        let mut add_category = false;
        let editing = self.editing();

        let screen = ctx.screen_rect();
        // Object sits against the top-right corner; Light fills the gap on its left.
        let top = screen.top() + 48.0;
        let margin = 16.0;
        let gap = 12.0;
        let frame = 14.0;
        let object_inner = 360.0;
        let object_outer = object_inner + frame;
        egui::Window::new("Объект")
            .id(Id::new("object-panel"))
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(Pos2::new(screen.right() - margin, top))
            .default_width(object_inner)
            .default_height(32.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_enabled_ui(editing, |ui| {
                    ui.label("Название");
                    ui.text_edit_singleline(&mut object.name);
                    ui.label("Категория");
                    egui::ComboBox::from_id_salt("object-category")
                        .selected_text(if object.category.is_empty() {
                            "—"
                        } else {
                            object.category.as_str()
                        })
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(object.category.is_empty(), "—")
                                .clicked()
                            {
                                object.category.clear();
                            }
                            for category in &categories {
                                if ui
                                    .selectable_label(&object.category == category, category)
                                    .clicked()
                                {
                                    object.category.clone_from(category);
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut draft);
                        if ui.button("Добавить").clicked() {
                            add_category = true;
                        }
                    });
                    ui.separator();
                    ui.label("Пространство");
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(object.space == Space::Plane, "2D")
                            .clicked()
                            && object.space != Space::Plane
                        {
                            object.space = Space::Plane;
                            object.custom = true;
                            commit = true;
                        }
                        if ui
                            .selectable_label(object.space == Space::Solid, "3D")
                            .clicked()
                            && object.space != Space::Solid
                        {
                            object.space = Space::Solid;
                            object.custom = true;
                            commit = true;
                        }
                    });
                    ui.label("Грани");
                    note_shape(
                        axis(ui, &mut object.size.x, "X ", 1.0),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    note_shape(
                        axis(ui, &mut object.size.y, "Y ", 1.0),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    if object.space == Space::Solid {
                        note_shape(
                            axis(ui, &mut object.size.z, "Z ", 1.0),
                            &mut object,
                            &mut preview,
                            &mut commit,
                        );
                    }
                    object.size = object.size.max(Vec3::splat(1.0));
                    ui.label("Положение");
                    note_move(
                        axis(ui, &mut object.position.x, "X ", 1.0),
                        &mut preview,
                        &mut commit,
                    );
                    note_move(
                        axis(ui, &mut object.position.y, "Y ", 1.0),
                        &mut preview,
                        &mut commit,
                    );
                    note_move(
                        axis(ui, &mut object.position.z, "Z ", 1.0),
                        &mut preview,
                        &mut commit,
                    );
                    ui.label("Поворот");
                    note_shape(
                        axis(ui, &mut object.rotation.x, "Рыскание ", 1.0),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    note_shape(
                        axis(ui, &mut object.rotation.y, "Тангаж ", 1.0),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    note_shape(
                        axis(ui, &mut object.rotation.z, "Крен ", 1.0),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    let corners =
                        ui.add(egui::Slider::new(&mut object.corners, 0.0..=100.0).text("Углы"));
                    ui.label(
                        RichText::new(if object.space == Space::Plane {
                            "0 — круг, 100 — квадрат"
                        } else {
                            "0 — шар, 100 — куб"
                        })
                        .size(12.0)
                        .color(Color32::from_rgb(150, 150, 156)),
                    );
                    note_shape(
                        Edit {
                            preview: corners.dragged(),
                            commit: released(&corners),
                        },
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    object.corners = object.corners.clamp(0.0, 100.0);
                });
            });

        egui::Window::new("Свет")
            .id(Id::new("light-panel"))
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(Pos2::new(screen.right() - margin - object_outer - gap, top))
            .default_width(132.0)
            .default_height(32.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_enabled_ui(editing, |ui| {
                    ui.label("Сила и цвет площадки");
                    note_shape(
                        axis(ui, &mut object.intensity, "", 100.0),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    object.intensity = object.intensity.max(0.0);
                    let mut rgb = object.color.to_array();
                    ui.horizontal(|ui| {
                        let edited = ui.color_edit_button_rgb(&mut rgb);
                        if edited.changed() {
                            object.color = Vec3::from_array(rgb);
                            object.custom = true;
                            if edited.dragged() {
                                preview = true;
                            } else {
                                commit = true;
                            }
                        }
                    });
                    note_shape(
                        axis(ui, &mut object.color.x, "R ", 0.01),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    note_shape(
                        axis(ui, &mut object.color.y, "G ", 0.01),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    note_shape(
                        axis(ui, &mut object.color.z, "B ", 0.01),
                        &mut object,
                        &mut preview,
                        &mut commit,
                    );
                    object.color = object.color.clamp(Vec3::ZERO, Vec3::ONE);
                });
            });

        if add_category {
            let name = draft.trim().to_owned();
            if !name.is_empty() && !self.categories.iter().any(|item| item == &name) {
                self.categories.push(name.clone());
            }
            if !name.is_empty() {
                object.category = name;
            }
            draft.clear();
        }
        let changed = object != self.objects[index] || self.categories != categories;
        if changed && !self.undo_group {
            self.remember();
        }
        if preview && !commit {
            self.undo_group = true;
        } else if changed && !preview && !commit {
            self.undo_group = true;
        } else if commit || !ctx.wants_keyboard_input() {
            self.undo_group = false;
        }
        self.draft_category = draft;
        self.objects[index] = object;
        if commit {
            self.lamps_moved();
        } else if preview {
            self.preview_lamp();
        }
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        if !self.export_open {
            return;
        }
        let mut open = self.export_open;
        egui::Window::new("Экспорт в BSP")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Файл появится рядом с картой, с суффиксом _light.");
                ui.label("Куда записать");
                ui.text_edit_singleline(&mut self.export_path);
                if ui.button("Обзор…").clicked() {
                    let name = Path::new(self.export_path.as_str())
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("map_light.bsp");
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("BSP", &["bsp"])
                        .set_file_name(name)
                        .save_file()
                    {
                        self.export_path = ensure_ext(path, "bsp").display().to_string();
                    }
                }
                let ready = self.bake_rx.is_none()
                    && self.solve_rx.is_none()
                    && !self.path.as_os_str().is_empty()
                    && !self.export_path.trim().is_empty();
                if ui
                    .add_enabled(ready, egui::Button::new("Экспортировать"))
                    .clicked()
                {
                    self.start_bake();
                }
                if self.path.as_os_str().is_empty() {
                    ui.label("Сначала импортируйте карту.");
                }
                if self.bake_rx.is_some() {
                    let done = self.bake_done.load(Ordering::Relaxed);
                    let units = self.bake_units.max(1);
                    let fraction = (done as f32 / units as f32).clamp(0.0, 1.0);
                    let label = if done >= self.bake_units && self.bake_units > 0 {
                        "Запись".to_owned()
                    } else {
                        format!("{}%", (fraction * 100.0).round() as u32)
                    };
                    ui.add(egui::ProgressBar::new(fraction).text(label));
                    ui.label(eta_text(self.export_started, done, self.bake_units));
                }
                if let Some(note) = &self.bake_note {
                    if self.bake_failed {
                        ui.colored_label(Color32::from_rgb(214, 96, 78), note);
                    } else {
                        ui.label(note);
                    }
                }
            });
        self.export_open = open;
    }

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Проект", &["lbr", "lightbaker"])
            .pick_file()
        else {
            return;
        };
        match read_project(&path) {
            Ok((map, categories, objects)) => {
                self.project_path = Some(path);
                self.categories = categories;
                if !self.categories.iter().any(|item| item == RELAPSE) {
                    self.categories.insert(0, RELAPSE.to_owned());
                }
                self.objects = objects;
                self.selected = None;
                self.undo.clear();
                self.redo.clear();
                self.undo_group = false;
                if map.is_file() {
                    self.begin_open(map);
                } else if !map.as_os_str().is_empty() {
                    self.error = Some(format!("Карта не найдена: {}", map.display()));
                }
            }
            Err(err) => self.error = Some(err),
        }
    }

    fn import_map(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Карта", &["bsp"])
            .pick_file()
        else {
            return;
        };
        self.begin_open(path);
    }

    fn save_project(&mut self) {
        let Some(path) = self.project_path.clone() else {
            self.save_project_as();
            return;
        };
        self.write_project_file(&path);
    }

    fn save_project_as(&mut self) {
        let name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or("проект");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Проект", &["lbr"])
            .set_file_name(format!("{name}.lbr"))
            .save_file()
        else {
            return;
        };
        let path = ensure_ext(path, "lbr");
        self.project_path = Some(path.clone());
        self.write_project_file(&path);
    }

    fn write_project_file(&mut self, path: &Path) {
        if let Err(err) = write_project(path, &self.path, &self.categories, &self.objects) {
            self.error = Some(err);
        }
    }

    fn open_export(&mut self) {
        if !self.path.as_os_str().is_empty() {
            self.export_path = beside(&self.path).display().to_string();
        }
        self.export_open = true;
    }

    fn add_object(&mut self, mut object: LightObject) {
        self.remember();
        self.undo_group = false;
        object.name = unique_name(&self.objects, &object.name);
        self.objects.push(object);
        self.selected = Some(self.objects.len() - 1);
        self.preview_lamp();
    }

    fn delete_object(&mut self, index: usize) {
        if !self.editing() || index >= self.objects.len() {
            return;
        }
        self.remember();
        self.undo_group = false;
        self.objects.remove(index);
        self.selected = match self.selected {
            Some(selected) if selected == index => None,
            Some(selected) if selected > index => Some(selected - 1),
            other => other,
        };
        self.object_menu = match self.object_menu {
            Some((menu, _)) if menu == index => None,
            Some((menu, pos)) if menu > index => Some((menu - 1, pos)),
            other => other,
        };
        self.dragging_lamp = false;
        self.lamp_moved = false;
        self.preview_lamp();
    }

    fn revision(&self) -> Revision {
        Revision {
            objects: self.objects.clone(),
            categories: self.categories.clone(),
            selected: self.selected,
        }
    }

    fn remember(&mut self) {
        self.undo.push(self.revision());
        if self.undo.len() > 80 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo_edit(&mut self) {
        let Some(previous) = self.undo.pop() else {
            return;
        };
        self.redo.push(self.revision());
        self.restore(previous);
    }

    fn redo_edit(&mut self) {
        let Some(next) = self.redo.pop() else {
            return;
        };
        self.undo.push(self.revision());
        self.restore(next);
    }

    fn restore(&mut self, revision: Revision) {
        let light = revision.objects.len() != self.objects.len()
            || revision
                .objects
                .iter()
                .zip(&self.objects)
                .any(|(next, current)| !same_emission(next, current));
        self.objects = revision.objects;
        self.categories = revision.categories;
        self.selected = revision
            .selected
            .filter(|index| *index < self.objects.len());
        self.object_menu = None;
        self.undo_group = false;
        self.dragging_lamp = false;
        self.lamp_moved = false;
        if light {
            self.lamps_moved();
        }
    }

    fn object_actions(&mut self, ctx: &egui::Context) {
        let Some((index, pos)) = self.object_menu else {
            return;
        };
        if index >= self.objects.len() {
            self.object_menu = None;
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.object_menu = None;
            return;
        }
        let name = self.objects[index].name.clone();
        let editing = self.editing();
        let mut delete = false;
        let area = egui::Area::new(Id::new("object-actions"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(168.0);
                    ui.label(
                        RichText::new(name)
                            .size(12.0)
                            .color(Color32::from_rgb(160, 160, 168)),
                    );
                    if ui
                        .add_enabled(editing, egui::Button::new("Удалить"))
                        .clicked()
                    {
                        delete = true;
                    }
                });
            });
        let opening = ctx.input(|input| {
            input
                .pointer
                .button_released(egui::PointerButton::Secondary)
        });
        let outside = ctx.input(|input| input.pointer.any_pressed()) && !area.response.hovered();
        if delete {
            self.object_menu = None;
            self.delete_object(index);
        } else if outside && !opening {
            self.object_menu = None;
        }
    }
}

fn matrix_ray_at(
    view_proj: [[f32; 4]; 4],
    eye: Vec3,
    rect: egui::Rect,
    pos: egui::Pos2,
) -> Option<(Vec3, Vec3)> {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return None;
    }
    let x = ((pos.x - rect.min.x) / rect.width()) * 2.0 - 1.0;
    let y = 1.0 - ((pos.y - rect.min.y) / rect.height()) * 2.0;
    matrix_ray(view_proj, eye, x, y)
}

fn matrix_ray(view_proj: [[f32; 4]; 4], eye: Vec3, ndc_x: f32, ndc_y: f32) -> Option<(Vec3, Vec3)> {
    let inv = Mat4::from_cols_array_2d(&view_proj).inverse();
    let far = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
    if !far.w.is_finite() || far.w.abs() < 1.0e-6 {
        return None;
    }
    let far = far.truncate() / far.w;
    let dir = (far - eye).normalize_or_zero();
    if dir.length_squared() < 1.0e-8 {
        None
    } else {
        Some((eye, dir))
    }
}

fn ndc_ray(camera: &Camera, aspect: f32, ndc_x: f32, ndc_y: f32) -> Option<(Vec3, Vec3)> {
    matrix_ray(camera.view_proj(aspect), camera.eye(), ndc_x, ndc_y)
}

fn aspect_of(ui: &egui::Ui, rect: Rect) -> f32 {
    let pixels = ui.ctx().pixels_per_point();
    let width = (rect.width() * pixels).round().max(1.0);
    let height = (rect.height() * pixels).round().max(1.0);
    width / height
}

struct Edit {
    preview: bool,
    commit: bool,
}

fn axis(ui: &mut egui::Ui, value: &mut f32, prefix: &str, speed: f32) -> Edit {
    let response = ui.add(egui::DragValue::new(value).speed(speed).prefix(prefix));
    Edit {
        preview: response.dragged(),
        commit: released(&response),
    }
}

fn note_shape(edit: Edit, object: &mut LightObject, preview: &mut bool, commit: &mut bool) {
    if edit.preview || edit.commit {
        object.custom = true;
    }
    *preview |= edit.preview;
    *commit |= edit.commit;
}

fn note_move(edit: Edit, preview: &mut bool, commit: &mut bool) {
    *preview |= edit.preview;
    *commit |= edit.commit;
}

fn flat_widgets(ui: &mut egui::Ui) {
    ui.style_mut().spacing.button_padding = Vec2::new(10.0, 4.0);
    let text = Stroke::new(1.0_f32, Color32::from_rgb(226, 226, 230));
    let visuals = ui.visuals_mut();
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = text;
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 48, 54);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(48, 48, 54);
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.fg_stroke = text;
    visuals.widgets.active.weak_bg_fill = Color32::from_rgb(60, 60, 68);
    visuals.widgets.active.bg_fill = Color32::from_rgb(60, 60, 68);
    visuals.widgets.active.bg_stroke = Stroke::NONE;
    visuals.widgets.active.fg_stroke = text;
    visuals.widgets.open.weak_bg_fill = Color32::from_rgb(48, 48, 54);
    visuals.widgets.open.bg_fill = Color32::from_rgb(48, 48, 54);
    visuals.widgets.open.bg_stroke = Stroke::NONE;
    visuals.widgets.open.fg_stroke = text;
}

fn menu_line(ui: &mut egui::Ui, title: &str, shortcut: &str) -> bool {
    ui.horizontal(|ui| {
        let clicked = ui.button(title).clicked();
        ui.add_space(16.0);
        ui.label(RichText::new(shortcut).color(Color32::from_rgb(140, 140, 148)));
        clicked
    })
    .inner
}

enum Caption {
    Min,
    Max,
    Restore,
    Close,
}

fn caption(ui: &mut egui::Ui, rect: Rect, kind: Caption) -> Response {
    let id = match kind {
        Caption::Min => Id::new("cap-min"),
        Caption::Max => Id::new("cap-max"),
        Caption::Restore => Id::new("cap-restore"),
        Caption::Close => Id::new("cap-close"),
    };
    let response = ui.interact(rect, id, Sense::click());
    let fill = if response.hovered() && matches!(kind, Caption::Close) {
        Color32::from_rgb(196, 43, 28)
    } else if response.hovered() {
        Color32::from_rgb(58, 58, 62)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    let color = Color32::from_rgb(220, 220, 224);
    let center = rect.center();
    let stroke = Stroke::new(1.0_f32, color);
    match kind {
        Caption::Min => {
            ui.painter().line_segment(
                [
                    Pos2::new(center.x - 5.0, center.y),
                    Pos2::new(center.x + 5.0, center.y),
                ],
                stroke,
            );
        }
        Caption::Max => {
            ui.painter().rect_stroke(
                Rect::from_center_size(center, Vec2::splat(10.0)),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Caption::Restore => {
            let back = Rect::from_min_size(center + Vec2::new(-2.0, -6.0), Vec2::splat(8.0));
            let front = Rect::from_min_size(center + Vec2::new(-6.0, -2.0), Vec2::splat(8.0));
            ui.painter()
                .rect_stroke(back, 0.0, stroke, egui::StrokeKind::Inside);
            ui.painter()
                .rect_filled(front, 0.0, Color32::from_rgb(24, 24, 26));
            ui.painter()
                .rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
        }
        Caption::Close => {
            ui.painter().line_segment(
                [center + Vec2::new(-4.5, -4.5), center + Vec2::new(4.5, 4.5)],
                stroke,
            );
            ui.painter().line_segment(
                [center + Vec2::new(4.5, -4.5), center + Vec2::new(-4.5, 4.5)],
                stroke,
            );
        }
    }
    response
}

fn same_emission(left: &LightObject, right: &LightObject) -> bool {
    left.space == right.space
        && left.size == right.size
        && left.position == right.position
        && left.rotation == right.rotation
        && left.corners == right.corners
        && left.intensity == right.intensity
        && left.color == right.color
        && left.template == right.template
        && left.custom == right.custom
}

fn unique_name(objects: &[LightObject], base: &str) -> String {
    if !objects.iter().any(|object| object.name == base) {
        return base.to_owned();
    }
    let mut number = 2;
    loop {
        let candidate = format!("{base} {number}");
        if !objects.iter().any(|object| object.name == candidate) {
            return candidate;
        }
        number += 1;
    }
}

fn ensure_ext(mut path: PathBuf, ext: &str) -> PathBuf {
    if path
        .extension()
        .and_then(|item| item.to_str())
        .is_some_and(|item| item.eq_ignore_ascii_case(ext))
    {
        return path;
    }
    let name = path
        .file_name()
        .and_then(|item| item.to_str())
        .unwrap_or("file")
        .to_owned();
    path.set_file_name(format!("{name}.{ext}"));
    path
}

fn draw_view_grid(painter: &egui::Painter, rect: Rect, view_proj: [[f32; 4]; 4], pane: &Pane) {
    if rect.width() < 8.0 || rect.height() < 8.0 {
        return;
    }
    let view = Mat4::from_cols_array_2d(&view_proj);
    let (right, up) = pane.axes();
    let half_h = pane.span.max(20.0);
    let half_w = half_h * (rect.width() / rect.height()).max(0.1);
    let step = grid_step(half_h * 2.0 / rect.height());
    let painter = painter.with_clip_rect(rect);
    let minor = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(186, 196, 208, 78));
    let major = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(214, 222, 232, 150));
    let plane = pane.target;
    grid_lines(
        &painter,
        view,
        rect,
        plane,
        right,
        up,
        plane.dot(right),
        half_w,
        half_h,
        step,
        minor,
        major,
    );
    grid_lines(
        &painter,
        view,
        rect,
        plane,
        up,
        right,
        plane.dot(up),
        half_h,
        half_w,
        step,
        minor,
        major,
    );
}

fn grid_lines(
    painter: &egui::Painter,
    view: Mat4,
    rect: Rect,
    plane: Vec3,
    axis: Vec3,
    other: Vec3,
    origin: f32,
    half_axis: f32,
    half_other: f32,
    step: f32,
    minor: Stroke,
    major: Stroke,
) {
    let start = ((origin - half_axis) / step).floor() as i32 - 1;
    let end = ((origin + half_axis) / step).ceil() as i32 + 1;
    for index in start..=end {
        let delta = index as f32 * step - origin;
        let a = plane + axis * delta - other * (half_other + step);
        let b = plane + axis * delta + other * (half_other + step);
        let Some(from) = project_point(view, rect, a) else {
            continue;
        };
        let Some(to) = project_point(view, rect, b) else {
            continue;
        };
        let stroke = if index.rem_euclid(8) == 0 {
            major
        } else {
            minor
        };
        painter.line_segment([from, to], stroke);
    }
}

fn project_point(view: Mat4, rect: Rect, world: Vec3) -> Option<Pos2> {
    let clip = view * world.extend(1.0);
    if !clip.w.is_finite() || clip.w <= 1.0e-4 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() || !(0.0..=1.0).contains(&ndc.z) {
        return None;
    }
    Some(Pos2::new(
        rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
        rect.top() + (0.5 - ndc.y * 0.5) * rect.height(),
    ))
}

fn grid_step(world_per_pixel: f32) -> f32 {
    let target = (world_per_pixel * 28.0).max(1.0);
    2.0_f32.powf(target.log2().round()).clamp(1.0, 8192.0)
}

fn eta_text(started: Option<Instant>, done: u64, units: u64) -> String {
    let Some(started) = started else {
        return "Считаю время…".to_owned();
    };
    if units == 0 || done == 0 {
        return "Считаю время…".to_owned();
    }
    let fraction = done as f32 / units as f32;
    if fraction < 0.02 {
        return "Считаю время…".to_owned();
    }
    let remain = started.elapsed().as_secs_f32() * (1.0 - fraction) / fraction;
    let seconds = remain.round().max(0.0) as u32;
    if seconds < 60 {
        format!("Осталось примерно {seconds} с")
    } else {
        format!("Осталось примерно {} мин {} с", seconds / 60, seconds % 60)
    }
}

/// World-axis half extents of the patch. A flat patch has a zero on its thin axis.
fn hitbox_half(area: Area) -> Vec3 {
    match area {
        Area::Rectangle(rectangle) => {
            let mut half = Vec3::ZERO;
            for corner in rectangle.corners() {
                half = half.max((corner - rectangle.center).abs());
            }
            half
        }
        Area::Disk(disk) => {
            let (axis, bitangent) = disk.frame();
            let mut half = Vec3::ZERO;
            for step in 0..16 {
                let angle = step as f32 / 16.0 * std::f32::consts::TAU;
                let rim = (axis * angle.cos() + bitangent * angle.sin()) * disk.radius;
                half = half.max(rim.abs());
            }
            half
        }
    }
}

fn selection_half(half: Vec3) -> Vec3 {
    half.max(Vec3::splat(0.8)) + Vec3::splat(1.2)
}

fn pick_area(origin: Vec3, dir: Vec3, areas: &[Area]) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (index, area) in areas.iter().enumerate() {
        let half = hitbox_half(*area).max(Vec3::splat(PICK_PAD));
        let Some(distance) = ray_box(origin, dir, area.center(), half) else {
            continue;
        };
        if best.is_none_or(|(so_far, _)| distance < so_far) {
            best = Some((distance, index));
        }
    }
    best.map(|(_, index)| index)
}

fn nearest_surface(origin: Vec3, dir: Vec3, triangles: &[Triangle]) -> Option<(Vec3, Vec3)> {
    let mut best: Option<(f32, Vec3)> = None;
    for triangle in triangles {
        let Some((distance, normal)) = ray_triangle(origin, dir, triangle.vertices) else {
            continue;
        };
        if best.is_none_or(|(so_far, _)| distance < so_far) {
            best = Some((distance, normal));
        }
    }
    best.map(|(distance, normal)| (origin + dir * distance, normal))
}

/// Puts the hitbox against the face. `normal` points out of the surface, toward the lamp.
fn seat_on(hit: Vec3, normal: Vec3, half: Vec3) -> Vec3 {
    let normal = normal.normalize_or_zero();
    if normal == Vec3::ZERO {
        return hit;
    }
    let support = half.x * normal.x.abs() + half.y * normal.y.abs() + half.z * normal.z.abs();
    hit + normal * (support + SEAT_GAP)
}

fn ray_box(origin: Vec3, dir: Vec3, center: Vec3, half: Vec3) -> Option<f32> {
    let min = center - half;
    let max = center + half;
    let mut enter = 0.0f32;
    let mut leave = f32::INFINITY;
    for axis in 0..3 {
        let start = origin[axis];
        let step = dir[axis];
        if step.abs() < 1.0e-8 {
            if start < min[axis] || start > max[axis] {
                return None;
            }
            continue;
        }
        let mut t1 = (min[axis] - start) / step;
        let mut t2 = (max[axis] - start) / step;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        enter = enter.max(t1);
        leave = leave.min(t2);
        if enter > leave {
            return None;
        }
    }
    if leave < 0.0 {
        return None;
    }
    Some(if enter >= 0.0 { enter } else { 0.0 })
}

/// Closest hit. The returned normal faces back along the ray, toward the camera.
fn ray_triangle(origin: Vec3, dir: Vec3, corners: [Vec3; 3]) -> Option<(f32, Vec3)> {
    let edge_u = corners[1] - corners[0];
    let edge_v = corners[2] - corners[0];
    let p = dir.cross(edge_v);
    let det = edge_u.dot(p);
    if det.abs() < 1.0e-6 {
        return None;
    }
    let inv = 1.0 / det;
    let offset = origin - corners[0];
    let u = offset.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = offset.cross(edge_u);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = edge_v.dot(q) * inv;
    if distance < 1.0e-4 {
        return None;
    }
    let mut normal = edge_u.cross(edge_v).normalize_or_zero();
    if normal.dot(dir) > 0.0 {
        normal = -normal;
    }
    Some((distance, normal))
}

fn push_box_frame(
    positions: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 3]>,
    center: Vec3,
    half: Vec3,
    color: [f32; 3],
) {
    let min = center - half;
    let max = center + half;
    let corner = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for (from, to) in edges {
        push_beam(positions, colors, corner[from], corner[to], 1.2, color);
    }
}

fn push_beam(
    positions: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 3]>,
    from: Vec3,
    to: Vec3,
    radius: f32,
    color: [f32; 3],
) {
    let dir = (to - from).normalize_or_zero();
    if dir == Vec3::ZERO {
        return;
    }
    let helper = if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let side = dir.cross(helper).normalize_or_zero() * radius;
    let up = dir.cross(side).normalize_or_zero() * radius;
    let ring = [
        from + side,
        from + up,
        from - side,
        from - up,
        to + side,
        to + up,
        to - side,
        to - up,
    ];
    let quads = [(0, 4, 5, 1), (1, 5, 6, 2), (2, 6, 7, 3), (3, 7, 4, 0)];
    for (a, b, c, d) in quads {
        push_quad_positions(positions, [ring[a], ring[b], ring[c], ring[d]]);
        push_repeat(colors, color, 6);
    }
}

fn noclip_hint(ui: &egui::Ui, rect: egui::Rect) {
    let text = "V — выйти из ноклипа";
    let font = egui::FontId::proportional(15.0);
    let color = egui::Color32::from_rgb(236, 236, 236);
    let galley = ui
        .ctx()
        .fonts(|fonts| fonts.layout_no_wrap(text.to_owned(), font, color));
    let pad = egui::vec2(10.0, 6.0);
    let size = galley.size() + pad * 2.0;
    let bg = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.bottom() - 18.0 - size.y * 0.5),
        size,
    );
    ui.painter()
        .rect_filled(bg, 4.0, egui::Color32::from_black_alpha(180));
    ui.painter()
        .galley(bg.min + pad, galley, egui::Color32::from_rgb(236, 236, 236));
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

/// Reflectivity is a bounce fraction. Grass sits near 0.06, which is darker
/// than the empty view, so the lawn reads as a hole. Encode it for the screen.
fn surface_color(albedo: Vec3) -> [f32; 3] {
    albedo
        .clamp(Vec3::ZERO, Vec3::ONE)
        .powf(1.0 / 2.2)
        .to_array()
}

fn push_quad_positions(positions: &mut Vec<[f32; 3]>, corners: [Vec3; 4]) {
    push_triangle_positions(positions, [corners[0], corners[1], corners[2]]);
    push_triangle_positions(positions, [corners[0], corners[2], corners[3]]);
}

fn push_repeat(colors: &mut Vec<[f32; 3]>, color: [f32; 3], count: usize) {
    for _ in 0..count {
        colors.push(color);
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use glam::Vec3;
    use solve::{Area, Rectangle, Triangle};

    use super::{
        hitbox_half, ndc_ray, nearest_surface, pick_area, seat_on, surface_color, Camera,
        FLY_BOOST, SEAT_GAP,
    };

    fn camera(yaw: f32) -> Camera {
        Camera {
            yaw,
            pitch: 0.0,
            distance: 100.0,
            target: Vec3::ZERO,
            noclip: false,
        }
    }

    #[test]
    fn grass_reflectivity_stays_lighter_than_the_empty_view() {
        let color = surface_color(Vec3::new(0.061, 0.077, 0.020));
        assert!(
            color.iter().all(|channel| *channel > 0.12),
            "{color:?}"
        );
    }

    #[test]
    fn wasd_slides_across_the_floor_toward_the_view() {
        let speed = 120.0;
        let dt = 0.05;
        let mut view = camera(0.0);
        assert!(view.translate(1.0, 0.0, 0.0, dt, false));
        assert!((view.target - Vec3::new(0.0, -speed * dt, 0.0)).length() < 1.0e-3);

        let mut view = camera(0.0);
        view.translate(0.0, 1.0, 0.0, dt, false);
        assert!((view.target - Vec3::new(-speed * dt, 0.0, 0.0)).length() < 1.0e-3);

        let mut view = camera(0.0);
        view.translate(1.0, 1.0, 0.0, 1.0, false);
        assert!((view.target.length() - speed * 0.1).abs() < 1.0e-3);
        assert_eq!(view.target.z, 0.0);

        let mut view = camera(0.0);
        assert!(!view.translate(0.0, 0.0, 0.0, 1.0, false));
        assert_eq!(view.target, Vec3::ZERO);

        let mut grounded = camera(0.0);
        grounded.pitch = 1.0;
        grounded.translate(1.0, 0.0, 1.0, dt, false);
        assert_eq!(grounded.target.z, 0.0);

        let mut flying = camera(0.0);
        flying.pitch = 1.0;
        flying.noclip = true;
        flying.translate(1.0, 0.0, 0.0, dt, false);
        assert!(flying.target.z < 0.0);

        let mut up = camera(0.0);
        up.noclip = true;
        up.translate(0.0, 0.0, 1.0, dt, false);
        assert!((up.target.z - speed * dt).abs() < 1.0e-3);

        let mut down = camera(0.0);
        down.noclip = true;
        down.translate(0.0, 0.0, -1.0, dt, false);
        assert!((down.target.z + speed * dt).abs() < 1.0e-3);

        let mut slow = camera(0.0);
        slow.noclip = true;
        let mut boosted = camera(0.0);
        boosted.noclip = true;
        slow.translate(1.0, 0.0, 0.0, dt, false);
        boosted.translate(1.0, 0.0, 0.0, dt, true);
        assert!((boosted.target.length() / slow.target.length() - FLY_BOOST).abs() < 1.0e-3);
    }

    #[test]
    fn middle_orbits_and_right_looks_in_place() {
        let mut orbiting = camera(0.4);
        let before = orbiting.eye();
        orbiting.orbit(egui::vec2(10.0, 0.0));
        assert_eq!(orbiting.target, Vec3::ZERO);
        assert!((orbiting.eye() - before).length() > 1.0);

        let mut looking = camera(0.4);
        let eye = looking.eye();
        looking.look(egui::vec2(10.0, 4.0));
        assert!((looking.eye() - eye).length() < 1.0e-3);
        assert!(looking.yaw > 0.4);
        assert!(looking.pitch > 0.0);
    }

    fn slab(center: Vec3) -> Area {
        Area::Rectangle(Rectangle {
            center,
            half_u: Vec3::X * 36.0,
            half_v: Vec3::Y * 8.0,
            normal: -Vec3::Z,
            intensity: 1.0,
            color: Vec3::ONE,
        })
    }

    #[test]
    fn a_lamp_sits_on_the_wall_by_its_hitbox() {
        let half = hitbox_half(slab(Vec3::ZERO));
        assert!((half - Vec3::new(36.0, 8.0, 0.0)).length() < 1.0e-3);

        let wall = [
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 2.0, 0.0),
            Vec3::new(10.0, 0.0, 2.0),
        ];
        let (hit, normal) = nearest_surface(
            Vec3::new(0.0, 0.2, 0.2),
            Vec3::X,
            &[Triangle { vertices: wall }],
        )
        .expect("wall");
        assert!(normal.x < 0.0);
        let placed = seat_on(hit, normal, half);
        assert!((placed.x - (10.0 - half.x - SEAT_GAP)).abs() < 1.0e-3);
        assert!((placed.y - 0.2).abs() < 1.0e-2);
    }

    #[test]
    fn a_double_click_ray_picks_the_near_lamp_and_misses_empty_space() {
        let areas = [
            slab(Vec3::new(0.0, 0.0, 40.0)),
            slab(Vec3::new(0.0, 0.0, 20.0)),
        ];
        assert_eq!(pick_area(Vec3::ZERO, Vec3::Z, &areas), Some(1));
        assert_eq!(pick_area(Vec3::new(100.0, 0.0, 0.0), Vec3::Z, &areas), None);
    }

    #[test]
    fn the_ortho_views_keep_the_target_on_screen() {
        use glam::Mat4;
        for kind in [
            super::OrthoKind::Top,
            super::OrthoKind::Front,
            super::OrthoKind::Side,
        ] {
            let pane = super::Pane {
                kind,
                target: Vec3::new(120.0, -80.0, 64.0),
                span: 900.0,
            };
            let clip = Mat4::from_cols_array_2d(&pane.view_proj(1.6)) * pane.target.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!(
                ndc.x.abs() < 0.05 && ndc.y.abs() < 0.05 && (0.0..1.0).contains(&ndc.z),
                "{kind:?} ndc {ndc:?} w {}",
                clip.w
            );
            let (right, up) = pane.axes();
            let nearby = pane.target + right * 100.0 + up * 50.0;
            let near_clip = Mat4::from_cols_array_2d(&pane.view_proj(1.6)) * nearby.extend(1.0);
            let near_ndc = near_clip.truncate() / near_clip.w;
            assert!(
                near_ndc.x.abs() < 1.0
                    && near_ndc.y.abs() < 1.0
                    && (0.0..1.0).contains(&near_ndc.z),
                "{kind:?} nearby {near_ndc:?}"
            );
        }
    }

    #[test]
    fn the_center_of_the_view_looks_at_the_orbit_point() {
        let (origin, dir) = ndc_ray(&camera(0.0), 1.0, 0.0, 0.0).expect("ray");
        assert!((origin - Vec3::new(0.0, 100.0, 0.0)).length() < 1.0e-2);
        assert!(dir.y < -0.5);
        assert!(dir.x.abs() < 0.2);
        assert!(dir.z.abs() < 0.2);
    }
}

fn push_triangle_positions(positions: &mut Vec<[f32; 3]>, corners: [Vec3; 3]) {
    for corner in corners {
        positions.push(corner.to_array());
    }
}
