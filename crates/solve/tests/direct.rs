use glam::Vec3;
use solve::{dynamic, solve, Area, Disk, Receiver, Rectangle, Role};

fn ceiling_light() -> Area {
    Area::Rectangle(Rectangle {
        center: Vec3::new(0.0, 0.0, 10.0),
        half_u: Vec3::new(5.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 1.0, 0.0),
        normal: -Vec3::Z,
        intensity: 100.0,
        color: Vec3::ONE,
    })
}

fn upward(position: Vec3) -> Receiver {
    Receiver {
        position,
        normal: Vec3::Z,
        albedo: Vec3::ZERO,
        role: Role::Other,
    }
}

fn quad(z: f32, x0: f32, x1: f32, y0: f32, y1: f32) -> Vec<solve::Triangle> {
    let a = Vec3::new(x0, y0, z);
    let b = Vec3::new(x1, y0, z);
    let c = Vec3::new(x1, y1, z);
    let d = Vec3::new(x0, y1, z);
    vec![
        solve::Triangle {
            vertices: [a, b, c],
        },
        solve::Triangle {
            vertices: [a, c, d],
        },
    ]
}

fn light(
    triangles: &[solve::Triangle],
    receivers: &[Receiver],
    area: &Area,
    rays: u32,
) -> Vec<[f32; 3]> {
    solve(triangles, receivers, &[*area], rays).light
}

#[test]
fn dynamic_matches_an_open_patch_and_ignores_occlusion() {
    let area = ceiling_light();
    let receiver = upward(Vec3::ZERO);
    let open = light(&[], &[receiver], &area, 16);
    assert_eq!(dynamic(&[receiver], &[area]), open);

    let blocked = light(&quad(5.0, -20.0, 20.0, -20.0, 20.0), &[receiver], &area, 16);
    assert_eq!(blocked[0], [0.0, 0.0, 0.0]);
    assert_eq!(dynamic(&[receiver], &[area]), open);

    let back = Receiver {
        position: Vec3::ZERO,
        normal: -Vec3::Z,
        albedo: Vec3::ZERO,
        role: Role::Other,
    };
    assert_eq!(dynamic(&[back], &[area]), vec![[0.0, 0.0, 0.0]]);
}

#[test]
fn far_receiver_follows_nearest_point() {
    let area = ceiling_light();
    let near = upward(Vec3::ZERO);
    let far = upward(Vec3::new(9.0, 0.0, 0.0));
    let colors = light(&[], &[near, far], &area, 16);
    let ratio = colors[1][0] / colors[0][0];
    let expected = 100.0 / 116.0;
    assert!(
        (ratio - expected).abs() < 1.0e-4,
        "ratio {ratio}, expected {expected}"
    );
    assert!(colors[1][0] < colors[0][0]);
}

#[test]
fn patch_behind_the_receiver_adds_nothing() {
    let area = ceiling_light();
    let floor = upward(Vec3::ZERO);
    let back = Receiver {
        position: Vec3::ZERO,
        normal: -Vec3::Z,
        albedo: Vec3::ZERO,
        role: Role::Other,
    };
    let colors = light(&[], &[floor, back], &area, 16);
    assert!(colors[0][0] > 0.0, "{:?}", colors[0]);
    assert_eq!(colors[1], [0.0, 0.0, 0.0]);
}

#[test]
fn visibility_is_open_closed_or_partial() {
    let area = Area::Rectangle(Rectangle {
        center: Vec3::new(0.0, 0.0, 10.0),
        half_u: Vec3::new(2.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 0.5, 0.0),
        normal: -Vec3::Z,
        intensity: 100.0,
        color: Vec3::ONE,
    });
    let receiver = upward(Vec3::ZERO);

    let open = light(&[], &[receiver], &area, 16);
    assert!((open[0][0] - 1.0).abs() < 1.0e-4, "{}", open[0][0]);

    let closed = light(&quad(5.0, -20.0, 20.0, -20.0, 20.0), &[receiver], &area, 16);
    assert_eq!(closed[0], [0.0, 0.0, 0.0]);

    let partial = light(&quad(5.0, 0.0, 20.0, -20.0, 20.0), &[receiver], &area, 16);
    let visibility = partial[0][0];
    assert!(
        (0.2..0.8).contains(&visibility),
        "penumbra visibility {visibility}"
    );
}

#[test]
fn same_input_is_bitwise_equal() {
    let area = ceiling_light();
    let receiver = upward(Vec3::new(3.0, 0.5, 0.0));
    let blocker = quad(4.0, 0.0, 2.0, -2.0, 2.0);
    let first = solve(&blocker, &[receiver], &[area], 32);
    let second = solve(&blocker, &[receiver], &[area], 32);
    assert_eq!(first, second);
}

#[test]
fn open_receiver_ignores_budget() {
    let area = ceiling_light();
    let receiver = upward(Vec3::ZERO);
    let few = light(&[], &[receiver], &area, 4);
    let many = light(&[], &[receiver], &area, 64);
    assert_eq!(few, many);
}

#[test]
fn patch_color_scales_each_channel() {
    let Area::Rectangle(mut rectangle) = ceiling_light() else {
        panic!("ceiling light is a rectangle");
    };
    rectangle.color = Vec3::new(0.25, 0.5, 0.0);
    let colors = light(&[], &[upward(Vec3::ZERO)], &Area::Rectangle(rectangle), 8);
    assert!((colors[0][0] - 0.25).abs() < 1.0e-4, "{}", colors[0][0]);
    assert!((colors[0][1] - 0.5).abs() < 1.0e-4, "{}", colors[0][1]);
    assert_eq!(colors[0][2], 0.0);
}

#[test]
fn disk_falls_off_from_the_rim() {
    let disk = Area::Disk(Disk {
        center: Vec3::new(0.0, 0.0, 10.0),
        radius: 2.0,
        normal: -Vec3::Z,
        axis: Vec3::X,
        intensity: 100.0,
        color: Vec3::ONE,
    });
    let under = light(&[], &[upward(Vec3::ZERO)], &disk, 16);
    assert!((under[0][0] - 1.0).abs() < 1.0e-4, "{}", under[0][0]);

    let aside = light(&[], &[upward(Vec3::new(5.0, 0.0, 0.0))], &disk, 16);
    let expected = 100.0 / 109.0;
    assert!(
        (aside[0][0] - expected).abs() < 1.0e-4,
        "{} vs {expected}",
        aside[0][0]
    );
}
