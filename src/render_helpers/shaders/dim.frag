// angel-niri (inactive_dim.rs): a vignette of dark dots over an inactive window, `dim_pixel`
// logical px each: a 4×4 Bayer dither from dense at the edges to none in the middle. Cells in the
// window's corners (`dim_corner`, a pixel staircase or a radius) stay clean.
precision highp float;

uniform float niri_alpha;
uniform vec2 niri_size;
varying vec2 niri_v_coords;

uniform vec4 dim_color;
uniform float dim_pixel;
uniform float dim_corner;

// 4×4 Bayer matrix, 0..15
float bayer4(vec2 c) {
    vec2 a = mod(c, 2.0);
    vec2 b = mod(floor(c / 2.0), 2.0);
    return 4.0 * (2.0 * a.x + 3.0 * a.y - 4.0 * a.x * a.y) + (2.0 * b.x + 3.0 * b.y - 4.0 * b.x * b.y);
}

void main() {
    vec2 cell = floor(niri_v_coords * niri_size / dim_pixel);
    vec2 origin = cell * dim_pixel;
    vec2 from_edge = min(origin, niri_size - origin - dim_pixel);

    // the corner cut, one cell wider so no dot hangs past a staircase step
    if (from_edge.x + from_edge.y < dim_corner + dim_pixel) {
        gl_FragColor = vec4(0.0);
        return;
    }

    // 0 at the edges, 1 at 60 % of the way to the middle (clean inside), the frame following the
    // window's shape
    vec2 depth = from_edge / max(niri_size * 0.3, vec2(1.0));
    float t = clamp(min(depth.x, depth.y), 0.0, 1.0);
    float density = 0.75 * (1.0 - t) * (1.0 - t);

    float on = (bayer4(cell) + 0.5) / 16.0 < density ? 1.0 : 0.0;
    gl_FragColor = dim_color * (on * niri_alpha);
}
