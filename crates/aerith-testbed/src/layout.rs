// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
// the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where things go in the window: the plane, and the controls with what a click on them does.
//! Sizes are in the window's logical pixels, as Wayland places surfaces.

/// The plane's rectangle, as `Plane::place` takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Where the plane goes in a window of `window` (width, height): the whole window, or with
/// `letterbox` the largest rectangle of the video's shape, centred. Without the video's size yet,
/// or with a size of zero, the whole window. Never smaller than one pixel a side, which is the
/// smallest plane.
pub fn plane_rect(window: (u32, u32), video: Option<(u32, u32)>, letterbox: bool) -> PlaneRect {
    let (width, height) = (window.0.max(1), window.1.max(1));
    let whole = PlaneRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let Some((video_width, video_height)) = video.filter(|(w, h)| letterbox && *w > 0 && *h > 0)
    else {
        return whole;
    };
    let scale = f64::min(
        f64::from(width) / f64::from(video_width),
        f64::from(height) / f64::from(video_height),
    );
    // Rounded to whole pixels, within the window.
    let side =
        |video: u32, window: u32| ((f64::from(video) * scale).round() as u32).clamp(1, window);
    let (fitted_width, fitted_height) = (side(video_width, width), side(video_height, height));
    PlaneRect {
        x: ((width - fitted_width) / 2) as i32,
        y: ((height - fitted_height) / 2) as i32,
        width: fitted_width,
        height: fitted_height,
    }
}

/// A rectangle, by its corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    fn contains(&self, (x, y): (f32, f32)) -> bool {
        (self.left..=self.right).contains(&x) && (self.top..=self.bottom).contains(&y)
    }

    /// The rectangle with each coordinate multiplied by `scale`.
    pub fn scaled(&self, scale: f32) -> Rect {
        Rect {
            left: self.left * scale,
            top: self.top * scale,
            right: self.right * scale,
            bottom: self.bottom * scale,
        }
    }
}

/// The panel's height, its margin from the window's edges, and its corners' radius.
const PANEL_HEIGHT: f32 = 72.0;
const PANEL_MARGIN: f32 = 24.0;
pub const PANEL_RADIUS: f32 = 16.0;
/// The play/pause button, a disc, and the space around it.
const BUTTON_RADIUS: f32 = 22.0;
const BUTTON_INSET: f32 = 14.0;
/// The seek bar's thickness, and how far from it a click still seeks.
const BAR_THICKNESS: f32 = 6.0;
const BAR_REACH: f32 = 14.0;
/// The space between the button and the bar, and after the bar.
const BAR_GAP: f32 = 20.0;
const BAR_END: f32 = 28.0;

/// The controls' shapes in a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Controls {
    /// The translucent panel behind the rest.
    pub panel: Rect,
    /// The play/pause button's disc: its square.
    pub button: Rect,
    /// The seek bar's track; the part played and the knob are drawn over it.
    pub bar: Rect,
}

impl Controls {
    /// The controls along the bottom of a window of `window` (width, height). In a window too small
    /// for them, the shapes shrink to nothing rather than turn inside out.
    pub fn new(window: (f32, f32)) -> Self {
        let (width, height) = (window.0.max(0.0), window.1.max(0.0));
        let panel_bottom = (height - PANEL_MARGIN).max(0.0);
        let panel = Rect {
            left: PANEL_MARGIN.min(width / 2.0),
            top: (panel_bottom - PANEL_HEIGHT).max(0.0),
            right: (width - PANEL_MARGIN).max(width / 2.0),
            bottom: panel_bottom,
        };
        let middle = (panel.top + panel.bottom) / 2.0;
        let button_left = (panel.left + BUTTON_INSET).min(panel.right);
        let button = Rect {
            left: button_left,
            top: middle - BUTTON_RADIUS,
            right: (button_left + 2.0 * BUTTON_RADIUS).min(panel.right),
            bottom: middle + BUTTON_RADIUS,
        };
        let bar_left = (button.right + BAR_GAP).min(panel.right);
        let bar = Rect {
            left: bar_left,
            top: middle - BAR_THICKNESS / 2.0,
            right: (panel.right - BAR_END).max(bar_left),
            bottom: middle + BAR_THICKNESS / 2.0,
        };
        Self { panel, button, bar }
    }

