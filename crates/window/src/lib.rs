mod base;
mod gpu;
mod project;

use std::collections::HashMap;
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
    apply_base, base_label, capture_base, kind_label, read_lights, read_project, relapse_fixtures,
    write_lights, write_project, Base, DoorChoice, DoorState, LightObject, Space, RELAPSE,
};
use solve::{
    broad_light, dynamic, solve_reporting, Area, Cover, Receiver, Role, Triangle, RAY_PASSES,
};

const PREVIEW_RAYS: u32 = 16;
const LOOK_SPEED: f32 = 0.0025;
const FLY_BOOST: f32 = 3.0;
const PICK_PAD: f32 = 4.0;
const SEAT_GAP: f32 = 1.0;
const SELECTION: [f32; 3] = [0.35, 0.92, 1.0];

/// `choose` opens the map list. Otherwise the map comes from the launch argument.
pub fn run(choose: bool) -> eframe::Result {
    let options = eframe::NativeOptions {
        // Decorations are off so the custom bar owns minimize, maximize, and close.
        // A fixed inner size fights maximize, so only a minimum size is set.
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_maximized(true)
            .with_resizable(true)
            .with_drag_and_drop(true)
            .with_min_inner_size([960.0, 640.0]),
        // wgpu ignores `vsync` and waits on the swapchain instead. AutoNoVsync
        // prefers an immediate present, so the view is not locked to the display.
        vsync: false,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            present_mode: eframe::egui_wgpu::wgpu::PresentMode::AutoNoVsync,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "Source Lightbaker",
        options,
        Box::new(move |cc| Ok(Box::new(Lightbaker::new(cc, choose)))),
    )
}

fn load_map(path: PathBuf, progress: &AtomicU64, stage: &AtomicU8) -> Result<Opened, String> {
    let publish = |phase: map::LoadPhase, done: u64, total: u64| {
        let (start, end) = match phase {
            map::LoadPhase::File => (0.0, 0.08),
            map::LoadPhase::World => (0.08, 0.22),
            map::LoadPhase::Textures => (0.22, 0.70),
            map::LoadPhase::Packs => (0.70, 0.86),
            map::LoadPhase::Props => (0.86, 0.96),
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
    stage.store(5, Ordering::Relaxed);
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
        4 => "Читаю текстуры",
        _ => "Считаю свет",
    }
}

fn is_project(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("lbr") || ext.eq_ignore_ascii_case("lightbaker")
        })
}

fn is_lights(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lbrl"))
}

#[derive(Debug)]
enum Launch {
    Project(PathBuf),
    Map(PathBuf),
    None,
}

fn launch_from_args() -> Launch {
    launch_from(std::env::args().skip(1))
}

fn launch_from<I, S>(args: I) -> Launch
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut map = None;
    for arg in args {
        let text = arg.as_ref().trim().trim_matches('"');
        if text.is_empty() || text.starts_with('-') {
            continue;
        }
        let path = PathBuf::from(text);
        if is_lights(&path) {
            continue;
        }
        if is_project(&path) {
            return Launch::Project(path);
        }
        if map.is_none() {
            map = Some(path);
        }
    }
    map.map(Launch::Map).unwrap_or(Launch::None)
}

fn browse_start() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|profile| profile.join("Desktop"))
        .filter(|path| path.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Double-click on a .lbr file should start this program and pass the path.
#[cfg(windows)]
fn register_project_type() {
    use std::os::windows::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let command = format!("\"{}\" \"%1\"", exe.display());
    let write = |key: &str, value: &str| {
        let _ = std::process::Command::new("reg")
            .args(["add", key, "/ve", "/d", value, "/f"])
            .creation_flags(0x0800_0000)
            .status();
    };
    write(r"HKCU\Software\Classes\.lbr", "Lightbaker.Project");
    write(
        r"HKCU\Software\Classes\Lightbaker.Project",
        "Проект Lightbaker",
    );
    write(
        r"HKCU\Software\Classes\Lightbaker.Project\shell\open\command",
        &command,
    );
    unsafe {
        SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null());
    }
}

#[cfg(windows)]
#[link(name = "shell32")]
extern "system" {
    fn SHChangeNotify(
        event_id: u32,
        flags: u32,
        item1: *const std::ffi::c_void,
        item2: *const std::ffi::c_void,
    );
}

#[cfg(not(windows))]
fn register_project_type() {}

fn fallback_map() -> Result<PathBuf, String> {
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

/// The path stored in a project, or the same file name beside the project and
/// in the game's map folders. Relapse lives under `download/maps`, not `maps`.
fn resolve_map(stored: &Path, project: &Path) -> Result<PathBuf, String> {
    if stored.is_file() {
        return Ok(stored.to_path_buf());
    }
    let mut names = Vec::new();
    if let Some(name) = stored.file_name() {
        if !name.is_empty() {
            names.push(name.to_os_string());
            if Path::new(name).extension().is_none() {
                let mut with_bsp = name.to_os_string();
                with_bsp.push(".bsp");
                names.push(with_bsp);
            }
        }
    }
    let root = PathBuf::from(r"D:\Steam\steamapps\common\GarrysMod\garrysmod");
    let mut dirs = Vec::new();
    if let Some(parent) = project.parent() {
        dirs.push(parent.to_path_buf());
    }
    if let Some(parent) = stored
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        dirs.push(parent.to_path_buf());
    }
    dirs.push(root.join("maps"));
    dirs.push(root.join("download").join("maps"));
    for dir in &dirs {
        for name in &names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    if stored.as_os_str().is_empty() {
        Err("В проекте не указана карта".into())
    } else {
        Err(format!("Карта не найдена: {}", stored.display()))
    }
}

fn listed_maps() -> Vec<ListedMap> {
    let root = PathBuf::from(r"D:\Steam\steamapps\common\GarrysMod\garrysmod");
    let mut maps = Vec::new();
    push_maps(&root.join("maps"), "maps", &mut maps);
    push_maps(&root.join("download").join("maps"), "загрузки", &mut maps);
    maps.sort_by(|left, right| {
        left.title
            .to_ascii_lowercase()
            .cmp(&right.title.to_ascii_lowercase())
            .then_with(|| left.place.cmp(right.place))
    });
    maps
}

fn push_maps(dir: &Path, place: &'static str, maps: &mut Vec<ListedMap>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.to_ascii_lowercase().ends_with(".bsp") {
            continue;
        }
        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(name)
            .to_string();
        maps.push(ListedMap { path, title, place });
    }
}

fn map_megabytes(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|meta| meta.len() / (1024 * 1024))
        .unwrap_or(0)
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
    selected: Vec<usize>,
    door_states: Vec<DoorState>,
    selected_door: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picture {
    Dynamic,
    Real,
}

#[derive(Clone, Copy)]
enum Aim {
    Lamp(usize),
    Door(usize),
}

struct PlacedDoor {
    id: String,
    name: String,
    state: DoorState,
    closed_triangles: Vec<solve::Triangle>,
    open_triangles: Vec<solve::Triangle>,
    closed_surface: Vec<map::Surface>,
    open_surface: Vec<map::Surface>,
    closed_bounds: (Vec3, Vec3),
    open_bounds: (Vec3, Vec3),
}

impl PlacedDoor {
    fn from_map(door: map::Door) -> Self {
        let closed_bounds = mesh_bounds(&door.closed.triangles);
        let open_bounds = mesh_bounds(&door.open.triangles);
        Self {
            id: door.id,
            name: door.name,
            state: DoorState::Closed,
            closed_triangles: door.closed.triangles,
            open_triangles: door.open.triangles,
            closed_surface: door.closed.surface,
            open_surface: door.open.surface,
            closed_bounds,
            open_bounds,
        }
    }

    /// Triangles that block light, when the leaf is still in the solve.
    fn blocking(&self) -> Option<&[solve::Triangle]> {
        match self.state {
            DoorState::Closed => Some(&self.closed_triangles),
            DoorState::Open => Some(&self.open_triangles),
            DoorState::Gone => None,
        }
    }

    fn pose(&self) -> Option<(&[solve::Triangle], &[map::Surface])> {
        match self.state {
            DoorState::Closed => Some((&self.closed_triangles, &self.closed_surface)),
            DoorState::Open => Some((&self.open_triangles, &self.open_surface)),
            DoorState::Gone => None,
        }
    }

    fn shown_bounds(&self) -> (Vec3, Vec3) {
        match self.state {
            DoorState::Open => self.open_bounds,
            DoorState::Closed | DoorState::Gone => self.closed_bounds,
        }
    }
}

fn door_state_name(state: DoorState) -> &'static str {
    match state {
        DoorState::Closed => "закрыта",
        DoorState::Open => "открыта",
        DoorState::Gone => "убрана",
    }
}

fn mesh_bounds(triangles: &[solve::Triangle]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let mut any = false;
    for triangle in triangles {
        for point in triangle.vertices {
            if !point.is_finite() {
                continue;
            }
            min = min.min(point);
            max = max.max(point);
            any = true;
        }
    }
    if any {
        (min, max)
    } else {
        (Vec3::ZERO, Vec3::ZERO)
    }
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
    /// World without door leaves. Door state is added back in [`Lightbaker::compose_geometry`].
    world_triangles: Arc<Vec<solve::Triangle>>,
    world_surface: Vec<map::Surface>,
    materials: Arc<Vec<map::Material>>,
    images: Arc<Vec<map::Image>>,
    doors: Vec<PlacedDoor>,
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
    map_light: bool,
    show_shapes: bool,
    generation: u64,
    dynamic_colors: Vec<[f32; 3]>,
    /// Shadow rays for every dynamic light. Rebuilt when the map loads.
    cover: Arc<Cover>,
    /// Map-load shadow pass. The map is shown before this finishes.
    shadow_rx: Option<mpsc::Receiver<ShadowPass>>,
    /// One visibility row per light that has already been traced.
    shades: Vec<Shade>,
    traced: Vec<[f32; 3]>,
    traced_generation: u64,
    colors: Vec<[f32; 3]>,
    sealed: Vec<usize>,
    solves: u32,
    solve_ms: u128,
    solve_rx: Option<mpsc::Receiver<Done>>,
    solve_done: Arc<AtomicU64>,
    solve_units: u64,
    scene: Arc<gpu::Scene>,
    scene_id: u64,
    atlas: LightAtlas,
    light_pixels: Vec<u16>,
    lightmap: Arc<Vec<u16>>,
    lightmap_size: u32,
    light_id: u64,
    plain_lights: Arc<Vec<[f32; 4]>>,
    tex_lights: Arc<Vec<[f32; 4]>>,
    marker_positions: Option<Arc<Vec<[f32; 3]>>>,
    marker_colors: Option<Arc<Vec<[f32; 3]>>>,
    marker_id: u64,
    camera: Camera,
    cursor_held: bool,
    selected: Vec<usize>,
    /// List row a Shift-click ranges from.
    select_anchor: Option<usize>,
    selected_door: Option<usize>,
    door_menu: Option<(usize, Pos2)>,
    door_plan: Vec<DoorChoice>,
    door_plan_map: Option<PathBuf>,
    object_menu: Option<(usize, Pos2)>,
    undo: Vec<Revision>,
    redo: Vec<Revision>,
    undo_group: bool,
    /// Objects copied with Ctrl+C. Paste steps the group aside together.
    clipboard: Vec<LightObject>,
    paste_times: u32,
    /// Held state of the copy and paste keys, so each chord fires once.
    command_copy: bool,
    command_paste: bool,
    dragging_lamp: bool,
    /// Selected object the pointer grabbed.
    drag_index: Option<usize>,
    lamp_moved: bool,
    snapshot: Option<Arc<map::Snapshot>>,
    /// Start on the map list instead of a path given at launch.
    choose: bool,
    maps: Vec<ListedMap>,
    map_query: String,
    opening: Option<Opening>,
    bake_rx: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    bake_done: Arc<AtomicU64>,
    bake_units: u64,
    bake_note: Option<String>,
    bake_failed: bool,
}

struct ListedMap {
    path: PathBuf,
    title: String,
    place: &'static str,
}

#[derive(Clone, Copy, PartialEq)]
struct ShadeKey {
    center: Vec3,
    normal: Vec3,
    half_u: Vec3,
    half_v: Vec3,
    radius: f32,
}

struct Shade {
    key: ShadeKey,
    seen: Vec<f32>,
}

