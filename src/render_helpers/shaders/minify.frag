
// ashd: texture minification for zoomed-out canvases.
//
// niri_mode 0 (box): textures have no mipmaps, so a texture drawn at 1/k of its size with plain
// bilinear filtering reads only 2x2 of the k x k texels under every output pixel and skips the
// rest: thin strokes (text) drop out or turn jagged. This averages niri_taps x niri_taps bilinear
// taps spread evenly over the pixel's footprint in the texture (taken from the screen-space
// derivatives of the texture coordinates, so buffer scale, viewporter crops and buffer
// transforms are accounted for). With niri_taps = ceil(k) the taps are at most one texel apart,
// which together with the bilinear weights of every tap approximates a box filter of the
// footprint (what a mipmap or area-averaging downscale would give).
//
// niri_mode 1 (sharp, 1 < k <= 2): a Catmull-Rom kernel stretched to the pixel footprint
// (support 2 output pixels = 2k texels on each side), evaluated on whole texels. niri_k holds k
// along the buffer's x and y axes. The texture size is not a uniform of the texture shaders; it
// follows from k and the derivatives of the normalized texture coordinates (one output pixel
// spans k texels = k / size in normalized units) and is rounded to whole texels, which makes it
// exact. Texel centres are read through the bilinear sampler at exact centres, which returns
// the texel itself.

#define NIRI_MAX_TAPS 8

uniform float niri_taps;
uniform float niri_mode;
uniform vec2 niri_k;

vec4 niri_box_sample(vec2 coords) {
    vec2 dx = dFdx(coords);
    vec2 dy = dFdy(coords);
    float n = niri_taps;
    vec4 sum = vec4(0.0);
    for (int i = 0; i < NIRI_MAX_TAPS; i++) {
        if (float(i) >= n)
            break;
        float fx = (float(i) + 0.5) / n - 0.5;
        for (int j = 0; j < NIRI_MAX_TAPS; j++) {
            if (float(j) >= n)
                break;
            float fy = (float(j) + 0.5) / n - 0.5;
            sum += texture2D(tex, coords + fx * dx + fy * dy);
        }
    }
    return sum / (n * n);
}

// Catmull-Rom (B = 0, C = 0.5) cubic.
float niri_cr(float x) {
    x = abs(x);
    if (x < 1.0)
        return (1.5 * x - 2.5) * x * x + 1.0;
    if (x < 2.0)
        return ((-0.5 * x + 2.5) * x - 4.0) * x + 2.0;
    return 0.0;
}

vec4 niri_sharp_sample(vec2 coords) {
    vec2 per_px = abs(dFdx(coords)) + abs(dFdy(coords));
    vec2 size = floor(niri_k / max(per_px, vec2(1e-9)) + 0.5);
    vec2 p = coords * size - 0.5;
    vec2 base = floor(p);
    vec2 f = p - base;
    vec2 inv_k = 1.0 / niri_k;
    // Taps base - (h - 1) ..= base + h with h = ceil(2k) cover the support |d| < 2k.
    vec2 h = ceil(2.0 * niri_k);

    vec4 sum = vec4(0.0);
    float wsum = 0.0;
    for (int j = 0; j < NIRI_MAX_TAPS; j++) {
        float oy = float(j) - h.y + 1.0;
        if (oy > h.y)
            break;
        float wy = niri_cr((oy - f.y) * inv_k.y);
        for (int i = 0; i < NIRI_MAX_TAPS; i++) {
            float ox = float(i) - h.x + 1.0;
            if (ox > h.x)
                break;
            float w = niri_cr((ox - f.x) * inv_k.x) * wy;
            sum += w * texture2D(tex, (base + vec2(ox, oy) + 0.5) / size);
            wsum += w;
        }
    }
    vec4 color = sum / wsum;
    // The negative lobes can overshoot; keep the premultiplied colour valid.
    color.a = clamp(color.a, 0.0, 1.0);
    color.rgb = clamp(color.rgb, vec3(0.0), vec3(color.a));
    return color;
}

vec4 niri_minify_sample(vec2 coords) {
    if (niri_mode > 0.5)
        return niri_sharp_sample(coords);
    return niri_box_sample(coords);
}
