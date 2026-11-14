
// ashd: supersampled texture minification for zoomed-out canvases.
//
// Textures have no mipmaps, so a texture drawn at 1/k of its size with plain bilinear filtering
// reads only 2x2 of the k x k texels under every output pixel and skips the rest: thin strokes
// (text) drop out or turn jagged. This averages niri_taps x niri_taps bilinear taps spread evenly
// over the pixel's footprint in the texture (taken from the screen-space derivatives of the
// texture coordinates, so buffer scale, viewporter crops and buffer transforms are accounted
// for). With niri_taps = ceil(k) the taps are at most one texel apart, which together with the
// bilinear weights of every tap approximates a box filter of the footprint (what a mipmap or
// area-averaging downscale would give).

#define NIRI_MAX_TAPS 8

uniform float niri_taps;

vec4 niri_minify_sample(vec2 coords) {
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
