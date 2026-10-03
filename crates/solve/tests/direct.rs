use glam::Vec3;
use solve::{
    broad_light, dynamic, solve, Area, Cover, Disk, Omni, Receiver, Rectangle, Role, Volume,
    LAMP_REACH,
};

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
fn a_wide_disk_stops_on_the_ceiling_and_a_lamp_does_not() {
    let sun = Area::Disk(Disk {
        center: Vec3::new(0.0, 0.0, 200.0),
        radius: 200.0,
        normal: -Vec3::Z,
        axis: Vec3::X,
        intensity: 8_000_000.0,
        color: Vec3::ONE,
    });
    let floor = upward(Vec3::ZERO);
    let beside = upward(Vec3::new(80.0, 0.0, 0.0));
    let ceiling_face = Receiver {
        position: Vec3::new(0.0, 0.0, 32.0),
        normal: -Vec3::Z,
        albedo: Vec3::ZERO,
        role: Role::Other,
    };
    let ceiling = quad(32.0, -40.0, 40.0, -40.0, 40.0);
    let cover = Cover::new(&ceiling);
    let seen = cover.see(&[floor, beside, ceiling_face], &[sun]);
    assert_eq!(seen, vec![0.0, 1.0, 0.0]);
    let colors = broad_light(&[floor, beside, ceiling_face], &[sun], &seen);
    assert_eq!(colors[0], [0.0, 0.0, 0.0]);
    assert!(colors[1][0] > 0.0, "{:?}", colors[1]);
    assert_eq!(colors[2], [0.0, 0.0, 0.0]);
}

fn high_lamp() -> Area {
    Area::Rectangle(Rectangle {
        center: Vec3::new(0.0, 0.0, 80.0),
        half_u: Vec3::new(5.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 1.0, 0.0),
        normal: -Vec3::Z,
        intensity: 100.0,
        color: Vec3::ONE,
    })
}

#[test]
fn a_small_lamp_ray_stops_on_the_ceiling() {
    let lamp = high_lamp();
    let floor = upward(Vec3::ZERO);
    // Far enough in front of the patch that it is a wall, not the mount.
    let ceiling = quad(30.0, -40.0, 40.0, -40.0, 40.0);
    let seen = Cover::new(&ceiling).see(&[floor], &[lamp]);
    assert_eq!(seen, vec![0.0]);
    assert!(dynamic(&[floor], &[lamp])[0][0] > 0.0);
}

#[test]
fn the_fixture_and_its_mount_do_not_shadow_every_direction() {
    let lamp = ceiling_light();
    let floor = upward(Vec3::ZERO);
    // The glass and the face it is bolted to sit on the patch.
    let shell = quad(9.0, -6.0, 6.0, -2.0, 2.0);
    assert_eq!(Cover::new(&shell).see(&[floor], &[lamp]), vec![1.0]);
    // A corner of the cage, just outside the patch, is still the lamp.
    let corner = quad(6.0, 6.0, 14.0, -4.0, 4.0);
    let aside = upward(Vec3::new(20.0, 0.0, 0.0));
    assert_eq!(Cover::new(&corner).see(&[aside], &[lamp]), vec![1.0]);
    // Farther than the penumbra, and large enough that every sample crosses it.
    let wall = vertical(160.0);
    let beside = upward(Vec3::new(280.0, 0.0, 0.0));
    assert_eq!(Cover::new(&wall).see(&[beside], &[lamp]), vec![0.0]);
}

#[test]
fn a_figure_does_not_cut_the_wall_it_sits_on() {
    let lamp = Area::Omni(Omni {
        center: Vec3::new(10.0, 0.0, 40.0),
        axis_x: Vec3::new(4.0, 0.0, 0.0),
        axis_y: Vec3::new(0.0, 6.0, 0.0),
        axis_z: Vec3::new(0.0, 0.0, 8.0),
        intensity: 18_000.0,
        color: Vec3::ONE,
    });
    let mount = vertical(0.0);
    let beside = Receiver {
        position: Vec3::new(0.0, 40.0, 40.0),
        normal: Vec3::X,
        albedo: Vec3::ZERO,
        role: Role::Other,
    };
    let seen = Cover::new(&mount).see(&[beside], &[lamp]);
    assert!(
        seen[0] > 0.9,
        "the figure cut the wall it sits on: {}",
        seen[0]
    );
    let blocker = vertical(40.0);
    let beyond = Receiver {
        position: Vec3::new(80.0, 0.0, 40.0),
        normal: -Vec3::X,
        albedo: Vec3::ZERO,
        role: Role::Other,
    };
    assert_eq!(Cover::new(&blocker).see(&[beyond], &[lamp]), vec![0.0]);
}

