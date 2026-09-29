use glam::Vec3;
use solve::{solve, Area, Receiver, Rectangle, Role, Triangle};

fn other(position: Vec3, normal: Vec3) -> Receiver {
    Receiver {
        position,
        normal,
        albedo: Vec3::ZERO,
        role: Role::Other,
    }
}

fn wall(position: Vec3, normal: Vec3, albedo: Vec3) -> Receiver {
    Receiver {
        position,
        normal,
        albedo,
        role: Role::Wall,
    }
}

fn floor(position: Vec3, albedo: Vec3) -> Receiver {
    Receiver {
        position,
        normal: Vec3::Z,
        albedo,
        role: Role::Floor,
    }
}

fn quad(origin: Vec3, edge_u: Vec3, edge_v: Vec3) -> [Triangle; 2] {
    [
        Triangle {
            vertices: [origin, origin + edge_u, origin + edge_u + edge_v],
        },
        Triangle {
            vertices: [origin, origin + edge_u + edge_v, origin + edge_v],
        },
    ]
}

fn closed_box(min: Vec3, max: Vec3) -> Vec<Triangle> {
    let p = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    let quads = [
        [0, 1, 2, 3],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [3, 2, 6, 7],
        [0, 3, 7, 4],
        [1, 2, 6, 5],
    ];
    let mut triangles = Vec::with_capacity(12);
    for face in quads {
        triangles.push(Triangle {
            vertices: [p[face[0]], p[face[1]], p[face[2]]],
        });
        triangles.push(Triangle {
            vertices: [p[face[0]], p[face[2]], p[face[3]]],
        });
    }
    triangles
}

fn downward(center: Vec3, intensity: f32) -> Area {
    Area::Rectangle(Rectangle {
        center,
        half_u: Vec3::new(0.6, 0.0, 0.0),
        half_v: Vec3::new(0.0, 0.6, 0.0),
        normal: -Vec3::Z,
        intensity,
        color: Vec3::ONE,
    })
}

#[test]
fn wall_albedo_enters_the_bounce_only() {
    let light = downward(Vec3::new(0.0, 0.0, 8.0), 400.0);
    let blocker = quad(
        Vec3::new(-4.0, -8.0, 3.0),
        Vec3::new(8.0, 0.0, 0.0),
        Vec3::new(0.0, 16.0, 0.0),
    );
    let surface = quad(
        Vec3::new(6.0, -8.0, 0.0),
        Vec3::new(0.0, 16.0, 0.0),
        Vec3::new(0.0, 0.0, 12.0),
    );
    let mut triangles = Vec::new();
    triangles.extend(blocker);
    triangles.extend(surface);

    let probe = other(Vec3::new(2.0, 0.0, 0.0), Vec3::Z);
    let dark_wall = wall(Vec3::new(6.0, 0.0, 6.0), -Vec3::X, Vec3::ZERO);
    let lit_wall = wall(Vec3::new(6.0, 0.0, 6.0), -Vec3::X, Vec3::new(0.8, 0.2, 0.1));
    let dark = solve(&triangles, &[dark_wall, probe], &[light], 64);
    let lit = solve(&triangles, &[lit_wall, probe], &[light], 64);

    assert_eq!(
        dark.light[0], lit.light[0],
        "wall luxel must ignore its albedo"
    );
    assert_eq!(dark.light[1], [0.0, 0.0, 0.0]);
    assert!(lit.light[1][0] > lit.light[1][1], "{:?}", lit.light[1]);
    assert!(lit.light[1][0] > 1.0e-3, "{:?}", lit.light[1]);
}

#[test]
fn floor_albedo_does_not_change_light() {
    let light = downward(Vec3::new(0.0, 0.0, 8.0), 400.0);
    let surface = quad(
        Vec3::new(6.0, -8.0, 0.0),
        Vec3::new(0.0, 16.0, 0.0),
        Vec3::new(0.0, 0.0, 12.0),
    );
    let wall = wall(Vec3::new(6.0, 0.0, 6.0), -Vec3::X, Vec3::new(0.7, 0.6, 0.5));
    let bare = floor(Vec3::new(0.0, 0.0, 0.0), Vec3::ZERO);
    let painted = floor(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.2, 0.8, 0.1));
    let plain = solve(&surface, &[wall, bare], &[light], 32);
    let tinted = solve(&surface, &[wall, painted], &[light], 32);
    assert_eq!(plain, tinted);
}

#[test]
fn two_bounces_carry_the_second_wall() {
    let light = downward(Vec3::new(0.0, 0.0, 12.0), 400.0);
    let mut triangles = Vec::new();
    triangles.extend(quad(
        Vec3::new(6.0, -10.0, 0.0),
        Vec3::new(0.0, 20.0, 0.0),
        Vec3::new(0.0, 0.0, 16.0),
    ));
    triangles.extend(quad(
        Vec3::new(2.0, -10.0, 0.0),
        Vec3::new(0.0, 20.0, 0.0),
        Vec3::new(0.0, 0.0, 9.0),
    ));

    let probe = other(Vec3::new(4.0, 0.0, 3.0), -Vec3::X);
    let run = |albedo_a: Vec3, albedo_b: Vec3| {
        let receivers = [
            wall(Vec3::new(6.0, 0.0, 6.0), -Vec3::X, albedo_a),
            wall(Vec3::new(2.0, 0.0, 3.0), Vec3::X, albedo_b),
            probe,
        ];
        solve(&triangles, &receivers, &[light], 64)
    };

    let carried = run(Vec3::new(1.0, 0.0, 0.0), Vec3::ONE);
    assert!(carried.light[2][0] > 1.0e-3, "{:?}", carried.light[2]);
    assert_eq!(carried.light[2][1], 0.0);
    assert_eq!(carried.light[2][2], 0.0);

    let stopped_at_second = run(Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO);
    assert_eq!(stopped_at_second.light[2], [0.0, 0.0, 0.0]);

    let stopped_at_first = run(Vec3::ZERO, Vec3::ONE);
    assert_eq!(stopped_at_first.light[1], [0.0, 0.0, 0.0]);
    assert_eq!(stopped_at_first.light[2], [0.0, 0.0, 0.0]);
}

