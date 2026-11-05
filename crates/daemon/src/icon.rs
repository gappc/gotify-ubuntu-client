//! Tray icon pixmaps drawn at runtime: a disc whose colour shows the state,
//! plus a blue dot for unread messages.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconKind {
    Ok,
    Down,
    #[default]
    Offline,
    /// Messages arrive, but the Uptime Kuma status page can't confirm all is up.
    Unsure,
}

const OK: [u8; 3] = [0x33, 0xd1, 0x7a]; // GNOME green, as in the app icon
const DOWN: [u8; 3] = [0xe0, 0x1b, 0x24];
const OFFLINE: [u8; 3] = [0x77, 0x76, 0x7b];
const UNSURE: [u8; 3] = [0xf5, 0xc2, 0x11]; // GNOME yellow
const UNREAD: [u8; 3] = [0x35, 0x84, 0xe4];

/// How much of the pixel centred at (px, py) lies inside the circle (0..1),
/// with a one-pixel soft edge for anti-aliasing.
fn coverage(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}

/// `a` drawn over `b`, both as (alpha 0..1, rgb).
fn over(top: (f32, [u8; 3]), bottom: (f32, [u8; 3])) -> (f32, [u8; 3]) {
    let (ta, tc) = top;
    let (ba, bc) = bottom;
    let a = ta + ba * (1.0 - ta);
    if a == 0.0 {
        return (0.0, [0; 3]);
    }
    let mix = |t: u8, b: u8| ((t as f32 * ta + b as f32 * ba * (1.0 - ta)) / a).round() as u8;
    (a, [mix(tc[0], bc[0]), mix(tc[1], bc[1]), mix(tc[2], bc[2])])
}

pub fn render(kind: IconKind, unread: bool, size: i32) -> ksni::Icon {
    let colour = match kind {
        IconKind::Ok => OK,
        IconKind::Down => DOWN,
        IconKind::Offline => OFFLINE,
        IconKind::Unsure => UNSURE,
    };
    let s = size as f32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut pixel = (coverage(px, py, s / 2.0, s / 2.0, s * 0.4), colour);
            if unread {
                pixel = over((coverage(px, py, s * 0.8, s * 0.2, s * 0.2), UNREAD), pixel);
            }
            let (a, [r, g, b]) = pixel;
            data.extend_from_slice(&[(a * 255.0).round() as u8, r, g, b]);
        }
    }
    ksni::Icon { width: size, height: size, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(icon: &ksni::Icon, x: i32, y: i32) -> [u8; 4] {
        let i = ((y * icon.width + x) * 4) as usize;
        icon.data[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn icon_center_colours() {
        for (kind, rgb) in [(IconKind::Ok, [0x33, 0xd1, 0x7a]), (IconKind::Down, [0xe0, 0x1b, 0x24]), (IconKind::Offline, [0x77, 0x76, 0x7b]), (IconKind::Unsure, [0xf5, 0xc2, 0x11])] {
            let icon = render(kind, false, 44);
            assert_eq!(pixel(&icon, 22, 30), [0xff, rgb[0], rgb[1], rgb[2]], "{kind:?}");
        }
    }

    #[test]
    fn icon_corner_transparent() {
        assert_eq!(pixel(&render(IconKind::Down, false, 44), 0, 43)[0], 0);
    }

    #[test]
    fn unread_dot_blue() {
        let (x, y) = (35, 9); // 80 %, 20 % of 44
        assert_eq!(pixel(&render(IconKind::Ok, true, 44), x, y), [0xff, 0x35, 0x84, 0xe4]);
        assert_ne!(pixel(&render(IconKind::Ok, false, 44), x, y), [0xff, 0x35, 0x84, 0xe4]);
    }

    #[test]
    fn icon_data_len() {
        let icon = render(IconKind::Ok, true, 22);
        assert_eq!((icon.width, icon.height, icon.data.len()), (22, 22, 22 * 22 * 4));
    }
}
