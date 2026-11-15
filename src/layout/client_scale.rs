//! ashd: client buffer scale that follows the canvas zoom.
//!
//! A canvas window drawn at zoom `z` used to show a buffer rendered for the output scale `s`,
//! resampled to `z` by the compositor. No resampling kernel makes that sharp: glyphs were
//! rasterized and hinted for a pixel grid that is not the one they end up on.
//!
//! Instead, once the zoom of a canvas has settled, every window on its plane is told (through
//! `wp_fractional_scale_v1` and `wl_surface.preferred_buffer_scale`) that its preferred scale is
//! `s × z`, while its logical size and position stay exactly as they are. A client that follows
//! renders `logical × s × z` pixels with a viewport of the logical size, and the canvas then
//! draws that buffer 1:1 at the zoomed size (see [`crate::render_helpers::canvas_rescale`]): text
//! is rendered natively for the pixels it is shown on. Clients that do not follow keep working as
//! before and are resampled.
//!
//! - The scale is only changed at rest: zoom animations, wheel zooms and pinches keep scaling the
//!   existing buffers, and [`CLIENT_SCALE_SETTLE`] after the zoom stopped changing the new scale is
//!   sent, once per window. Leaving the zoomed state (zoom target 1, strip mode, a window leaving
//!   the plane) restores the output scale right away.
//! - The settled zoom is kept on the grid the protocol can express (`k / 120` of the effective
//!   scale, see [`quantize_zoom`]), so that the buffer the client renders has exactly the size the
//!   canvas draws it at.
//! - The integer `preferred_buffer_scale` is `ceil(s × z)`, at least 1: integer-only clients on a
//!   scale-1 output keep rendering at 1, on a scale-2 output at zoom ≤ 0.5 they render at 1.

use std::time::Duration;

use smithay::output;

/// Time the canvas zoom must stay unchanged before windows are asked to re-render at the zoomed
/// scale. Long enough that a wheel zoom made of several notches (each retargeting the zoom
/// animation) or a pause between two key presses does not re-render every window in between,
/// short enough that the text sharpens right after the motion ends. The zoom animations
/// themselves take 150–300 ms, so the new buffers usually arrive ~250–450 ms after the last
/// input.
pub const CLIENT_SCALE_SETTLE: Duration = Duration::from_millis(120);

/// Denominator of `wp_fractional_scale_v1` scales.
const FRACTIONAL_DENOMINATOR: f64 = 120.;

/// Zooms closer to 1 than this count as 1 (the window keeps the output scale).
const UNZOOMED_EPSILON: f64 = 1e-6;

/// Nearest zoom at which `output_scale × zoom` is a multiple of 1/120, i.e. a scale that
/// `wp_fractional_scale_v1` can express exactly.
pub fn quantize_zoom(zoom: f64, output_scale: f64) -> f64 {
    if !(zoom.is_finite() && output_scale.is_finite() && output_scale > 0.) {
        return zoom;
    }
    let steps = (zoom * output_scale * FRACTIONAL_DENOMINATOR)
        .round()
        .max(1.);
    steps / (FRACTIONAL_DENOMINATOR * output_scale)
}

/// [`quantize_zoom`] kept within `min..=max` (moving one grid step inward if rounding left the
/// range); if the range holds no grid value, the zoom is only clamped.
pub fn quantize_zoom_within(zoom: f64, output_scale: f64, min: f64, max: f64) -> f64 {
    let zoom = zoom.clamp(min, max);
    let q = quantize_zoom(zoom, output_scale);
    if (min - 1e-9..=max + 1e-9).contains(&q) {
        return q;
    }
    let grid = 1. / (FRACTIONAL_DENOMINATOR * output_scale);
    let inward = if q < min { q + grid } else { q - grid };
    if (min - 1e-9..=max + 1e-9).contains(&inward) {
        inward
    } else {
        zoom
    }
}