struct ShadowPass {
    generation: u64,
    areas: Vec<Area>,
    seen: Vec<f32>,
}

fn visibility_column(seen: &[f32], receivers: usize, areas: usize, area: usize) -> Vec<f32> {
    (0..receivers)
        .map(|receiver| seen[receiver * areas + area])
        .collect()
}

fn add_light(colors: &mut [[f32; 3]], extra: &[[f32; 3]]) {
    for (slot, add) in colors.iter_mut().zip(extra) {
        slot[0] += add[0];
        slot[1] += add[1];
        slot[2] += add[2];
    }
}

fn shade_key(area: &Area) -> ShadeKey {
    match area {
        Area::Rectangle(rectangle) => ShadeKey {
            center: rectangle.center,
            normal: rectangle.normal,
            half_u: rectangle.half_u,
            half_v: rectangle.half_v,
            radius: 0.0,
        },
        Area::Disk(disk) => ShadeKey {
            center: disk.center,
            normal: disk.normal,
            half_u: Vec3::ZERO,
            half_v: Vec3::ZERO,
            radius: disk.radius,
        },
        Area::Volume(volume) => ShadeKey {
            center: volume.center,
            normal: volume.axis_x,
            half_u: volume.axis_y,
            half_v: volume.axis_z,
            radius: 1.0,
        },
        Area::Omni(omni) => ShadeKey {
            center: omni.center,
            normal: omni.axis_x,
            half_u: omni.axis_y,
            half_v: omni.axis_z,
            radius: 2.0,
        },
    }
}

