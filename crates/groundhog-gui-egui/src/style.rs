//! Shared colours and small drawing helpers.
//!
//! Map overlays use fixed colours that read well over any tile; widgets get
//! their colours from egui's theme. The semantic colours follow the iced
//! frontend's palette loosely, so the two frontends read as the same station.

/// The colour of everything on the tiles that must stay fixed.
pub mod map {
    use egui::Color32;

    /// Home position marker.
    pub const HOME: Color32 = Color32::from_rgb(0x6f, 0xce, 0x94);
    /// Label text over any tile.
    pub const LABEL: Color32 = Color32::WHITE;
    /// Selected vehicle's ring.
    pub const SELECTED: Color32 = Color32::from_rgb(0x4c, 0xb2, 0xff);
    /// The dashed route line.
    pub const ROUTE: Color32 = Color32::from_rgb(0x6f, 0xce, 0x94);
    /// Item markers: default, selected, currently executed, dragged.
    pub const MARKER_DEFAULT: Color32 = Color32::from_rgb(0x8a, 0x9a, 0xc4);
    pub const MARKER_SELECTED: Color32 = Color32::WHITE;
    pub const MARKER_CURRENT: Color32 = Color32::from_rgb(0x6f, 0xce, 0x94);
    pub const MARKER_DRAGGED: Color32 = Color32::from_rgb(0xff, 0xd1, 0x66);
    /// Vehicle arrow: the lit right half and the shaded left half.
    pub const ARROW_RIGHT: Color32 = Color32::from_rgb(0xff, 0x4d, 0x4d);
    pub const ARROW_LEFT: Color32 = Color32::from_rgb(0xc0, 0x38, 0x38);
    /// The arrow's outline and the HUD dial's hairline.
    pub const OUTLINE: Color32 = Color32::from_rgb(0x28, 0x28, 0x28);
    /// HUD dial surface.
    pub const HUD_SURFACE: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 235);
    /// Case (outline) trail brightness and core trail brightness, matching
    /// the iced frontend's two passes.
    pub const TRAIL_CASE_BRIGHTNESS: f32 = 0.4;
    pub const TRAIL_CORE_BRIGHTNESS: f32 = 0.85;
    /// The altitude bands of the trail colouring.
    pub const TRAIL_BANDS: usize = 24;
    pub const TRAIL_BAND_MAX_ALT_M: f32 = 1000.0;

    /// The altitude band `altitude_m` falls into, equally sampled over
    /// [`TRAIL_BANDS`] steps, yellow (ground) → red (high).
    pub fn band_of(altitude_m: f32) -> usize {
        let t = (altitude_m / TRAIL_BAND_MAX_ALT_M).clamp(0.0, 1.0);
        ((t * (TRAIL_BANDS - 1) as f32).round() as usize).min(TRAIL_BANDS - 1)
    }

    /// The colour of one trail band at `brightness` ∈ 0..1 (case…core):
    /// OKLCH with the iced frontend's parameters, hue yellow → red.
    pub fn band_color(band: usize, brightness: f32) -> Color32 {
        const YELLOW_HUE_DEG_DEG: f64 = 90.0;
        const RED_HUE_DEG_DEG: f64 = 380.0;
        const CHROMA_RATIO: f64 = 0.2;

        let t = band as f64 / (TRAIL_BANDS - 1) as f64;
        let hue_deg =
            (YELLOW_HUE_DEG_DEG + t * (RED_HUE_DEG_DEG - YELLOW_HUE_DEG_DEG)).to_radians();

        let c = (brightness as f64) * CHROMA_RATIO;
        let (a, b) = (c * hue_deg.cos(), c * hue_deg.sin());
        let l = brightness as f64;

        // OkLab → linear sRGB.
        let (lr, lg, lb) = {
            let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
            let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
            let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
            (
                4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
                -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
                -0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_,
            )
        };

        let channel = |x: f64| -> u8 {
            let gamma = if x >= 0.0031308 {
                1.055 * x.powf(1.0 / 2.4) - 0.055
            } else {
                12.92 * x
            };
            (gamma * 255.0).round().clamp(0.0, 255.0) as u8
        };
        Color32::from_rgb(channel(lr), channel(lg), channel(lb))
    }
}

/// A trace of how a severity renders, for each log line.
pub mod severity {
    use egui::Color32;
    use mavio::default_dialect::enums::MavSeverity;

    pub const CRITICAL: Color32 = Color32::from_rgb(0xff, 0x4d, 0x4d);
    pub const ERROR: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);
    pub const WARNING: Color32 = Color32::from_rgb(0xff, 0xd1, 0x66);
    pub const INFO: Color32 = Color32::from_rgb(0x4c, 0xb2, 0xff);
    pub const DEBUG: Color32 = Color32::from_rgb(0x8a, 0x9a, 0xac);

    /// The semantic colour of a status text's severity, as the iced frontend
    /// styles it.
    pub fn color(severity: MavSeverity) -> Color32 {
        match severity {
            MavSeverity::Emergency
            | MavSeverity::Alert
            | MavSeverity::Critical
            | MavSeverity::Error => CRITICAL,
            MavSeverity::Warning | MavSeverity::Notice => WARNING,
            MavSeverity::Info => INFO,
            MavSeverity::Debug => DEBUG,
        }
    }
}

/// Toast and notice levels, mirrored from the notifications feature.
pub mod notice {
    use egui::Color32;

    pub const ERROR: Color32 = Color32::from_rgb(0x6e, 0x22, 0x22);
    pub const WARNING: Color32 = Color32::from_rgb(0x6e, 0x5c, 0x22);
    pub const SUCCESS: Color32 = Color32::from_rgb(0x22, 0x5d, 0x35);
    pub const INFO: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x3a);
}