#[test]
fn grate_gap_passes_and_bar_blocks() {
    let light = Area::Rectangle(Rectangle {
        center: Vec3::new(0.0, 0.0, 20.0),
        half_u: Vec3::new(0.4, 0.0, 0.0),
        half_v: Vec3::new(0.0, 0.4, 0.0),
        normal: -Vec3::Z,
        intensity: 100.0,
        color: Vec3::ONE,
    });
    let mut bars = Vec::new();
    bars.extend(quad(
        Vec3::new(-8.0, 2.0, 10.0),
        Vec3::new(16.0, 0.0, 0.0),
        Vec3::new(0.0, 3.0, 0.0),
    ));
    bars.extend(quad(
        Vec3::new(-8.0, -5.0, 10.0),
        Vec3::new(16.0, 0.0, 0.0),
        Vec3::new(0.0, 3.0, 0.0),
    ));
    let gap = other(Vec3::ZERO, Vec3::Z);
    let bar = other(Vec3::new(0.0, 6.0, 0.0), Vec3::Z);
    let colors = solve(&bars, &[gap, bar], &[light], 16).light;
    assert!((colors[0][0] - 0.25).abs() < 1.0e-3, "gap {}", colors[0][0]);
    assert_eq!(colors[1], [0.0, 0.0, 0.0]);
}

#[test]
fn lift_raises_only_an_almost_black_floor() {
    let light = downward(Vec3::new(0.0, 0.0, 24.0), 8_000.0);
    let surface = quad(
        Vec3::new(3.0, -12.0, 0.0),
        Vec3::new(0.0, 24.0, 0.0),
        Vec3::new(0.0, 0.0, 20.0),
    );
    let mut triangles = Vec::new();
    triangles.extend(surface);
    triangles.extend(closed_box(
        Vec3::new(-6.0, -48.0, -2.0),
        Vec3::new(6.0, -32.0, 14.0),
    ));
    let receivers = [
        wall(
            Vec3::new(3.0, 0.0, 10.0),
            -Vec3::X,
            Vec3::new(0.8, 0.75, 0.7),
        ),
        floor(Vec3::ZERO, Vec3::ZERO),
        other(Vec3::ZERO, Vec3::Z),
        floor(Vec3::new(0.0, -40.0, 2.0), Vec3::new(0.9, 0.1, 0.1)),
        other(Vec3::new(0.0, -40.0, 2.0), Vec3::Z),
    ];
    let solved = solve(&triangles, &receivers, &[light], 64);
    assert!(solved.sealed.is_empty());
    assert_eq!(solved.light[1], solved.light[2], "bright floor must stay");
    assert!(solved.light[1][0] > 1.0, "{:?}", solved.light[1]);
    assert_eq!(solved.light[4], [0.0, 0.0, 0.0]);
    assert!(solved.light[3][0] > 0.0, "{:?}", solved.light[3]);
    assert!(solved.light[3][0] < solved.light[1][0]);
    let again = solve(&triangles, &receivers, &[light], 64);
    assert_eq!(solved, again);
}

#[test]
fn outer_room_is_not_a_deaf_shell() {
    let mesh = closed_box(Vec3::ZERO, Vec3::new(80.0, 80.0, 40.0));
    let lamp = downward(Vec3::new(40.0, 40.0, 30.0), 200.0);
    let receiver = other(Vec3::new(40.0, 40.0, 0.0), Vec3::Z);
    let solved = solve(&mesh, &[receiver], &[lamp], 16);
    assert!(solved.sealed.is_empty());
    assert!(solved.light[0][0] > 0.0);
}

#[test]
fn inner_shell_mark_ignores_budget_and_drops_its_light() {
    let mut mesh = closed_box(Vec3::new(-40.0, -40.0, -10.0), Vec3::new(40.0, 40.0, 40.0));
    mesh.extend(closed_box(
        Vec3::new(-5.0, -5.0, -5.0),
        Vec3::new(5.0, 5.0, 5.0),
    ));
    let outside = downward(Vec3::new(20.0, 0.0, 30.0), 300.0);
    let inside = downward(Vec3::ZERO, 300.0);
    let outsider = other(Vec3::new(20.0, 0.0, 0.0), Vec3::Z);
    let insider = other(Vec3::new(0.0, 0.0, -3.0), Vec3::Z);
    let few = solve(&mesh, &[outsider, insider], &[outside, inside], 4);
    let many = solve(&mesh, &[outsider, insider], &[outside, inside], 128);
    let only_outside = solve(&mesh, &[outsider, insider], &[outside], 4);
    assert_eq!(few.sealed, vec![1]);
    assert_eq!(many.sealed, few.sealed);
    assert_eq!(few.light, only_outside.light);
    assert_eq!(few.light[1], [0.0, 0.0, 0.0]);
    assert!(few.light[0][0] > 0.0, "{:?}", few.light[0]);
}