impl Lightbaker {
    fn new(cc: &eframe::CreationContext<'_>, choose: bool) -> Self {
        gpu::init(cc);
        let mut app = Self {
            error: None,
            path: PathBuf::new(),
            triangles_note: String::new(),
            luxels: Vec::new(),
            receivers: Arc::new(Vec::new()),
            triangles: Arc::new(Vec::new()),
            surface: Vec::new(),
            world_triangles: Arc::new(Vec::new()),
            world_surface: Vec::new(),
            materials: Arc::new(Vec::new()),
            images: Arc::new(Vec::new()),
            doors: Vec::new(),
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
            map_light: true,
            show_shapes: true,
            generation: 0,
            dynamic_colors: Vec::new(),
            cover: Arc::new(Cover::new(&[])),
            shadow_rx: None,
            shades: Vec::new(),
            traced: Vec::new(),
            traced_generation: 0,
            colors: Vec::new(),
            sealed: Vec::new(),
            solves: 0,
            solve_ms: 0,
            solve_rx: None,
            solve_done: Arc::new(AtomicU64::new(0)),
            solve_units: 0,
            scene: Arc::new(gpu::Scene::empty()),
            scene_id: 0,
            atlas: LightAtlas::blank(),
            light_pixels: vec![0; 32 * 32 * 4],
            lightmap: Arc::new(vec![0; 32 * 32 * 4]),
            lightmap_size: 32,
            light_id: 0,
            plain_lights: Arc::new(Vec::new()),
            tex_lights: Arc::new(Vec::new()),
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
            selected: Vec::new(),
            select_anchor: None,
            selected_door: None,
            door_menu: None,
            door_plan: Vec::new(),
            door_plan_map: None,
            object_menu: None,
            undo: Vec::new(),
            redo: Vec::new(),
            undo_group: false,
            clipboard: Vec::new(),
            paste_times: 0,
            command_copy: false,
            command_paste: false,
            dragging_lamp: false,
            drag_index: None,
            lamp_moved: false,
            snapshot: None,
            choose,
            maps: if choose { listed_maps() } else { Vec::new() },
            map_query: String::new(),
            opening: None,
            bake_rx: None,
            bake_done: Arc::new(AtomicU64::new(0)),
            bake_units: 0,
            bake_note: None,
            bake_failed: false,
        };
        match launch_from_args() {
            Launch::Project(path) => app.open_project_file(&path),
            Launch::Map(path) => app.begin_open(path),
            Launch::None if !choose => match fallback_map() {
                Ok(path) => app.begin_open(path),
                Err(err) => app.error = Some(err),
            },
            Launch::None => {}
        }
        if choose {
            register_project_type();
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
        self.receivers = Arc::new(opened.luxels.iter().map(|luxel| luxel.receiver).collect());
        self.world_triangles = Arc::new(opened.triangles);
        self.world_surface = opened.surface;
        self.materials = Arc::new(opened.materials);
        self.images = Arc::new(opened.images);
        self.doors = opened.doors.into_iter().map(PlacedDoor::from_map).collect();
        self.apply_door_plan(&path);
        self.atlas = pack_faces(&atlas_faces(&opened.snapshot));
        self.lightmap_size = self.atlas.size;
        self.light_pixels = vec![0; self.atlas.texels()];
        self.compose_geometry();
        self.shades.clear();
        self.selected_door = None;
        self.door_menu = None;
        self.triangles_note = format!(
            "{} · {} люкселей · {} треугольников · {} дверей",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("карта"),
            opened.luxels.len(),
            self.triangles.len(),
            self.doors.len(),
        );
        self.snapshot = Some(Arc::new(opened.snapshot));
        self.luxels = opened.luxels;
        self.picture = Picture::Dynamic;
        self.generation = self.generation.wrapping_add(1);
        self.traced.clear();
        self.traced_generation = 0;
        // A project can hold every fixture. Tracing them here would hold the
        // loading screen until the last ray, so the map is shown first.
        self.dynamic_colors = vec![[0.0, 0.0, 0.0]; self.receivers.len()];
        self.present();
        self.trace_shadows();
        self.sealed.clear();
        self.marker_positions = None;
        self.marker_colors = None;
        self.selected.retain(|index| *index < self.objects.len());
        if self
            .select_anchor
            .is_some_and(|index| index >= self.objects.len())
        {
            self.select_anchor = self.selected.first().copied();
        }
        self.dragging_lamp = false;
        self.drag_index = None;
        self.lamp_moved = false;
        self.path = path;
        self.bake_note = None;
        self.bake_failed = false;
        self.undo.clear();
        self.redo.clear();
        self.undo_group = false;
    }

    fn apply_door_plan(&mut self, path: &Path) {
        let plan = if self
            .door_plan_map
            .as_ref()
            .is_some_and(|planned| planned == path)
        {
            std::mem::take(&mut self.door_plan)
        } else {
            Vec::new()
        };
        self.door_plan_map = None;
        for choice in plan {
            if let Some(door) = self.doors.iter_mut().find(|door| door.id == choice.id) {
                door.state = choice.state;
            }
        }
    }

    fn compose_geometry(&mut self) {
        let mut triangles = (*self.world_triangles).clone();
        let mut surface = self.world_surface.clone();
        for door in &self.doors {
            if let Some((extra, faces)) = door.pose() {
                triangles.extend_from_slice(extra);
                surface.extend_from_slice(faces);
            }
        }
        self.triangles = Arc::new(triangles);
        self.cover = Arc::new(Cover::new(&self.triangles));
        self.surface = surface;
        self.scene = Arc::new(build_scene(
            &self.surface,
            &self.materials,
            Arc::clone(&self.images),
            &self.atlas,
        ));
        self.scene_id = self.scene_id.wrapping_add(1);
    }

    fn doors_changed(&mut self) {
        self.compose_geometry();
        self.generation = self.generation.wrapping_add(1);
        self.shades.clear();
        self.sealed.clear();
        self.traced.clear();
        self.traced_generation = 0;
        self.dynamic_colors = vec![[0.0; 3]; self.receivers.len()];
        self.present();
        self.trace_shadows();
        if self.picture == Picture::Real && self.solve_rx.is_none() {
            self.resolve();
        }
    }

    fn set_door_state(&mut self, index: usize, state: DoorState) {
        if !self.editing() {
            return;
        }
        let Some(door) = self.doors.get(index) else {
            return;
        };
        if door.state == state {
            return;
        }
        self.remember();
        self.undo_group = false;
        self.doors[index].state = state;
        self.doors_changed();
    }

    fn remove_selected_door(&mut self) {
        let Some(index) = self.selected_door else {
            return;
        };
        self.set_door_state(index, DoorState::Gone);
    }

    fn door_choices(&self) -> Vec<DoorChoice> {
        self.doors
            .iter()
            .map(|door| DoorChoice {
                id: door.id.clone(),
                state: door.state,
            })
            .collect()
    }

    fn door_inspector(&mut self, ctx: &egui::Context) {
        let Some(index) = self.selected_door else {
            return;
        };
        if index >= self.doors.len() {
            self.selected_door = None;
            return;
        }
        let name = self.doors[index].name.clone();
        let state = self.doors[index].state;
        let editing = self.editing();
        let mut next = None;
        let screen = ctx.screen_rect();
        egui::Window::new("Дверь")
            .id(Id::new("door-panel"))
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(Pos2::new(screen.right() - 16.0, screen.top() + 48.0))
            .default_width(280.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new(name).strong());
                ui.label(format!("Сейчас {}", door_state_name(state)));
                ui.label("Свет считается с этой створкой. В карте дверь остаётся.");
                ui.add_enabled_ui(editing, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(state != DoorState::Open, egui::Button::new("Открыть"))
                            .clicked()
                        {
                            next = Some(DoorState::Open);
                        }
                        if ui
                            .add_enabled(state != DoorState::Closed, egui::Button::new("Закрыть"))
                            .clicked()
                        {
                            next = Some(DoorState::Closed);
                        }
                    });
                    if ui
                        .add_enabled(
                            state != DoorState::Gone,
                            egui::Button::new("Убрать из расчёта"),
                        )
                        .clicked()
                    {
                        next = Some(DoorState::Gone);
                    }
                });
            });
        if let Some(state) = next {
            self.set_door_state(index, state);
        }
    }

    fn door_actions(&mut self, ctx: &egui::Context) {
        let Some((index, pos)) = self.door_menu else {
            return;
        };
        if index >= self.doors.len() {
            self.door_menu = None;
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.door_menu = None;
            return;
        }
        let state = self.doors[index].state;
        let editing = self.editing();
        let mut next = None;
        let area = egui::Area::new(Id::new("door-actions"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(180.0);
                    if ui
                        .add_enabled(
                            editing && state != DoorState::Open,
                            egui::Button::new("Открыть"),
                        )
                        .clicked()
                    {
                        next = Some(DoorState::Open);
                    }
                    if ui
                        .add_enabled(
                            editing && state != DoorState::Closed,
                            egui::Button::new("Закрыть"),
                        )
                        .clicked()
                    {
                        next = Some(DoorState::Closed);
                    }
                    if ui
                        .add_enabled(
                            editing && state != DoorState::Gone,
                            egui::Button::new("Убрать из расчёта"),
                        )
                        .clicked()
                    {
                        next = Some(DoorState::Gone);
                    }
                });
            });
        let opening = ctx.input(|input| {
            input
                .pointer
                .button_released(egui::PointerButton::Secondary)
        });
        let outside = ctx.input(|input| input.pointer.any_pressed()) && !area.response.hovered();
        if let Some(state) = next {
            self.door_menu = None;
            self.set_door_state(index, state);
        } else if outside && !opening {
            self.door_menu = None;
        }
    }

    fn areas(&self) -> Vec<Area> {
        self.objects.iter().flat_map(LightObject::areas).collect()
    }

    fn lamps_moved(&mut self) {
        self.apply_dynamic(true);
        if self.picture == Picture::Real && self.solve_rx.is_none() {
            self.resolve();
        }
    }

    fn preview_lamp(&mut self) {
        self.apply_dynamic(false);
    }

    fn apply_dynamic(&mut self, trace: bool) {
        self.generation = self.generation.wrapping_add(1);
        self.sealed.clear();
        self.dynamic_colors = self.compose(trace);
        self.present();
    }

    /// Every light stops at a wall. Dragging one leaves the others shadowed
    /// and lets that one shine through until the pointer is released.
    fn compose(&mut self, trace: bool) -> Vec<[f32; 3]> {
        let areas = self.areas();
        let count = self.receivers.len();
        let mut colors = vec![[0.0, 0.0, 0.0]; count];
        let mut used = vec![false; self.shades.len()];
        let mut missing = Vec::new();
        for area in &areas {
            let key = shade_key(area);
            let found = self.shades.iter().enumerate().position(|(index, shade)| {
                !used[index] && shade.key == key && shade.seen.len() == count
            });
            if let Some(index) = found {
                used[index] = true;
                add_light(
                    &mut colors,
                    &broad_light(&self.receivers, &[*area], &self.shades[index].seen),
                );
            } else {
                missing.push(*area);
            }
        }
        if !missing.is_empty() && trace {
            let seen = self.cover.see(&self.receivers, &missing);
            add_light(&mut colors, &broad_light(&self.receivers, &missing, &seen));
            // `seen` is one row per receiver. A later edit looks each area up
            // on its own, so store that area's column, not a cut of the rows.
            for (index, area) in missing.iter().enumerate() {
                self.shades.push(Shade {
                    key: shade_key(area),
                    seen: visibility_column(&seen, count, missing.len(), index),
                });
                used.push(true);
            }
        } else if !missing.is_empty() {
            add_light(&mut colors, &dynamic(&self.receivers, &missing));
        }
        let mut kept = Vec::new();
        for (index, shade) in self.shades.drain(..).enumerate() {
            if used.get(index).copied().unwrap_or(false) {
                kept.push(shade);
            }
        }
        self.shades = kept;
        colors
    }

    fn trace_shadows(&mut self) {
        self.shadow_rx = None;
        let areas = self.areas();
        if areas.is_empty() || self.receivers.is_empty() {
            return;
        }
        let cover = Arc::clone(&self.cover);
        let receivers = Arc::clone(&self.receivers);
        let generation = self.generation;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let seen = cover.see(&receivers, &areas);
            let _ = tx.send(ShadowPass {
                generation,
                areas,
                seen,
            });
        });
        self.shadow_rx = Some(rx);
    }

    fn poll_shadows(&mut self) {
        let message = self.shadow_rx.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(pass) => Some(Ok(pass)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(())),
        });
        let Some(message) = message else {
            return;
        };
        self.shadow_rx = None;
        let Ok(pass) = message else {
            return;
        };
        if pass.generation != self.generation {
            return;
        }
        let count = self.receivers.len();
        let areas = pass.areas.len();
        if areas == 0 || pass.seen.len() != count * areas {
            return;
        }
        let mut colors = vec![[0.0, 0.0, 0.0]; count];
        add_light(
            &mut colors,
            &broad_light(&self.receivers, &pass.areas, &pass.seen),
        );
        self.shades.clear();
        for (index, area) in pass.areas.iter().enumerate() {
            self.shades.push(Shade {
                key: shade_key(area),
                seen: visibility_column(&pass.seen, count, areas, index),
            });
        }
        self.dynamic_colors = colors;
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
        fill_lightmap(&self.atlas, &self.colors, &mut self.light_pixels);
        self.lightmap = Arc::new(self.light_pixels.clone());
        self.plain_lights = Arc::new(vertex_lights(
            &self.scene.plain_luxels,
            &self.scene.plain_props,
            &self.colors,
        ));
        self.tex_lights = Arc::new(vertex_lights(
            &self.scene.tex_luxels,
            &self.scene.tex_props,
            &self.colors,
        ));
        self.light_id = self.light_id.wrapping_add(1);
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
                input.key_pressed(egui::Key::V) && !input.modifiers.command,
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

    /// Double-click selects one object. Ctrl-click adds or removes one.
    /// A click outside the selection clears it. Dragging moves the whole group.
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
        let aimed = pos.and_then(|pos| self.aim_under(rect, view_proj, eye, pos));
        let command = ui.input(|input| input.modifiers.command);
        if response.double_clicked_by(primary) {
            self.finish_lamp_drag();
            match aimed {
                Some(Aim::Lamp(index)) => self.select_only(index),
                Some(Aim::Door(index)) => self.select_door(index),
                None if !command => self.clear_selection(),
                None => {}
            }
            self.object_menu = None;
            self.door_menu = None;
        } else if response.secondary_clicked() {
            match aimed {
                Some(Aim::Lamp(index)) => {
                    if !self.selected.contains(&index) {
                        self.select_only(index);
                    }
                    let at = response.interact_pointer_pos().unwrap_or(Pos2::ZERO);
                    self.object_menu = Some((index, at));
                    self.door_menu = None;
                }
                Some(Aim::Door(index)) => {
                    self.select_door(index);
                    let at = response.interact_pointer_pos().unwrap_or(Pos2::ZERO);
                    self.door_menu = Some((index, at));
                }
                None => {}
            }
        } else if command && response.clicked_by(primary) {
            if let Some(Aim::Lamp(index)) = aimed {
                self.selected_door = None;
                self.door_menu = None;
                self.toggle_selected(index);
            } else if let Some(Aim::Door(index)) = aimed {
                self.select_door(index);
            }
            self.object_menu = None;
        } else if response.clicked_by(primary) {
            if let Some(Aim::Door(index)) = aimed {
                if self.selected_door != Some(index) {
                    self.select_door(index);
                }
                self.object_menu = None;
                self.door_menu = None;
            } else if !self.aim_keeps(aimed) {
                self.clear_selection();
                self.object_menu = None;
                self.door_menu = None;
            }
        }
        if response.drag_started_by(primary) && self.editing() {
            let pressed = ui
                .input(|input| input.pointer.press_origin())
                .and_then(|pos| self.aim_under(rect, view_proj, eye, pos));
            self.drag_index = match pressed {
                Some(Aim::Lamp(index)) if self.selected.contains(&index) => Some(index),
                _ => None,
            };
            self.dragging_lamp = self.drag_index.is_some();
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
        self.drag_index = None;
        if self.lamp_moved {
            self.lamp_moved = false;
            self.apply_dynamic(true);
            if self.picture == Picture::Real && self.solve_rx.is_none() {
                self.resolve();
            }
        }
    }

    fn aim_under(
        &self,
        rect: egui::Rect,
        view_proj: [[f32; 4]; 4],
        eye: Vec3,
        pos: egui::Pos2,
    ) -> Option<Aim> {
        let (origin, dir) = matrix_ray_at(view_proj, eye, rect, pos)?;
        self.aim_at(origin, dir)
    }

    fn aim_at(&self, origin: Vec3, dir: Vec3) -> Option<Aim> {
        let lamp = self.lamp_distance(origin, dir);
        let door = self.door_distance(origin, dir);
        match (lamp, door) {
            (Some((lamp_distance, lamp)), Some((door_distance, door))) => {
                if door_distance < lamp_distance {
                    Some(Aim::Door(door))
                } else {
                    Some(Aim::Lamp(lamp))
                }
            }
            (Some((_, index)), None) => Some(Aim::Lamp(index)),
            (None, Some((_, index))) => Some(Aim::Door(index)),
            (None, None) => None,
        }
    }

    fn aim_keeps(&self, aimed: Option<Aim>) -> bool {
        match aimed {
            Some(Aim::Lamp(index)) => self.selected.contains(&index),
            Some(Aim::Door(index)) => self.selected_door == Some(index),
            None => false,
        }
    }

    fn lamp_distance(&self, origin: Vec3, dir: Vec3) -> Option<(f32, usize)> {
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
        best
    }

    fn door_distance(&self, origin: Vec3, dir: Vec3) -> Option<(f32, usize)> {
        let mut best: Option<(f32, usize)> = None;
        for (index, door) in self.doors.iter().enumerate() {
            let Some(triangles) = door.blocking() else {
                continue;
            };
            let (min, max) = door.shown_bounds();
            let center = (min + max) * 0.5;
            let half = (max - min) * 0.5;
            if ray_box(origin, dir, center, half.max(Vec3::splat(1.0))).is_none() {
                continue;
            }
            for triangle in triangles {
                let Some((distance, _)) = ray_triangle(origin, dir, triangle.vertices) else {
                    continue;
                };
                if distance <= 0.0 {
                    continue;
                }
                if best.is_none_or(|(so_far, _)| distance < so_far) {
                    best = Some((distance, index));
                }
            }
        }
        best
    }

    fn drag_lamp(
        &mut self,
        rect: egui::Rect,
        view_proj: [[f32; 4]; 4],
        eye: Vec3,
        pos: egui::Pos2,
    ) {
        let Some(index) = self.drag_index else {
            return;
        };
        if index >= self.objects.len() {
            return;
        }
        let Some((origin, dir)) = matrix_ray_at(view_proj, eye, rect, pos) else {
            return;
        };
        let Some((hit, normal)) = nearest_surface(origin, dir, &self.triangles) else {
            return;
        };
        let (min, max) = self.objects[index].bounds();
        let placed = seat_on(hit, normal, (max - min) * 0.5);
        let delta = placed - self.objects[index].position;
        if delta.length_squared() < 1.0e-6 {
            return;
        }
        if !self.lamp_moved {
            self.remember();
            self.undo_group = false;
        }
        for item in self.selected.clone() {
            if let Some(object) = self.objects.get_mut(item) {
                object.position += delta;
            }
        }
        self.preview_lamp();
        self.lamp_moved = true;
    }

    fn markers(&mut self) -> (Arc<Vec<[f32; 3]>>, Arc<Vec<[f32; 3]>>, u64) {
        let mut positions = Vec::new();
        let mut colors = Vec::new();
        let mut cursor = 0usize;
        for (index, object) in self.objects.iter().enumerate() {
            let chosen = self.selected.contains(&index);
            if self.show_shapes {
                let count = object.areas().len();
                let sealed = self.showing_real()
                    && (cursor..cursor + count).any(|area| self.sealed.contains(&area));
                cursor += count;
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
            }
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
        if let Some(index) = self.selected_door {
            if let Some(door) = self.doors.get(index) {
                let (min, max) = door.shown_bounds();
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
        self.poll_shadows();
        self.take_drop(ctx);
        self.shortcuts(ctx);
        self.chrome(ctx);
        if self.opening.is_some() {
            draw_loading(ctx, self.opening.as_ref().expect("opening"));
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        self.poll_bake();
        self.drive_camera(ctx);
        if self.bake_rx.is_some() || self.shadow_rx.is_some() {
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

        if self.choose && self.luxels.is_empty() {
            self.draw_picker(ctx);
            self.export_window(ctx);
            self.object_actions(ctx);
            return;
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
            self.door_inspector(ctx);
        }
        self.export_window(ctx);
        self.object_actions(ctx);
        self.door_actions(ctx);
    }
}

impl Lightbaker {
    fn draw_picker(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space((ui.available_height() * 0.08).max(12.0));
            ui.vertical_centered(|ui| {
                ui.set_max_width(640.0);
                ui.heading("Выберите карту");
                ui.label("Обзор показывает карты и проекты .lbr.");
                if let Some(error) = &self.error {
                    ui.colored_label(Color32::from_rgb(214, 96, 78), error);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Обзор…").clicked() {
                        self.import_map();
                    }
                    if ui.button("Проект…").clicked() {
                        self.open_project();
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.map_query)
                            .hint_text("Найти")
                            .desired_width(280.0),
                    );
                });
                if let Some(error) = &self.error {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(214, 96, 78), error);
                }
                ui.add_space(12.0);
                let query = self.map_query.to_ascii_lowercase();
                let shown: Vec<usize> = self
                    .maps
                    .iter()
                    .enumerate()
                    .filter(|(_, map)| {
                        query.is_empty()
                            || map.title.to_ascii_lowercase().contains(&query)
                            || map.place.to_ascii_lowercase().contains(&query)
                    })
                    .map(|(index, _)| index)
                    .collect();
                if self.maps.is_empty() {
                    ui.label("В папках maps и download нет BSP. Укажите файл через Обзор.");
                    return;
                }
                if shown.is_empty() {
                    ui.label("Ничего не найдено.");
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 24.0).max(120.0))
                    .show(ui, |ui| {
                        for index in shown {
                            let title = self.maps[index].title.clone();
                            let place = self.maps[index].place;
                            let path = self.maps[index].path.clone();
                            let size = map_megabytes(&path);
                            let label = format!("{title}    {place}    {size} МБ");
                            if ui
                                .add_sized([640.0, 32.0], egui::Button::new(label))
                                .clicked()
                            {
                                self.begin_open(path);
                            }
                        }
                    });
            });
        });
    }

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
        // egui turns Ctrl+C / Ctrl+V into clipboard events and drops the key,
        // and on a Russian layout those keys never arrive as C and V.
        let (copy_key, paste_key) = command_chord();
        let focused = ctx.input(|input| input.focused);
        let copy_edge = focused && copy_key && !self.command_copy;
        let paste_edge = focused && paste_key && !self.command_paste;
        self.command_copy = copy_key;
        self.command_paste = paste_key;
        if !ctx.wants_keyboard_input() {
            let (copy_event, paste_event, delete) = ctx.input_mut(|input| {
                let mut copy = false;
                let mut paste = false;
                input.events.retain(|event| match event {
                    egui::Event::Copy => {
                        copy = true;
                        false
                    }
                    egui::Event::Paste(_) => {
                        paste = true;
                        false
                    }
                    _ => true,
                });
                let delete = input.consume_key(egui::Modifiers::NONE, egui::Key::Delete);
                (copy, paste, delete)
            });
            if copy_event || copy_edge {
                self.copy_object();
            }
            if paste_event || paste_edge {
                self.paste_object();
            }
            if delete {
                if self.selected_door.is_some() {
                    self.remove_selected_door();
                } else {
                    self.delete_selected();
                }
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
                        Vec2::new(210.0, 28.0),
                    )),
                    |ui| {
                        flat_widgets(ui);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let _ = ui.menu_button("Файл", |ui| {
                                ui.set_min_width(260.0);
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
                                if ui.button("Сохранить источники…").clicked() {
                                    self.save_lights();
                                }
                                if ui.button("Импорт источников…").clicked() {
                                    self.import_lights();
                                }
                                ui.separator();
                                if ui.button("Экспорт в .bsp").clicked() {
                                    self.open_export();
                                }
                            });
                            let _ = ui.menu_button("Правка", |ui| {
                                ui.set_min_width(240.0);
                                if menu_command(
                                    ui,
                                    "Копировать",
                                    "Ctrl+C",
                                    !self.selected.is_empty(),
                                ) {
                                    self.copy_object();
                                }
                                if menu_command(
                                    ui,
                                    "Вставить",
                                    "Ctrl+V",
                                    !self.clipboard.is_empty() && self.editing(),
                                ) {
                                    self.paste_object();
                                }
                                if menu_command(
                                    ui,
                                    "Удалить",
                                    "Delete",
                                    self.editing()
                                        && (!self.selected.is_empty()
                                            || self.selected_door.is_some()),
                                ) {
                                    if self.selected_door.is_some() {
                                        self.remove_selected_door();
                                    } else {
                                        self.delete_selected();
                                    }
                                }
                            });
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
            .take(8)
            .collect();
        let door_hits: Vec<usize> = self
            .doors
            .iter()
            .enumerate()
            .filter(|(_, door)| door.name.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .take(8)
            .collect();
        if hits.is_empty() && door_hits.is_empty() {
            return;
        }
        let mut pick = None;
        let mut pick_door = None;
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
                                .selectable_label(self.selected.contains(&index), label)
                                .clicked()
                            {
                                pick = Some(index);
                            }
                        }
                        for index in door_hits {
                            let label = format!(
                                "{} · {}",
                                self.doors[index].name,
                                door_state_name(self.doors[index].state)
                            );
                            if ui
                                .selectable_label(self.selected_door == Some(index), label)
                                .clicked()
                            {
                                pick_door = Some(index);
                            }
                        }
                    });
            });
        if let Some(index) = pick_door {
            self.select_door(index);
            self.search.clear();
        } else if let Some(index) = pick {
            self.select_only(index);
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
                ui.checkbox(&mut self.map_light, "Освещение карты");
                ui.checkbox(&mut self.show_shapes, "Включить отображение фигур света");
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
                if !self.doors.is_empty() {
                    ui.label("Двери");
                    let door_height = (self.doors.len() as f32 * 22.0).clamp(44.0, 160.0);
                    egui::ScrollArea::vertical()
                        .id_salt("doors")
                        .max_height(door_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for index in 0..self.doors.len() {
                                let title = format!(
                                    "{} · {}",
                                    self.doors[index].name,
                                    door_state_name(self.doors[index].state)
                                );
                                let item =
                                    ui.selectable_label(self.selected_door == Some(index), title);
                                if item.clicked() {
                                    self.select_door(index);
                                    self.door_menu = None;
                                }
                            }
                        });
                    ui.separator();
                }
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
                            let item = ui.selectable_label(self.selected.contains(&index), title);
                            if item.clicked() {
                                let (shift, command) = ui.input(|input| {
                                    (input.modifiers.shift, input.modifiers.command)
                                });
                                if shift {
                                    self.select_range(index);
                                } else if command {
                                    self.toggle_selected(index);
                                } else {
                                    self.select_only(index);
                                }
                                self.object_menu = None;
                            }
                            if item.secondary_clicked() && !self.selected.contains(&index) {
                                self.select_only(index);
                                self.object_menu = None;
                            }
                            let mut remove = false;
                            let mut copy = false;
                            let mut paste = false;
                            let can_paste = !self.clipboard.is_empty() && self.editing();
                            item.context_menu(|ui| {
                                ui.set_min_width(180.0);
                                if ui.button("Копировать").clicked() {
                                    copy = true;
                                }
                                if ui
                                    .add_enabled(can_paste, egui::Button::new("Вставить"))
                                    .clicked()
                                {
                                    paste = true;
                                }
                                if ui
                                    .add_enabled(self.editing(), egui::Button::new("Удалить"))
                                    .clicked()
                                {
                                    remove = true;
                                }
                            });
                            if copy {
                                if !self.selected.contains(&index) {
                                    self.select_only(index);
                                }
                                self.copy_object();
                            }
                            if paste {
                                self.paste_object();
                                break;
                            }
                            if remove {
                                if !self.selected.contains(&index) {
                                    self.select_only(index);
                                }
                                self.delete_selected();
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
        let (marker_positions, marker_colors, marker_id) = self.markers();
        ui.painter()
            .add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                gpu::RoomCallback {
                    scene: Arc::clone(&self.scene),
                    scene_id: self.scene_id,
                    lightmap: Arc::clone(&self.lightmap),
                    lightmap_size: self.lightmap_size,
                    light_id: self.light_id,
                    plain_lights: Arc::clone(&self.plain_lights),
                    tex_lights: Arc::clone(&self.tex_lights),
                    marker_positions,
                    marker_colors,
                    marker_id,
                    view_proj,
                    width,
                    height,
                    slot,
                    map_light: self.map_light,
                    glow: self
                        .objects
                        .iter()
                        .filter_map(|object| object.figure_glow())
                        .collect(),
                },
            ));
    }

    fn inspectors(&mut self, ctx: &egui::Context) {
        self.selected.retain(|index| *index < self.objects.len());
        if self
            .select_anchor
            .is_some_and(|index| index >= self.objects.len())
        {
            self.select_anchor = self.selected.first().copied();
        }
        if self.selected.is_empty() {
            return;
        }
        let indices = self.selected.clone();
        let editing = self.editing();
        let categories = self.categories.clone();
        let mut draft = std::mem::take(&mut self.draft_category);
        let mut pending = Pending::default();
        let mut add_category = false;
        let mut save_base = false;
        let mut use_base = false;
        let was_grouped = self.undo_group;

        let screen = ctx.screen_rect();
        // Object sits against the top-right corner; Light fills the gap on its left.
        let top = screen.top() + 48.0;
        let margin = 16.0;
        let gap = 12.0;
        let frame = 14.0;
        let object_inner = 360.0;
        let object_outer = object_inner + frame;
        {
            let objects = &self.objects;
            egui::Window::new("Объект")
                .id(Id::new("object-panel"))
                .pivot(egui::Align2::RIGHT_TOP)
                .default_pos(Pos2::new(screen.right() - margin, top))
                .default_width(object_inner)
                .default_height(32.0)
                .resizable(true)
                .show(ctx, |ui| {
                    let name_shared = shared_text(objects, &indices, |object| object.name.as_str());
                    let category_shared =
                        shared_text(objects, &indices, |object| object.category.as_str());
                    let space_shared = shared_space(objects, &indices);
                    let size_fallback = objects[indices[0]].size.to_array();
                    let position_fallback = objects[indices[0]].position.to_array();
                    let rotation_fallback = objects[indices[0]].rotation.to_array();
                    let size_shared = [
                        shared_number(objects, &indices, |object| object.size.x),
                        shared_number(objects, &indices, |object| object.size.y),
                        shared_number(objects, &indices, |object| object.size.z),
                    ];
                    let position_shared = [
                        shared_number(objects, &indices, |object| object.position.x),
                        shared_number(objects, &indices, |object| object.position.y),
                        shared_number(objects, &indices, |object| object.position.z),
                    ];
                    let rotation_shared = [
                        shared_number(objects, &indices, |object| object.rotation.x),
                        shared_number(objects, &indices, |object| object.rotation.y),
                        shared_number(objects, &indices, |object| object.rotation.z),
                    ];
                    let corners_shared =
                        shared_number(objects, &indices, |object| object.corners);
                    let corners_fallback = objects[indices[0]].corners;
                    ui.add_enabled_ui(editing, |ui| {
                        if indices.len() > 1 {
                            ui.label(format!("Выбрано: {}", indices.len()));
                        }
                        ui.label("Название");
                        let mut name = name_shared.clone().unwrap_or_else(|| "-".to_owned());
                        if ui.text_edit_singleline(&mut name).changed() {
                            pending.name = Some(name);
                        }
                        ui.label("Категория");
                        let category_label = match category_shared.as_deref() {
                            Some("") => "—".to_owned(),
                            Some(name) => name.to_owned(),
                            None => "-".to_owned(),
                        };
                        egui::ComboBox::from_id_salt("object-category")
                            .selected_text(category_label)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(category_shared.as_deref() == Some(""), "—")
                                    .clicked()
                                    && category_shared.as_deref() != Some("")
                                {
                                    pending.category = Some(String::new());
                                }
                                for category in &categories {
                                    if ui
                                        .selectable_label(
                                            category_shared.as_deref() == Some(category.as_str()),
                                            category,
                                        )
                                        .clicked()
                                        && category_shared.as_deref() != Some(category.as_str())
                                    {
                                        pending.category = Some(category.clone());
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
                                .selectable_label(space_shared == Some(Space::Plane), "2D")
                                .clicked()
                                && space_shared != Some(Space::Plane)
                            {
                                pending.space = Some(Space::Plane);
                                pending.shape = true;
                                pending.commit = true;
                            }
                            if ui
                                .selectable_label(space_shared == Some(Space::Solid), "3D")
                                .clicked()
                                && space_shared != Some(Space::Solid)
                            {
                                pending.space = Some(Space::Solid);
                                pending.shape = true;
                                pending.commit = true;
                            }
                        });
                        ui.label("Грани");
                        let size_x = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, size_shared[0], size_fallback[0], "X ", 1.0),
                        );
                        pending.size[0] = size_x;
                        let size_y = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, size_shared[1], size_fallback[1], "Y ", 1.0),
                        );
                        pending.size[1] = size_y;
                        let any_solid = indices
                            .iter()
                            .any(|index| objects[*index].space == Space::Solid);
                        let show_z = match pending.space {
                            Some(Space::Solid) => true,
                            Some(Space::Plane) => false,
                            None => any_solid,
                        };
                        if show_z {
                            let size_z = keep(
                                &mut pending,
                                true,
                                mix_drag(ui, size_shared[2], size_fallback[2], "Z ", 1.0),
                            );
                            pending.size[2] = size_z;
                        }
                        ui.label("Положение");
                        let position_x = keep(
                            &mut pending,
                            false,
                            mix_drag(ui, position_shared[0], position_fallback[0], "X ", 1.0),
                        );
                        pending.position[0] = position_x;
                        let position_y = keep(
                            &mut pending,
                            false,
                            mix_drag(ui, position_shared[1], position_fallback[1], "Y ", 1.0),
                        );
                        pending.position[1] = position_y;
                        let position_z = keep(
                            &mut pending,
                            false,
                            mix_drag(ui, position_shared[2], position_fallback[2], "Z ", 1.0),
                        );
                        pending.position[2] = position_z;
                        ui.label("Поворот");
                        let yaw = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, rotation_shared[0], rotation_fallback[0], "Рыскание ", 1.0),
                        );
                        pending.rotation[0] = yaw;
                        let pitch = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, rotation_shared[1], rotation_fallback[1], "Тангаж ", 1.0),
                        );
                        pending.rotation[1] = pitch;
                        let roll = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, rotation_shared[2], rotation_fallback[2], "Крен ", 1.0),
                        );
                        pending.rotation[2] = roll;
                        let corners = keep(
                            &mut pending,
                            true,
                            mix_slider(ui, corners_shared, corners_fallback, "Углы"),
                        );
                        pending.corners = corners;
                        let space_now = pending.space.or(space_shared);
                        ui.label(
                            RichText::new(match space_now {
                                Some(Space::Plane) => "0 — круг, 100 — квадрат",
                                Some(Space::Solid) => "0 — шар, 100 — куб",
                                None => "0 — круг или шар, 100 — квадрат или куб",
                            })
                            .size(12.0)
                            .color(Color32::from_rgb(150, 150, 156)),
                        );
                        ui.separator();
                        if ui.button("Сохранить как базовые параметры").clicked() {
                            save_base = true;
                        }
                        if ui.button("Привести к базовым параметрам").clicked() {
                            use_base = true;
                        }
                        ui.label(
                            RichText::new(
                                "Имя, категория и положение остаются. Остальное — база новых источников.",
                            )
                            .size(12.0)
                            .color(Color32::from_rgb(150, 150, 156)),
                        );
                    });
                });

            egui::Window::new("Свет")
                .id(Id::new("light-panel"))
                .pivot(egui::Align2::RIGHT_TOP)
                .default_pos(Pos2::new(screen.right() - margin - object_outer - gap, top))
                .default_width(196.0)
                .default_height(32.0)
                .resizable(true)
                .show(ctx, |ui| {
                    let intensity_shared =
                        shared_number(objects, &indices, |object| object.intensity);
                    let intensity_fallback = objects[indices[0]].intensity;
                    let color_shared = shared_color(objects, &indices);
                    let color_fallback = objects[indices[0]].color.to_array();
                    let channel_shared = [
                        shared_number(objects, &indices, |object| object.color.x),
                        shared_number(objects, &indices, |object| object.color.y),
                        shared_number(objects, &indices, |object| object.color.z),
                    ];
                    let inside_shared = shared_number(objects, &indices, |object| object.inside);
                    let rim_shared = shared_number(objects, &indices, |object| object.rim);
                    let spread_shared = shared_number(objects, &indices, |object| object.spread);
                    let mesh_shared = shared_number(objects, &indices, |object| object.mesh);
                    let inside_fallback = objects[indices[0]].inside;
                    let rim_fallback = objects[indices[0]].rim;
                    let spread_fallback = objects[indices[0]].spread;
                    let mesh_fallback = objects[indices[0]].mesh;
                    ui.add_enabled_ui(editing, |ui| {
                        ui.label("Сила и цвет площадки");
                        let intensity = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, intensity_shared, intensity_fallback, "", 100.0),
                        );
                        pending.intensity = intensity;
                        let mut rgb = color_shared
                            .unwrap_or(Vec3::from_array(color_fallback))
                            .to_array();
                        ui.horizontal(|ui| {
                            if color_shared.is_none() {
                                ui.label("-");
                            }
                            let edited = ui.color_edit_button_rgb(&mut rgb);
                            if edited.changed() {
                                pending.color = Some(Vec3::from_array(rgb));
                                pending.shape = true;
                                if edited.dragged() {
                                    pending.preview = true;
                                } else {
                                    pending.commit = true;
                                }
                            }
                        });
                        let red = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, channel_shared[0], color_fallback[0], "R ", 0.01),
                        );
                        pending.channels[0] = red;
                        let green = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, channel_shared[1], color_fallback[1], "G ", 0.01),
                        );
                        pending.channels[1] = green;
                        let blue = keep(
                            &mut pending,
                            true,
                            mix_drag(ui, channel_shared[2], color_fallback[2], "B ", 0.01),
                        );
                        pending.channels[2] = blue;
                        ui.label("Свет внутри фигуры");
                        let inside = keep(
                            &mut pending,
                            false,
                            mix_slider(ui, inside_shared, inside_fallback, "Сила"),
                        );
                        pending.inside = inside;
                        let rim = keep(
                            &mut pending,
                            false,
                            mix_slider(ui, rim_shared, rim_fallback, "Кромка"),
                        );
                        pending.rim = rim;
                        let spread = keep(
                            &mut pending,
                            false,
                            mix_slider(ui, spread_shared, spread_fallback, "Охват"),
                        );
                        pending.spread = spread;
                        let mesh = keep(
                            &mut pending,
                            false,
                            mix_slider(ui, mesh_shared, mesh_fallback, "Сетка"),
                        );
                        pending.mesh = mesh;
                        ui.label(
                            RichText::new("Сила — яркость стекла, выше 40 сильно ярче")
                                .size(12.0)
                                .color(Color32::from_rgb(150, 150, 156)),
                        );
                        ui.label(
                            RichText::new("Кромка — где свечение кончается у края")
                                .size(12.0)
                                .color(Color32::from_rgb(150, 150, 156)),
                        );
                        ui.label(
                            RichText::new("Охват — свечение от центра, сетка — металл")
                                .size(12.0)
                                .color(Color32::from_rgb(150, 150, 156)),
                        );
                    });
                });
        }

        if add_category {
            let name = draft.trim().to_owned();
            if !name.is_empty() && !self.categories.iter().any(|item| item == &name) {
                self.categories.push(name.clone());
            }
            if !name.is_empty() {
                pending.category = Some(name);
            }
            draft.clear();
        }
        // A click that does not move the number still reports a release. Trace
        // only after a real edit, including the release that follows a drag.
        if pending.commit && !pending.any() && !was_grouped {
            pending.commit = false;
        }
        let changed = pending.any() || use_base;
        if changed && !self.undo_group {
            self.remember();
        }
        apply_pending(&mut self.objects, &indices, &pending);
        if use_base {
            let base = base::live();
            for index in &indices {
                apply_base(&mut self.objects[*index], &base);
            }
            pending.commit = true;
        }
        if save_base {
            if let Some(object) = self.objects.get(indices[0]) {
                if let Err(err) = base::store(capture_base(object)) {
                    self.error = Some(err);
                }
            }
        }
        if pending.preview && !pending.commit {
            self.undo_group = true;
        } else if changed && !pending.preview && !pending.commit {
            self.undo_group = true;
        } else if pending.commit || !ctx.wants_keyboard_input() {
            self.undo_group = false;
        }
        self.draft_category = draft;
        if pending.commit {
            self.lamps_moved();
        } else if pending.preview {
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
            .set_directory(browse_start())
            .add_filter("Проект", &["lbr", "lightbaker"])
            .add_filter("Карта или проект", &["bsp", "lbr", "lightbaker"])
            .pick_file()
        else {
            return;
        };
        self.open_any(path);
    }

    fn open_project_file(&mut self, path: &Path) {
        match read_project(path) {
            Ok((map, categories, objects, doors)) => {
                self.project_path = Some(path.to_path_buf());
                self.categories = categories;
                if !self.categories.iter().any(|item| item == RELAPSE) {
                    self.categories.insert(0, RELAPSE.to_owned());
                }
                self.objects = objects;
                self.selected.clear();
                self.select_anchor = None;
                self.selected_door = None;
                self.undo.clear();
                self.redo.clear();
                self.undo_group = false;
                match resolve_map(&map, path) {
                    Ok(map) => {
                        self.door_plan = doors;
                        self.door_plan_map = Some(map.clone());
                        self.begin_open(map);
                    }
                    Err(err) => self.error = Some(err),
                }
            }
            Err(err) => self.error = Some(err),
        }
    }

    fn take_drop(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        for path in dropped {
            if is_lights(&path) {
                self.import_lights_file(&path);
                return;
            }
            if is_project(&path)
                || path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("bsp"))
            {
                self.open_any(path);
                return;
            }
        }
    }

    fn import_map(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_directory(browse_start())
            .add_filter("Карта или проект", &["bsp", "lbr", "lightbaker"])
            .pick_file()
        else {
            return;
        };
        self.open_any(path);
    }

    fn open_any(&mut self, path: PathBuf) {
        if is_project(&path) {
            self.open_project_file(&path);
        } else {
            self.door_plan.clear();
            self.door_plan_map = None;
            self.begin_open(path);
        }
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
        if let Err(err) = write_project(
            path,
            &self.path,
            &self.categories,
            &self.objects,
            &self.door_choices(),
        ) {
            self.error = Some(err);
        }
    }

    fn map_ready(&self) -> bool {
        self.opening.is_none() && !self.luxels.is_empty()
    }

    fn save_lights(&mut self) {
        if !self.map_ready() {
            self.error = Some("Сначала откройте карту.".into());
            return;
        }
        let name = self
            .project_path
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or("источники");
        let start = self
            .project_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .filter(|path| path.is_dir())
            .unwrap_or_else(browse_start);
        let Some(path) = rfd::FileDialog::new()
            .set_directory(start)
            .add_filter("Источники", &["lbrl"])
            .set_file_name(format!("{name}.lbrl"))
            .save_file()
        else {
            return;
        };
        let path = ensure_ext(path, "lbrl");
        if let Err(err) = write_lights(&path, &self.categories, &self.objects) {
            self.error = Some(err);
        }
    }

    fn import_lights(&mut self) {
        if !self.map_ready() {
            self.error = Some("Сначала откройте карту.".into());
            return;
        }
        if !self.editing() {
            self.error = Some("Дождитесь окончания расчёта.".into());
            return;
        }
        let start = self
            .project_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .filter(|path| path.is_dir())
            .unwrap_or_else(browse_start);
        let Some(path) = rfd::FileDialog::new()
            .set_directory(start)
            .add_filter("Источники", &["lbrl"])
            .pick_file()
        else {
            return;
        };
        self.import_lights_file(&path);
    }

    fn import_lights_file(&mut self, path: &Path) {
        if !self.map_ready() {
            self.error = Some("Сначала откройте карту.".into());
            return;
        }
        if !self.editing() {
            self.error = Some("Дождитесь окончания расчёта.".into());
            return;
        }
        let (categories, objects) = match read_lights(path) {
            Ok(read) => read,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        self.remember();
        self.undo_group = false;
        for category in categories {
            if !category.is_empty() && !self.categories.iter().any(|item| item == &category) {
                self.categories.push(category);
            }
        }
        let first = self.objects.len();
        for mut object in objects {
            if self.objects.iter().any(|item| item.name == object.name) {
                object.name = unique_name(&self.objects, name_stem(&object.name));
            }
            if !object.category.is_empty()
                && !self.categories.iter().any(|item| item == &object.category)
            {
                self.categories.push(object.category.clone());
            }
            self.objects.push(object);
        }
        self.selected = (first..self.objects.len()).collect();
        self.select_anchor = self.selected.first().copied();
        self.apply_dynamic(true);
    }

    fn open_export(&mut self) {
        if !self.path.as_os_str().is_empty() {
            self.export_path = beside(&self.path).display().to_string();
        }
        self.export_open = true;
    }

    fn add_object(&mut self, mut object: LightObject) {
        let (min, max) = object.bounds();
        let lift = object.position - self.camera.target;
        object.position = place_along_view(
            self.camera.eye(),
            self.camera.target,
            lift,
            (max - min) * 0.5,
            &self.triangles,
        );
        self.remember();
        self.undo_group = false;
        object.name = unique_name(&self.objects, &object.name);
        self.objects.push(object);
        self.select_only(self.objects.len() - 1);
        self.apply_dynamic(true);
    }

    fn select_only(&mut self, index: usize) {
        self.selected_door = None;
        self.door_menu = None;
        self.selected.clear();
        self.selected.push(index);
        self.select_anchor = Some(index);
    }

    fn select_door(&mut self, index: usize) {
        if index >= self.doors.len() {
            return;
        }
        self.selected.clear();
        self.select_anchor = None;
        self.object_menu = None;
        self.selected_door = Some(index);
    }

    fn toggle_selected(&mut self, index: usize) {
        if let Some(place) = self.selected.iter().position(|item| *item == index) {
            self.selected.remove(place);
            if self.select_anchor == Some(index) {
                self.select_anchor = self.selected.last().copied();
            }
        } else {
            self.selected.push(index);
            self.select_anchor = Some(index);
        }
    }

    fn select_range(&mut self, index: usize) {
        let anchor = self.select_anchor.unwrap_or(index);
        if self.select_anchor.is_none() {
            self.select_anchor = Some(anchor);
        }
        let (lo, hi) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        self.selected = (lo..=hi)
            .filter(|item| *item < self.objects.len())
            .collect();
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
        self.select_anchor = None;
        self.selected_door = None;
        self.door_menu = None;
    }

    fn copy_object(&mut self) {
        let mut indices = self.selected.clone();
        indices.sort_unstable();
        indices.dedup();
        let copied: Vec<_> = indices
            .into_iter()
            .filter_map(|index| self.objects.get(index).cloned())
            .collect();
        if copied.is_empty() {
            return;
        }
        self.clipboard = copied;
        self.paste_times = 0;
    }

    fn paste_object(&mut self) {
        if !self.editing() || self.clipboard.is_empty() {
            return;
        }
        self.paste_times = self.paste_times.saturating_add(1);
        let sources = self.clipboard.clone();
        let span = sources.iter().fold(0.0_f32, |span, object| {
            span.max(object.size.x.max(object.size.y))
        });
        let offset = paste_offset(self.camera.yaw, Vec3::new(span, 0.0, 0.0), self.paste_times);
        self.remember();
        self.undo_group = false;
        let mut made = Vec::new();
        for source in sources {
            let mut object = source;
            object.position += offset;
            object.name = unique_name(&self.objects, name_stem(&object.name));
            self.objects.push(object);
            made.push(self.objects.len() - 1);
        }
        self.selected = made;
        self.select_anchor = self.selected.first().copied();
        self.object_menu = None;
        self.dragging_lamp = false;
        self.drag_index = None;
        self.lamp_moved = false;
        self.apply_dynamic(true);
    }

    fn delete_selected(&mut self) {
        if !self.editing() || self.selected.is_empty() {
            return;
        }
        self.remember();
        self.undo_group = false;
        let mut indices = self.selected.clone();
        indices.sort_unstable();
        indices.dedup();
        for index in indices.into_iter().rev() {
            if index < self.objects.len() {
                self.objects.remove(index);
            }
        }
        self.clear_selection();
        self.object_menu = None;
        self.dragging_lamp = false;
        self.drag_index = None;
        self.lamp_moved = false;
        self.apply_dynamic(true);
    }

    fn revision(&self) -> Revision {
        Revision {
            objects: self.objects.clone(),
            categories: self.categories.clone(),
            selected: self.selected.clone(),
            door_states: self.doors.iter().map(|door| door.state).collect(),
            selected_door: self.selected_door,
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
        let doors = revision.door_states.len() == self.doors.len()
            && revision
                .door_states
                .iter()
                .zip(self.doors.iter().map(|door| door.state))
                .any(|(next, current)| *next != current);
        self.objects = revision.objects;
        self.categories = revision.categories;
        self.selected = revision
            .selected
            .into_iter()
            .filter(|index| *index < self.objects.len())
            .collect();
        self.select_anchor = self.selected.first().copied();
        if revision.door_states.len() == self.doors.len() {
            for (door, state) in self.doors.iter_mut().zip(revision.door_states) {
                door.state = state;
            }
        }
        self.selected_door = revision
            .selected_door
            .filter(|index| *index < self.doors.len());
        self.object_menu = None;
        self.door_menu = None;
        self.undo_group = false;
        self.dragging_lamp = false;
        self.drag_index = None;
        self.lamp_moved = false;
        if doors {
            self.doors_changed();
        } else if light {
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
        let can_paste = !self.clipboard.is_empty() && editing;
        let mut delete = false;
        let mut copy = false;
        let mut paste = false;
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
                    if ui.button("Копировать").clicked() {
                        copy = true;
                    }
                    if ui
                        .add_enabled(can_paste, egui::Button::new("Вставить"))
                        .clicked()
                    {
                        paste = true;
                    }
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
        if copy {
            self.object_menu = None;
            self.copy_object();
        } else if paste {
            self.object_menu = None;
            self.paste_object();
        } else if delete {
            self.object_menu = None;
            self.delete_selected();
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

#[derive(Default)]
struct Pending {
    name: Option<String>,
    category: Option<String>,
    space: Option<Space>,
    size: [Option<f32>; 3],
    position: [Option<f32>; 3],
    rotation: [Option<f32>; 3],
    corners: Option<f32>,
    intensity: Option<f32>,
    color: Option<Vec3>,
    channels: [Option<f32>; 3],
    inside: Option<f32>,
    rim: Option<f32>,
    spread: Option<f32>,
    mesh: Option<f32>,
    preview: bool,
    commit: bool,
    shape: bool,
}

impl Pending {
    fn any(&self) -> bool {
        self.name.is_some()
            || self.category.is_some()
            || self.space.is_some()
            || self.size.iter().any(Option::is_some)
            || self.position.iter().any(Option::is_some)
            || self.rotation.iter().any(Option::is_some)
            || self.corners.is_some()
            || self.intensity.is_some()
            || self.color.is_some()
            || self.channels.iter().any(Option::is_some)
            || self.inside.is_some()
            || self.rim.is_some()
            || self.spread.is_some()
            || self.mesh.is_some()
    }
}

fn keep(pending: &mut Pending, shape: bool, change: (Option<f32>, Edit)) -> Option<f32> {
    let (value, edit) = change;
    pending.preview |= edit.preview;
    pending.commit |= edit.commit;
    if value.is_some() && shape {
        pending.shape = true;
    }
    value
}

fn mix_drag(
    ui: &mut egui::Ui,
    shared: Option<f32>,
    fallback: f32,
    prefix: &str,
    speed: f32,
) -> (Option<f32>, Edit) {
    let mut value = shared.unwrap_or(fallback);
    let mut drag = egui::DragValue::new(&mut value).speed(speed).prefix(prefix);
    if shared.is_none() {
        drag = drag
            .custom_formatter(|_, _| "-".to_owned())
            .custom_parser(parse_number);
    }
    let response = ui.add(drag);
    (
        response.changed().then_some(value),
        Edit {
            preview: response.dragged(),
            commit: released(&response),
        },
    )
}

fn mix_slider(
    ui: &mut egui::Ui,
    shared: Option<f32>,
    fallback: f32,
    text: &str,
) -> (Option<f32>, Edit) {
    let mut value = shared.unwrap_or(fallback);
    let mut slider = egui::Slider::new(&mut value, 0.0..=100.0).text(text);
    if shared.is_none() {
        slider = slider
            .custom_formatter(|_, _| "-".to_owned())
            .custom_parser(parse_number);
    }
    let response = ui.add(slider);
    (
        response.changed().then_some(value),
        Edit {
            preview: response.dragged(),
            commit: released(&response),
        },
    )
}

fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim().trim_end_matches('%').trim();
    if text.is_empty() || text == "-" {
        None
    } else {
        text.parse().ok()
    }
}

fn shared_number(
    objects: &[LightObject],
    indices: &[usize],
    get: impl Fn(&LightObject) -> f32,
) -> Option<f32> {
    let first = *indices.first()?;
    let value = get(&objects[first]);
    indices
        .iter()
        .all(|index| (get(&objects[*index]) - value).abs() <= 1.0e-3)
        .then_some(value)
}

fn shared_text(
    objects: &[LightObject],
    indices: &[usize],
    get: impl Fn(&LightObject) -> &str,
) -> Option<String> {
    let first = *indices.first()?;
    let value = get(&objects[first]);
    indices
        .iter()
        .all(|index| get(&objects[*index]) == value)
        .then(|| value.to_owned())
}

fn shared_space(objects: &[LightObject], indices: &[usize]) -> Option<Space> {
    let first = *indices.first()?;
    let value = objects[first].space;
    indices
        .iter()
        .all(|index| objects[*index].space == value)
        .then_some(value)
}

fn shared_color(objects: &[LightObject], indices: &[usize]) -> Option<Vec3> {
    let first = *indices.first()?;
    let value = objects[first].color;
    indices
        .iter()
        .all(|index| {
            let color = objects[*index].color;
            (color.x - value.x).abs() <= 1.0e-3
                && (color.y - value.y).abs() <= 1.0e-3
                && (color.z - value.z).abs() <= 1.0e-3
        })
        .then_some(value)
}

fn apply_pending(objects: &mut [LightObject], indices: &[usize], pending: &Pending) {
    for index in indices {
        let object = &mut objects[*index];
        if let Some(name) = &pending.name {
            object.name.clone_from(name);
        }
        if let Some(category) = &pending.category {
            object.category.clone_from(category);
        }
        if let Some(space) = pending.space {
            object.space = space;
        }
        if let Some(value) = pending.size[0] {
            object.size.x = value;
        }
        if let Some(value) = pending.size[1] {
            object.size.y = value;
        }
        if let Some(value) = pending.size[2] {
            object.size.z = value;
        }
        if pending.size.iter().any(Option::is_some) {
            object.size = object.size.max(Vec3::splat(1.0));
        }
        if let Some(value) = pending.position[0] {
            object.position.x = value;
        }
        if let Some(value) = pending.position[1] {
            object.position.y = value;
        }
        if let Some(value) = pending.position[2] {
            object.position.z = value;
        }
        if let Some(value) = pending.rotation[0] {
            object.rotation.x = value;
        }
        if let Some(value) = pending.rotation[1] {
            object.rotation.y = value;
        }
        if let Some(value) = pending.rotation[2] {
            object.rotation.z = value;
        }
        if let Some(value) = pending.corners {
            object.corners = value.clamp(0.0, 100.0);
        }
        if let Some(value) = pending.intensity {
            object.intensity = value.max(0.0);
        }
        if let Some(color) = pending.color {
            object.color = color;
        }
        if let Some(value) = pending.channels[0] {
            object.color.x = value;
        }
        if let Some(value) = pending.channels[1] {
            object.color.y = value;
        }
        if let Some(value) = pending.channels[2] {
            object.color.z = value;
        }
        if pending.color.is_some() || pending.channels.iter().any(Option::is_some) {
            object.color = object.color.clamp(Vec3::ZERO, Vec3::ONE);
        }
        if let Some(value) = pending.inside {
            object.inside = value.clamp(0.0, 100.0);
        }
        if let Some(value) = pending.rim {
            object.rim = value.clamp(0.0, 100.0);
        }
        if let Some(value) = pending.spread {
            object.spread = value.clamp(0.0, 100.0);
        }
        if let Some(value) = pending.mesh {
            object.mesh = value.clamp(0.0, 100.0);
        }
        if pending.shape {
            object.custom = true;
        }
    }
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
    menu_command(ui, title, shortcut, true)
}

fn menu_command(ui: &mut egui::Ui, title: &str, shortcut: &str, enabled: bool) -> bool {
    ui.horizontal(|ui| {
        let clicked = ui.add_enabled(enabled, egui::Button::new(title)).clicked();
        ui.add_space(16.0);
        ui.label(RichText::new(shortcut).color(Color32::from_rgb(140, 140, 148)));
        clicked
    })
    .inner
}

/// Physical Ctrl+C and Ctrl+V. egui keeps those chords for the system clipboard
/// and never reports them as the C and V keys.
fn command_chord() -> (bool, bool) {
    let ctrl = vk_down(0x11);
    let shift = vk_down(0x10);
    let alt = vk_down(0x12);
    if !ctrl || shift || alt {
        return (false, false);
    }
    (vk_down(0x43), vk_down(0x56))
}

fn vk_down(vk: i32) -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(key: i32) -> i16;
    }
    unsafe { GetAsyncKeyState(vk) < 0 }
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
        && left.inside == right.inside
        && left.rim == right.rim
        && left.color == right.color
        && left.template == right.template
        && left.custom == right.custom
}

fn name_stem(name: &str) -> &str {
    let Some((head, tail)) = name.rsplit_once(' ') else {
        return name;
    };
    if !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit()) {
        head
    } else {
        name
    }
}

/// Steps a pasted copy to the camera's right so it does not sit inside the original.
fn paste_offset(yaw: f32, size: Vec3, times: u32) -> Vec3 {
    let right = Vec3::new(-yaw.cos(), yaw.sin(), 0.0);
    let right = if right.length_squared() < 1.0e-8 {
        Vec3::X
    } else {
        right.normalize()
    };
    let span = size.x.max(size.y).max(8.0) + 8.0;
    right * span * times as f32
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
        Area::Volume(volume) => box_half(volume.axis_x, volume.axis_y, volume.axis_z),
        Area::Omni(omni) => box_half(omni.axis_x, omni.axis_y, omni.axis_z),
    }
}

