// angel-niri (inactive_dim.rs): a sparse grid of dark dots over an inactive window: one cell of
// `dim_pixel` logical px in every two by two, starting one cell in so pixel corners stay clean.
precision highp float;

uniform float niri_alpha;
uniform vec2 niri_size;
varying vec2 niri_v_coords;

uniform vec4 dim_color;
uniform float dim_pixel;

void main() {
    vec2 cell = floor(niri_v_coords * niri_size / dim_pixel);
    float on = mod(cell.x, 2.0) * mod(cell.y, 2.0);
    gl_FragColor = dim_color * (on * niri_alpha);
}
