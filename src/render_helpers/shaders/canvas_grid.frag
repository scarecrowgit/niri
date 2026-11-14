// ashd: canvas plane background (line grid, dot grid) and dashed region outlines.
//
// All lengths in uniforms are in logical units of the element (workspace view pixels); they are
// converted to physical pixels with the element's physical size, so that the pattern stays
// pixel-exact under the output scale and the overview zoom.

precision highp float;

#if defined(DEBUG_FLAGS)
uniform float niri_tint;
#endif

uniform float niri_alpha;
uniform float niri_scale;

uniform vec2 niri_size;
varying vec2 niri_v_coords;

// 0: line grid, 1: dot grid, 2: dashed line along x, 3: dashed line along y.
uniform float mode;
// Logical size of the element.
uniform vec2 elem_size;
// Logical position of a grid intersection (or the dash phase in x).
uniform vec2 grid_offset;
// Logical distance between grid lines / dots.
uniform float spacing;
// Logical line width / dot diameter, and its minimum in physical pixels.
uniform float line_width;
uniform float min_width;
// Logical dash and gap lengths.
uniform vec2 dash;
// Premultiplied colours.
uniform vec4 color;
uniform vec4 bg_color;

void main() {
    vec2 px_per_unit = niri_size / elem_size;
    // Pixel centre in physical pixels relative to the element.
    vec2 p = niri_v_coords * niri_size;
    float width = max(min_width, line_width * px_per_unit.x);

    vec4 result = bg_color;

    if (mode < 1.5) {
        vec2 period = vec2(spacing) * px_per_unit;
        // Fade the pattern out when it gets too dense.
        float fade = clamp((min(period.x, period.y) - 4.0) / 8.0, 0.0, 1.0);
        vec2 d = mod(p - grid_offset * px_per_unit, period);

        float coverage = 0.0;
        if (mode < 0.5) {
            // Lines: one band of `width` pixels per period, snapped to pixel centres.
            if (d.x < width || d.y < width)
                coverage = 1.0;
        } else {
            // Dots: anti-aliased discs centred on the intersections.
            vec2 c = min(d, period - d);
            float dist = length(c);
            coverage = clamp(width * 0.5 - dist + 0.5, 0.0, 1.0);
        }

        vec4 fg = color * (coverage * fade);
        result = fg + result * (1.0 - fg.a);
    } else {
        float t = (mode < 2.5 ? p.x : p.y) + (mode < 2.5 ? grid_offset.x : grid_offset.y) * px_per_unit.x;
        float on = max(1.0, dash.x * px_per_unit.x);
        float off = max(1.0, dash.y * px_per_unit.x);
        if (mod(t, on + off) < on)
            result = color;
        else
            result = vec4(0.0);
    }

    result = result * niri_alpha;

#if defined(DEBUG_FLAGS)
    if (niri_tint == 1.0)
        result = vec4(0.0, 0.2, 0.0, 0.2) + result * 0.8;
#endif

    gl_FragColor = result;
}