fn box_half(axis_x: Vec3, axis_y: Vec3, axis_z: Vec3) -> Vec3 {
    let mut half = Vec3::ZERO;
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            for sz in [-1.0, 1.0] {
                let corner = axis_x * sx + axis_y * sy + axis_z * sz;
                half = half.max(corner.abs());
            }
        }
    }
    half
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

/// Seats a new object on the first surface along the view from `eye` toward `target`.
/// `half` is the world-axis hitbox. `lift` stays on top of that seat, so the sun and
/// moon keep their sky offset. With no surface, the object stays at the orbit point.
fn place_along_view(
    eye: Vec3,
    target: Vec3,
    lift: Vec3,
    half: Vec3,
    triangles: &[Triangle],
) -> Vec3 {
    let fallback = target + lift;
    let dir = (target - eye).normalize_or_zero();
    if dir == Vec3::ZERO {
        return fallback;
    }
    let Some((hit, normal)) = nearest_surface(eye, dir, triangles) else {
        return fallback;
    };
    seat_on(hit, normal, half) + lift
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

#[derive(Eq, PartialEq, Hash)]
struct BatchKey {
    base: u32,
    second: u32,
    detail: u32,
    scale: u32,
    factor: u32,
}

#[derive(Clone, Copy)]
struct FacePatch {
    origin: [u32; 2],
    width: u32,
    height: u32,
    first: u32,
}

struct LightAtlas {
    size: u32,
    patches: Vec<Option<FacePatch>>,
}

impl LightAtlas {
    fn blank() -> Self {
        Self {
            size: 32,
            patches: Vec::new(),
        }
    }

    fn texels(&self) -> usize {
        (self.size as usize) * (self.size as usize) * 4
    }

    fn place(&self, face: u32, uv: [f32; 2]) -> [f32; 2] {
        let Some(Some(patch)) = self.patches.get(face as usize) else {
            return [0.5 / self.size as f32, 0.5 / self.size as f32];
        };
        [
            (patch.origin[0] as f32 + uv[0]) / self.size as f32,
            (patch.origin[1] as f32 + uv[1]) / self.size as f32,
        ]
    }
}

fn atlas_faces(snapshot: &map::Snapshot) -> Vec<map::FaceLight> {
    let mut faces = snapshot.faces.clone();
    faces.extend(map::prop_grids(&snapshot.props));
    faces
}

fn pack_faces(faces: &[map::FaceLight]) -> LightAtlas {
    let mut items = Vec::new();
    for (index, face) in faces.iter().enumerate() {
        let width = face.width;
        let height = face.height;
        if width == 0
            || height == 0
            || face.luxel_count != width.saturating_mul(height)
            || width.saturating_add(2) > 4096
            || height.saturating_add(2) > 4096
        {
            continue;
        }
        items.push((index, width, height, face.first_luxel));
    }
    items.sort_by(|left, right| right.2.cmp(&left.2).then(right.1.cmp(&left.1)));
    let mut size = 32u32;
    loop {
        if let Some(patches) = try_pack(&items, faces.len(), size) {
            return LightAtlas { size, patches };
        }
        if size >= 4096 {
            return LightAtlas {
                size: 4096,
                patches: pack_partial(&items, faces.len(), 4096),
            };
        }
        size *= 2;
    }
}

fn try_pack(
    items: &[(usize, u32, u32, u32)],
    faces: usize,
    size: u32,
) -> Option<Vec<Option<FacePatch>>> {
    let mut patches = vec![None; faces];
    let mut cursor = PackCursor {
        x: 1,
        y: 1,
        row_h: 0,
    };
    for &(index, width, height, first) in items {
        let origin = cursor.place(width, height, size)?;
        patches[index] = Some(FacePatch {
            origin,
            width,
            height,
            first,
        });
    }
    Some(patches)
}

fn pack_partial(
    items: &[(usize, u32, u32, u32)],
    faces: usize,
    size: u32,
) -> Vec<Option<FacePatch>> {
    let mut patches = vec![None; faces];
    let mut cursor = PackCursor {
        x: 1,
        y: 1,
        row_h: 0,
    };
    for &(index, width, height, first) in items {
        let Some(origin) = cursor.place(width, height, size) else {
            continue;
        };
        patches[index] = Some(FacePatch {
            origin,
            width,
            height,
            first,
        });
    }
    patches
}

struct PackCursor {
    x: u32,
    y: u32,
    row_h: u32,
}

impl PackCursor {
    fn place(&mut self, width: u32, height: u32, size: u32) -> Option<[u32; 2]> {
        let slot_w = width + 2;
        let slot_h = height + 2;
        if self.x > 1 && self.x.saturating_add(slot_w) > size {
            self.x = 1;
            self.y = self.y.saturating_add(self.row_h);
            self.row_h = 0;
        }
        if self.y.saturating_add(slot_h) > size || self.x.saturating_add(slot_w) > size {
            return None;
        }
        let origin = [self.x + 1, self.y + 1];
        self.x += slot_w;
        self.row_h = self.row_h.max(slot_h);
        Some(origin)
    }
}

fn fill_lightmap(atlas: &LightAtlas, colors: &[[f32; 3]], pixels: &mut [u16]) {
    if pixels.len() != atlas.texels() {
        return;
    }
    pixels.fill(0);
    let size = atlas.size as usize;
    for patch in atlas.patches.iter().flatten() {
        for t in 0..patch.height {
            for s in 0..patch.width {
                let index = patch.first as usize + (t * patch.width + s) as usize;
                let color = colors.get(index).copied().unwrap_or([0.0, 0.0, 0.0]);
                put_color(
                    pixels,
                    size,
                    patch.origin[0] + s,
                    patch.origin[1] + t,
                    color,
                );
            }
        }
        duplicate_edges(pixels, size, patch);
    }
    // Texel (0, 0) is what a face without a luxel grid samples. Padding must
    // not leave another face's light there, or that face stays lit with the
    // checkbox off.
    put_color(pixels, size, 0, 0, [0.0, 0.0, 0.0]);
}

fn put_color(pixels: &mut [u16], size: usize, x: u32, y: u32, color: [f32; 3]) {
    put_bits(
        pixels,
        size,
        x,
        y,
        [
            f16_bits(color[0]),
            f16_bits(color[1]),
            f16_bits(color[2]),
            f16_bits(1.0),
        ],
    );
}

fn put_bits(pixels: &mut [u16], size: usize, x: u32, y: u32, bits: [u16; 4]) {
    let x = x as usize;
    let y = y as usize;
    if x >= size || y >= size {
        return;
    }
    let index = (y * size + x) * 4;
    pixels[index..index + 4].copy_from_slice(&bits);
}

fn pixel_bits(pixels: &[u16], size: usize, x: u32, y: u32) -> [u16; 4] {
    let index = (y as usize * size + x as usize) * 4;
    [
        pixels[index],
        pixels[index + 1],
        pixels[index + 2],
        pixels[index + 3],
    ]
}

fn duplicate_edges(pixels: &mut [u16], size: usize, patch: &FacePatch) {
    for s in 0..patch.width {
        let top = pixel_bits(pixels, size, patch.origin[0] + s, patch.origin[1]);
        put_bits(pixels, size, patch.origin[0] + s, patch.origin[1] - 1, top);
        let bottom = pixel_bits(
            pixels,
            size,
            patch.origin[0] + s,
            patch.origin[1] + patch.height - 1,
        );
        put_bits(
            pixels,
            size,
            patch.origin[0] + s,
            patch.origin[1] + patch.height,
            bottom,
        );
    }
    for t in 0..patch.height {
        let left = pixel_bits(pixels, size, patch.origin[0], patch.origin[1] + t);
        put_bits(pixels, size, patch.origin[0] - 1, patch.origin[1] + t, left);
        let right = pixel_bits(
            pixels,
            size,
            patch.origin[0] + patch.width - 1,
            patch.origin[1] + t,
        );
        put_bits(
            pixels,
            size,
            patch.origin[0] + patch.width,
            patch.origin[1] + t,
            right,
        );
    }
    let corners = [
        (0i32, 0i32, -1i32, -1i32),
        (patch.width as i32 - 1, 0, patch.width as i32, -1),
        (0, patch.height as i32 - 1, -1, patch.height as i32),
        (
            patch.width as i32 - 1,
            patch.height as i32 - 1,
            patch.width as i32,
            patch.height as i32,
        ),
    ];
    for (sx, sy, dx, dy) in corners {
        let bits = pixel_bits(
            pixels,
            size,
            (patch.origin[0] as i32 + sx) as u32,
            (patch.origin[1] as i32 + sy) as u32,
        );
        put_bits(
            pixels,
            size,
            (patch.origin[0] as i32 + dx) as u32,
            (patch.origin[1] as i32 + dy) as u32,
            bits,
        );
    }
}

fn f16_bits(value: f32) -> u16 {
    if !(value > 0.0) {
        return 0;
    }
    let bits = value.min(65504.0).to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    if exp <= 0 {
        return 0;
    }
    if exp >= 31 {
        return 0x7c00;
    }
    ((exp as u16) << 10) | (((bits & 0x7f_ffff) >> 13) as u16)
}

fn f16_decode(bits: u16) -> f32 {
    let exp = ((bits >> 10) & 0x1f) as i32;
    let mant = (bits & 0x3ff) as f32;
    if exp == 0 {
        return mant / 1024.0 * 2.0f32.powi(-14);
    }
    if exp == 31 {
        return f32::INFINITY;
    }
    (1.0 + mant / 1024.0) * 2.0f32.powi(exp - 15)
}

fn build_scene(
    surface: &[map::Surface],
    materials: &[map::Material],
    images: Arc<Vec<map::Image>>,
    atlas: &LightAtlas,
) -> gpu::Scene {
    let mut plain_positions = Vec::new();
    let mut plain_colors = Vec::new();
    let mut plain_uvs = Vec::new();
    let mut plain_luxels = Vec::new();
    let mut plain_props = Vec::new();
    let mut plain_normals = Vec::new();
    let mut groups: HashMap<BatchKey, (Vec<gpu::TexVert>, Vec<u32>, Vec<f32>, Vec<[f32; 3]>)> =
        HashMap::new();
    for tri in surface {
        let material = materials.get(tri.material as usize);
        let prop = if tri.prop { 1.0 } else { 0.0 };
        let Some(base) = material.and_then(|material| {
            material
                .texture
                .filter(|index| images.get(*index as usize).is_some())
        }) else {
            push_triangle_positions(&mut plain_positions, tri.vertices);
            push_repeat(&mut plain_colors, surface_color(tri.albedo), 3);
            for corner in 0..3 {
                plain_luxels.push(tri.luxel[corner]);
                plain_props.push(prop);
                plain_uvs.push(light_chart(atlas, tri, corner));
                plain_normals.push(tri.normal[corner].to_array());
            }
            continue;
        };
        let material = material.expect("texture belongs to a material");
        let second = material
            .texture2
            .filter(|index| images.get(*index as usize).is_some())
            .unwrap_or(u32::MAX);
        let detail = material
            .detail
            .filter(|index| images.get(*index as usize).is_some())
            .unwrap_or(u32::MAX);
        let scale = if detail == u32::MAX {
            1.0
        } else {
            material.detail_scale
        };
        let factor = if detail == u32::MAX {
            0.0
        } else {
            material.detail_factor
        };
        let group = groups.entry(BatchKey {
            base,
            second,
            detail,
            scale: scale.to_bits(),
            factor: factor.to_bits(),
        });
        let group = group.or_default();
        for corner in 0..3 {
            group.0.push(gpu::TexVert {
                position: tri.vertices[corner].to_array(),
                uv: tri.uv[corner],
                blend: if second == u32::MAX {
                    0.0
                } else {
                    tri.blend[corner]
                },
                detail_scale: scale,
                detail_factor: factor,
                light_uv: light_chart(atlas, tri, corner),
            });
            group.1.push(tri.luxel[corner]);
            group.2.push(prop);
            group.3.push(tri.normal[corner].to_array());
        }
    }
    let mut verts = Vec::new();
    let mut tex_luxels = Vec::new();
    let mut tex_props = Vec::new();
    let mut tex_normals = Vec::new();
    let mut draws = Vec::new();
    for (key, (group, luxels, props, normals)) in groups {
        let start = verts.len() as u32;
        draws.push(gpu::Draw {
            start,
            count: group.len() as u32,
            base: key.base,
            second: key.second,
            detail: key.detail,
        });
        verts.extend(group);
        tex_luxels.extend(luxels);
        tex_props.extend(props);
        tex_normals.extend(normals);
    }
    gpu::Scene {
        verts,
        draws,
        images,
        plain_positions,
        plain_colors,
        plain_uvs,
        plain_luxels,
        plain_props,
        plain_normals,
        tex_luxels,
        tex_props,
        tex_normals,
    }
}

/// Atlas coordinate of one corner. A vertex-lit prop does not use the bare
/// texel: that sample is black, and the figure tint is carried separately.
fn light_chart(atlas: &LightAtlas, tri: &map::Surface, corner: usize) -> [f32; 2] {
    if tri.luxel[corner] == u32::MAX {
        atlas.place(tri.light_face, tri.light_uv[corner])
    } else {
        [0.25, 0.25]
    }
}

fn vertex_lights(luxels: &[u32], props: &[f32], colors: &[[f32; 3]]) -> Vec<[f32; 4]> {
    luxels
        .iter()
        .enumerate()
        .map(|(index, &luxel)| {
            let prop = props.get(index).copied().unwrap_or(0.0);
            let color = if luxel == u32::MAX {
                [-1.0, -1.0, -1.0]
            } else {
                colors
                    .get(luxel as usize)
                    .copied()
                    .unwrap_or([-1.0, -1.0, -1.0])
            };
            [color[0], color[1], color[2], prop]
        })
        .collect()
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
        apply_pending, build_scene, f16_decode, fill_lightmap, hitbox_half, is_lights, is_project,
        launch_from, name_stem, ndc_ray, nearest_surface, pack_faces, paste_offset, pick_area,
        place_along_view, resolve_map, seat_on, shared_number, surface_color, unique_name,
        vertex_lights, visibility_column, Camera, LightAtlas, Pending, FLY_BOOST, SEAT_GAP,
    };
    use crate::project::LightObject;

    #[test]
    fn a_paste_keeps_the_stem_and_steps_aside() {
        assert_eq!(name_stem("Клетка"), "Клетка");
        assert_eq!(name_stem("Клетка 2"), "Клетка");
        assert_eq!(name_stem("Клетка relapse"), "Клетка relapse");
        let original = LightObject::fluorescent(Vec3::new(10.0, 20.0, 30.0));
        let named = unique_name(&[original.clone()], name_stem(&original.name));
        assert_eq!(named, format!("{} 2", original.name));
        let once = paste_offset(0.0, original.size, 1);
        let twice = paste_offset(0.0, original.size, 2);
        assert!(once.length() > 8.0);
        assert!((twice - once * 2.0).length() < 1.0e-3);
        assert!(once.z.abs() < 1.0e-4);
    }

    #[test]
    fn a_mixed_field_keeps_the_others() {
        let first = LightObject::fluorescent(Vec3::new(1.0, 2.0, 3.0));
        let mut second = first.clone();
        second.position = Vec3::new(4.0, 5.0, 6.0);
        second.intensity = 100.0;
        second.name = "Другой".to_owned();
        let objects = [first.clone(), second.clone()];
        assert!(shared_number(&objects, &[0, 1], |object| object.intensity).is_none());
        assert!(shared_number(&objects, &[0], |object| object.intensity).is_some());
        let mut pending = Pending::default();
        pending.intensity = Some(50.0);
        pending.shape = true;
        let mut edited = objects.clone();
        apply_pending(&mut edited, &[0, 1], &pending);
        assert_eq!(edited[0].intensity, 50.0);
        assert_eq!(edited[1].intensity, 50.0);
        assert_eq!(edited[0].position, first.position);
        assert_eq!(edited[1].position, second.position);
        assert_eq!(edited[0].name, first.name);
        assert_eq!(edited[1].name, second.name);
        assert!(edited[0].custom && edited[1].custom);
    }

    #[test]
    fn a_cached_shadow_keeps_each_area_apart() {
        // Receiver-major, the same order Cover::see writes: three receivers, two areas.
        let seen = vec![
            1.0, 0.25, // receiver 0
            0.5, 0.0, // receiver 1
            0.0, 1.0, // receiver 2
        ];
        assert_eq!(visibility_column(&seen, 3, 2, 0), vec![1.0, 0.5, 0.0]);
        assert_eq!(visibility_column(&seen, 3, 2, 1), vec![0.25, 0.0, 1.0]);
    }

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
        assert!(color.iter().all(|channel| *channel > 0.12), "{color:?}");
    }

    fn flat(corners: [Vec3; 3], light_uv: [[f32; 2]; 3], light_face: u32) -> map::Surface {
        map::Surface {
            vertices: corners,
            albedo: Vec3::ONE,
            uv: [[0.0; 2]; 3],
            blend: [0.0; 3],
            material: map::NO_MATERIAL,
            light_uv,
            light_face,
            luxel: [u32::MAX; 3],
            prop: false,
            normal: [Vec3::Z; 3],
        }
    }

    #[test]
    fn a_luxel_lands_in_its_own_texel_and_not_on_a_triangle_corner() {
        let face = map::FaceLight {
            light_offset: 0,
            width: 2,
            height: 2,
            styles: [0, 255, 255, 255],
            bumped: false,
            first_luxel: 0,
            luxel_count: 4,
        };
        let atlas = pack_faces(&[face]);
        assert_eq!(atlas.size, 32);
        let patch = atlas.patches[0].expect("packed face");
        assert_eq!(patch.origin, [2, 2]);
        assert!(
            (atlas.place(0, [0.5, 0.5])[0] - 2.5 / 32.0).abs() < 1.0e-5,
            "the center of the first luxel is one pixel, not a mesh corner"
        );

        let mut pixels = vec![0u16; atlas.texels()];
        fill_lightmap(
            &atlas,
            &[
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
            ],
            &mut pixels,
        );
        let red = |x, y| {
            let index = (y * atlas.size as usize + x) * 4;
            f16_decode(pixels[index])
        };
        assert!((red(2, 2) - 1.0).abs() < 1.0e-3);
        assert!((red(3, 2) - 0.0).abs() < 1.0e-3);
        assert!(
            (red(1, 2) - 1.0).abs() < 1.0e-3,
            "padding repeats the edge texel"
        );
        assert_eq!(red(0, 0), 0.0);

        let tri = flat(
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            [[0.5, 0.5], [1.5, 0.5], [0.5, 1.5]],
            0,
        );
        let scene = build_scene(&[tri], &[], std::sync::Arc::new(Vec::new()), &atlas);
        assert_eq!(scene.plain_uvs.len(), 3);
        assert!((scene.plain_uvs[0][0] - 2.5 / 32.0).abs() < 1.0e-5);
        assert!((scene.plain_uvs[1][0] - 3.5 / 32.0).abs() < 1.0e-5);
        assert!((scene.plain_uvs[2][1] - 3.5 / 32.0).abs() < 1.0e-5);
    }

    #[test]
    fn a_vertex_lit_prop_takes_its_luxel_and_skips_the_atlas() {
        let mut tri = flat([Vec3::ZERO, Vec3::X, Vec3::Y], [[0.0; 2]; 3], u32::MAX);
        tri.luxel = [4, u32::MAX, 5];
        let atlas = LightAtlas::blank();
        let scene = build_scene(&[tri], &[], std::sync::Arc::new(Vec::new()), &atlas);
        assert_eq!(scene.plain_luxels, vec![4, u32::MAX, 5]);
        let colors = [
            [0.0; 3],
            [0.0; 3],
            [0.0; 3],
            [0.0; 3],
            [0.2, 0.4, 0.6],
            [1.0, 0.0, 0.0],
        ];
        let lights = vertex_lights(&scene.plain_luxels, &scene.plain_props, &colors);
        assert_eq!(lights[0], [0.2, 0.4, 0.6, 0.0]);
        assert_eq!(lights[1], [-1.0, -1.0, -1.0, 0.0]);
        assert_eq!(lights[2], [1.0, 0.0, 0.0, 0.0]);
        assert!((scene.plain_uvs[0][0] - 0.25).abs() < 1.0e-6);
        assert!(scene.plain_normals.iter().all(|normal| normal[2] > 0.9));
        let unlit = 0.5 / atlas.size as f32;
        assert!((scene.plain_uvs[1][0] - unlit).abs() < 1.0e-5);

        let image = map::Image {
            width: 1,
            height: 1,
            mips: vec![vec![255, 255, 255, 255]],
        };
        let material = map::Material {
            texture: Some(0),
            ..map::Material::default()
        };
        tri.material = 0;
        tri.luxel = [2, 2, 2];
        tri.prop = true;
        let scene = build_scene(
            &[tri],
            &[material],
            std::sync::Arc::new(vec![image]),
            &atlas,
        );
        assert!(scene.plain_positions.is_empty());
        assert_eq!(scene.tex_luxels, vec![2, 2, 2]);
        assert_eq!(scene.tex_props, vec![1.0, 1.0, 1.0]);
        assert!(scene.tex_normals.iter().all(|normal| normal[2] > 0.9));
        assert!((scene.verts[0].light_uv[0] - 0.25).abs() < 1.0e-6);
        let colors = [[0.0; 3], [0.0; 3], [0.3, 0.5, 0.7]];
        assert_eq!(
            vertex_lights(&scene.tex_luxels, &scene.tex_props, &colors)[0],
            [0.3, 0.5, 0.7, 1.0]
        );
        assert_eq!(
            vertex_lights(&[9], &[], &colors)[0],
            [-1.0, -1.0, -1.0, 0.0]
        );
    }

    #[test]
    fn a_project_finds_the_map_when_the_saved_path_is_gone() {
        let dir = std::env::temp_dir().join("lightbaker-map-resolve");
        std::fs::create_dir_all(&dir).unwrap();
        let bsp = dir.join("room.bsp");
        std::fs::write(&bsp, b"bsp").unwrap();
        let project = dir.join("room.lbr");
        let stored = std::path::PathBuf::from(r"D:\missing\maps\room.bsp");
        let found = resolve_map(&stored, &project).unwrap();
        assert_eq!(found, bsp);
        assert!(is_project(&project));
        assert!(!is_project(&bsp));
        assert!(resolve_map(std::path::Path::new(""), &project).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_argument_opens_the_project_not_the_menu() {
        match launch_from([r"C:\Users\Egor\Desktop\jail.lbr"]) {
            super::Launch::Project(path) => assert!(is_project(&path)),
            other => panic!("expected the project, got {other:?}"),
        }
        assert!(matches!(
            launch_from(["--flag", r"C:\Users\Egor\Desktop\jail.lbr"]),
            super::Launch::Project(_)
        ));
        assert!(matches!(
            launch_from(Vec::<&str>::new()),
            super::Launch::None
        ));
        assert!(matches!(
            launch_from([r"C:\Users\Egor\Desktop\jail.lbrl"]),
            super::Launch::None
        ));
        assert!(!is_lights(std::path::Path::new("jail.lbr")));
        assert!(is_lights(std::path::Path::new("jail.lbrl")));
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
    fn a_new_object_sits_on_the_surface_the_camera_looks_at() {
        let view = camera(0.0);
        let wall = [Triangle {
            vertices: [
                Vec3::new(-4.0, 0.0, -4.0),
                Vec3::new(4.0, 0.0, -4.0),
                Vec3::new(0.0, 0.0, 4.0),
            ],
        }];
        let half = Vec3::new(3.0, 2.0, 5.0);
        let placed = place_along_view(view.eye(), view.target, Vec3::ZERO, half, &wall);
        assert!((placed.y - (half.y + SEAT_GAP)).abs() < 1.0e-3);
        assert!(placed.x.abs() < 1.0e-3 && placed.z.abs() < 1.0e-3);

        let sky = Vec3::new(0.0, 0.0, 4096.0);
        let sun = place_along_view(view.eye(), view.target, sky, half, &wall);
        assert!((sun - (placed + sky)).length() < 1.0e-3);

        let missed = place_along_view(view.eye(), view.target, sky, half, &[]);
        assert_eq!(missed, view.target + sky);
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
