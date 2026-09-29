use glam::Vec3;
use solve::{solve, Receiver, Rectangle};

fn ceiling_light() -> Rectangle {
    Rectangle {
        center: Vec3::new(0.0, 0.0, 10.0),
        half_u: Vec3::new(5.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 1.0, 0.0),
        normal: -Vec3::Z,
        intensity: 100.0,
    }
}

fn upward(position: Vec3) -> Receiver {
    Receiver {
        position,
        normal: Vec3::Z,
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

#[test]
fn far_receiver_follows_nearest_point() {
    let area = ceiling_light();
    let near = upward(Vec3::ZERO);
    let far = upward(Vec3::new(9.0, 0.0, 0.0));
    let colors = solve(&[], &[near, far], &area, 16);
    let ratio = colors[1][0] / colors[0][0];
    let expected = 100.0 / 116.0;
    assert!(
        (ratio - expected).abs() < 1.0e-4,
        "ratio {ratio}, expected {expected}"
    );
    assert!(colors[1][0] < colors[0][0]);
}

#[test]
fn visibility_is_open_closed_or_partial() {
    let area = Rectangle {
        center: Vec3::new(0.0, 0.0, 10.0),
        half_u: Vec3::new(2.0, 0.0, 0.0),
        half_v: Vec3::new(0.0, 0.5, 0.0),
        normal: -Vec3::Z,
        intensity: 100.0,
    };
    let receiver = upward(Vec3::ZERO);

    let open = solve(&[], &[receiver], &area, 16);
    assert!((open[0][0] - 1.0).abs() < 1.0e-4, "{}", open[0][0]);

    let closed = solve(&quad(5.0, -20.0, 20.0, -20.0, 20.0), &[receiver], &area, 16);
    assert_eq!(closed[0], [0.0, 0.0, 0.0]);

    let partial = solve(&quad(5.0, 0.0, 20.0, -20.0, 20.0), &[receiver], &area, 16);
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
    let first = solve(&blocker, &[receiver], &area, 32);
    let second = solve(&blocker, &[receiver], &area, 32);
    assert_eq!(first, second);
}

#[test]
fn open_receiver_ignores_budget() {
    let area = ceiling_light();
    let receiver = upward(Vec3::ZERO);
    let few = solve(&[], &[receiver], &area, 4);
    let many = solve(&[], &[receiver], &area, 64);
    assert_eq!(few, many);
}