/// Whether a zoom counts as unzoomed.
pub fn is_unzoomed(zoom: f64) -> bool {
    (zoom - 1.).abs() < UNZOOMED_EPSILON
}

/// Scale to advertise to a window drawn at `zoom` on an output with scale `output`.
pub fn zoomed_client_scale(output: output::Scale, zoom: f64) -> output::Scale {
    if is_unzoomed(zoom) || !zoom.is_finite() || zoom <= 0. {
        return output;
    }
    let fractional = output.fractional_scale() * zoom;
    let fractional = (fractional * FRACTIONAL_DENOMINATOR).round().max(1.) / FRACTIONAL_DENOMINATOR;
    // Tolerate float noise right above an integer (2 × 0.5 must advertise 1, not 2).
    let integer = ((fractional - 1e-6).ceil() as i32).max(1);
    output::Scale::Custom {
        advertised_integer: integer,
        fractional,
    }
}

/// Whether two scales advertise the same values to clients.
pub fn same_scale(a: output::Scale, b: output::Scale) -> bool {
    a.integer_scale() == b.integer_scale() && a.fractional_scale() == b.fractional_scale()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_zoom_is_expressible() {
        // Scale 1: the ashd zoom steps are already on the grid.
        for z in [0.4, 0.5, 0.55, 0.75, 0.9, 1.0] {
            assert!((quantize_zoom(z, 1.) - z).abs() < 1e-12, "{z}");
        }
        // 0.62 is 74.4 / 120.
        assert!((quantize_zoom(0.62, 1.) - 74. / 120.).abs() < 1e-12);
        // Scale 1.25: 1.25 × z must be k / 120.
        let q = quantize_zoom(0.8, 1.25);
        assert!(((q * 1.25 * 120.) - (q * 1.25 * 120.).round()).abs() < 1e-9);
        assert!((q - 0.8).abs() <= 1. / 300.);
        // Scale 2: grid of 1 / 240.
        assert!((quantize_zoom(0.62, 2.) - 149. / 240.).abs() < 1e-12);
    }

    #[test]
    fn quantized_zoom_stays_in_range() {
        // 0.62 rounds down to 0.6167, below the minimum: one step up.
        let q = quantize_zoom_within(0.62, 1., 0.62, 1.);
        assert!((q - 75. / 120.).abs() < 1e-12);
        // No grid value in the range: clamped only.
        assert_eq!(quantize_zoom_within(0.621, 1., 0.6201, 0.6202), 0.6202);
        assert_eq!(quantize_zoom_within(5., 1., 0.4, 1.), 1.);
    }

    #[test]
    fn zoomed_scale_values() {
        let s = zoomed_client_scale(output::Scale::Integer(1), 0.9);
        assert_eq!(s.fractional_scale(), 0.9);
        assert_eq!(s.integer_scale(), 1);

        let s = zoomed_client_scale(output::Scale::Integer(2), 0.5);
        assert_eq!(s.fractional_scale(), 1.);
        assert_eq!(s.integer_scale(), 1);

        let s = zoomed_client_scale(output::Scale::Integer(2), 0.9);
        assert_eq!(s.fractional_scale(), 1.8);
        assert_eq!(s.integer_scale(), 2);

        let s = zoomed_client_scale(output::Scale::Fractional(1.25), 0.8);
        assert_eq!(s.fractional_scale(), 1.);
        assert_eq!(s.integer_scale(), 1);

        // Tiny zooms still advertise a positive scale.
        let s = zoomed_client_scale(output::Scale::Integer(1), 0.001);
        assert_eq!(s.fractional_scale(), 1. / 120.);
        assert_eq!(s.integer_scale(), 1);

        // Zoom 1 keeps the output scale as it is.
        let out = output::Scale::Fractional(1.5);
        assert!(same_scale(zoomed_client_scale(out, 1.), out));
        assert!(matches!(
            zoomed_client_scale(out, 1.),
            output::Scale::Fractional(_)
        ));
    }
}