    /// What a click at `point` asks, if it lands on a control.
    pub fn hit(&self, point: (f32, f32)) -> Option<Hit> {
        if self.button.contains(point) {
            return Some(Hit::PlayPause);
        }
        let reach = Rect {
            top: self.bar.top - BAR_REACH,
            bottom: self.bar.bottom + BAR_REACH,
            ..self.bar
        };
        if reach.contains(point) && self.bar.width() > 0.0 {
            let fraction = (point.0 - self.bar.left) / self.bar.width();
            return Some(Hit::Seek(fraction.clamp(0.0, 1.0)));
        }
        None
    }
}

/// What a click on the controls asks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    PlayPause,
    /// Seek to this fraction of the bar's span.
    Seek(f32),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PlaneRect {
        PlaneRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn the_plane_fills_the_window_unless_letterboxed() {
        assert_eq!(
            plane_rect((1280, 720), Some((320, 240)), false),
            rect(0, 0, 1280, 720)
        );
        assert_eq!(plane_rect((1280, 720), None, true), rect(0, 0, 1280, 720));
    }

    #[test]
    fn a_letterboxed_plane_keeps_the_videos_shape_centred() {
        // A 4:3 video in a 16:9 window: bars at the sides.
        assert_eq!(
            plane_rect((1280, 720), Some((320, 240)), true),
            rect(160, 0, 960, 720)
        );
        // A wide film in a 16:9 window: bars above and below.
        assert_eq!(
            plane_rect((1920, 1080), Some((1920, 800)), true),
            rect(0, 140, 1920, 800)
        );
        // The video's own shape: the whole window.
        assert_eq!(
            plane_rect((1920, 1080), Some((3840, 2160)), true),
            rect(0, 0, 1920, 1080)
        );
    }

    #[test]
    fn the_plane_is_never_empty() {
        assert_eq!(plane_rect((0, 0), None, false), rect(0, 0, 1, 1));
        assert_eq!(plane_rect((0, 0), Some((320, 240)), true), rect(0, 0, 1, 1));
        // A video of no size is no video size.
        assert_eq!(
            plane_rect((640, 480), Some((0, 240)), true),
            rect(0, 0, 640, 480)
        );
        // A sliver of a video still covers a pixel.
        assert_eq!(
            plane_rect((640, 480), Some((1, 100_000)), true),
            rect(319, 0, 1, 480)
        );
    }

    #[test]
    fn the_controls_sit_along_the_bottom() {
        let controls = Controls::new((1280.0, 720.0));
        assert_eq!(
            controls.panel,
            Rect {
                left: 24.0,
                top: 624.0,
                right: 1256.0,
                bottom: 696.0
            }
        );
        assert_eq!(controls.button.left, 38.0);
        assert_eq!(controls.button.width(), 44.0);
        assert_eq!(controls.bar.left, 102.0);
        assert_eq!(controls.bar.right, 1228.0);
        assert_eq!(controls.bar.height(), 6.0);
    }

    #[test]
    fn a_click_plays_pauses_or_seeks() {
        let controls = Controls::new((1280.0, 720.0));
        assert_eq!(controls.hit((60.0, 660.0)), Some(Hit::PlayPause));
        assert_eq!(controls.hit((102.0, 660.0)), Some(Hit::Seek(0.0)));
        assert_eq!(controls.hit((665.0, 670.0)), Some(Hit::Seek(0.5)));
        assert_eq!(controls.hit((1228.0, 650.0)), Some(Hit::Seek(1.0)));
        // Beside the track, within the panel: nothing.
        assert_eq!(controls.hit((1240.0, 660.0)), None);
        // Above the panel: nothing.
        assert_eq!(controls.hit((640.0, 300.0)), None);
    }

    #[test]
    fn controls_in_a_tiny_window_shrink_rather_than_turn_inside_out() {
        for window in [(0.0, 0.0), (10.0, 10.0), (60.0, 40.0)] {
            let controls = Controls::new(window);
            for shape in [controls.panel, controls.bar] {
                assert!(shape.width() >= 0.0 && shape.height() >= 0.0, "{window:?}");
            }
            assert!(controls.button.right >= controls.button.left, "{window:?}");
            // A bar of no width seeks nowhere.
            if controls.bar.width() == 0.0 {
                let on_the_bar = (controls.bar.left, controls.bar.top);
                assert!(!matches!(controls.hit(on_the_bar), Some(Hit::Seek(_))));
            }
        }
    }
}