#[test]
fn a_post_beside_the_lamp_does_not_black_the_floor() {
    let lamp = ceiling_light();
    let floor = upward(Vec3::new(60.0, 0.0, 0.0));
    let post = vertical_span(24.0, -2.0, 2.0, 0.0, 16.0);
    let seen = Cover::new(&post).see(&[floor], &[lamp]);
    assert!(
        seen[0] > 0.5,
        "a frame near the lamp blacked the floor: {}",
        seen[0]
    );
}

fn vertical(x: f32) -> Vec<solve::Triangle> {
    vertical_span(x, -80.0, 80.0, -20.0, 80.0)
}

fn vertical_span(x: f32, y0: f32, y1: f32, z0: f32, z1: f32) -> Vec<solve::Triangle> {
    let a = Vec3::new(x, y0, z0);
    let b = Vec3::new(x, y1, z0);
    let c = Vec3::new(x, y1, z1);
    let d = Vec3::new(x, y0, z1);
    vec![
        solve::Triangle { vertices: [a, b, c] },
        solve::Triangle { vertices: [a, c, d] },
    ]
}

#[test]
fn dynamic_matches_an_open_patch_and_ignores_occlusion() {
    let area = ceiling_light();
    let receiver = upward(Vec3::ZERO);
    let open = light(&[], &[receiver], &area, 16);
    assert_eq!(dynamic(&[receiver], &[area]), open);

    let blocked = light(
        &quad(30.0, -40.0, 40.0, -40.0, 40.0),
        &[receiver],
        &high_lamp(),
        16,
    );
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
    let reach = 48.0 * 48.0;
    let expected = (100.0 + reach) / (116.0 + reach);
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
        center: Vec3::new(0.0, 0.0, 40.0),
        half_u: Vec3::new(2.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 0.5, 0.0),
        normal: -Vec3::Z,
        intensity: 100.0,
        color: Vec3::ONE,
    });
    let receiver = upward(Vec3::ZERO);

    let open = light(&[], &[receiver], &area, 16);
    assert!(open[0][0] > 0.0, "{}", open[0][0]);

    let closed = light(&quad(10.0, -20.0, 20.0, -20.0, 20.0), &[receiver], &area, 16);
    assert_eq!(closed[0], [0.0, 0.0, 0.0]);

    let partial = light(&quad(10.0, 0.0, 20.0, -20.0, 20.0), &[receiver], &area, 16);
    let visibility = partial[0][0] / open[0][0];
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
    let scale = 100.0 / (10.0 * 10.0 + 48.0 * 48.0);
    assert!((colors[0][0] - 0.25 * scale).abs() < 1.0e-4, "{}", colors[0][0]);
    assert!((colors[0][1] - 0.5 * scale).abs() < 1.0e-4, "{}", colors[0][1]);
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
    let reach = 48.0 * 48.0;
    let under_expected = 100.0 / (100.0 + reach);
    assert!(
        (under[0][0] - under_expected).abs() < 1.0e-4,
        "{}",
        under[0][0]
    );

    let aside = light(&[], &[upward(Vec3::new(5.0, 0.0, 0.0))], &disk, 16);
    let expected = 100.0 / (109.0 + reach);
    assert!(
        (aside[0][0] - expected).abs() < 1.0e-4,
        "{} vs {expected}",
        aside[0][0]
    );
}

#[test]
fn the_inside_of_a_lamp_stays_lit_through_its_shell() {
    let volume = Area::Volume(Volume {
        center: Vec3::new(0.0, 0.0, 10.0),
        axis_x: Vec3::X * 8.0,
        axis_y: Vec3::Y * 8.0,
        axis_z: Vec3::Z * 8.0,
        intensity: 3000.0,
        color: Vec3::new(1.0, 0.5, 0.25),
    });
    let inside = Receiver {
        position: Vec3::new(0.0, 0.0, 6.0),
        normal: Vec3::Z,
        albedo: Vec3::ZERO,
        role: Role::Other,
    };
    let away = Receiver {
        position: Vec3::new(0.0, 0.0, 6.0),
        normal: -Vec3::Z,
        albedo: Vec3::ZERO,
        role: Role::Other,
    };
    let outside = upward(Vec3::new(0.0, 0.0, -20.0));
    let shell = quad(10.0, -4.0, 4.0, -4.0, 4.0);
    let colors = light(&shell, &[inside, away, outside], &volume, 8);
    let peak = 3000.0 / (LAMP_REACH * LAMP_REACH);
    assert!((colors[0][0] - peak).abs() < 1.0e-3, "{:?}", colors[0]);
    assert!((colors[0][1] - peak * 0.5).abs() < 1.0e-3);
    assert!((colors[0][2] - peak * 0.25).abs() < 1.0e-3);
    assert_eq!(colors[1], [0.0, 0.0, 0.0]);
    assert_eq!(colors[2], [0.0, 0.0, 0.0]);
    let seen = Cover::new(&shell).see(&[inside], &[volume]);
    assert_eq!(seen, vec![1.0]);
}
