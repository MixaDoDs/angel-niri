// angel-niri (desk_throw.rs): a window in flight between desks, drawn turned by `throw_angle`
// and grown by `throw_zoom` around its middle in whole `throw_pixel` cells — it steps, it doesn't
// smear. Behind it up to three afterimages (`throw_ghostN`: offset x, y from it, their turn, how
// opaque), dithered in the same cells by a 4×4 Bayer: the speed shows without a blur.
precision highp float;

uniform float niri_alpha;
uniform vec2 niri_size;
varying vec2 niri_v_coords;

uniform sampler2D niri_tex;
uniform vec2 throw_tex_size;
uniform mat3 throw_geo_to_tex;
uniform vec2 throw_tile_loc;
uniform vec2 throw_tile_size;
uniform float throw_angle;
uniform float throw_zoom;
uniform float throw_pixel;
uniform vec4 throw_ghost0;
uniform vec4 throw_ghost1;
uniform vec4 throw_ghost2;

// where `p` falls on the window placed at `loc` turned by `angle`, in its own px; < 0 outside
vec2 onTile(vec2 p, vec2 loc, float angle) {
    vec2 c = loc + throw_tile_size * 0.5;
    float s = sin(-angle);
    float k = cos(-angle);
    vec2 d = p - c;
    vec2 q = vec2(d.x * k - d.y * s, d.x * s + d.y * k) / throw_zoom + throw_tile_size * 0.5;
    if (q.x < 0.0 || q.y < 0.0 || q.x >= throw_tile_size.x || q.y >= throw_tile_size.y)
        return vec2(-1.0);
    return q;
}

vec4 sampleTile(vec2 q) {
    vec2 uv = (throw_geo_to_tex * vec3(q / throw_tile_size, 1.0)).xy;
    uv = (floor(uv * throw_tex_size) + 0.5) / throw_tex_size;
    return texture2D(niri_tex, uv);
}

// 4×4 Bayer, 0…1, by formula (no constant arrays: NVIDIA's GL won't compile them)
float bayer(vec2 cell) {
    vec2 a = mod(cell, 2.0);
    vec2 b = mod(floor(cell / 2.0), 2.0);
    float x = 2.0 * a.x + 3.0 * a.y - 4.0 * a.x * a.y;
    float y = 2.0 * b.x + 3.0 * b.y - 4.0 * b.x * b.y;
    return (4.0 * x + y + 0.5) / 16.0;
}

bool ghost(vec2 p, vec2 cell, vec4 g, inout vec4 color) {
    if (g.w <= 0.0 || bayer(cell) >= g.w)
        return false;
    vec2 q = onTile(p, throw_tile_loc + g.xy, g.z);
    if (q.x < 0.0)
        return false;
    color = sampleTile(q);
    return true;
}

void main() {
    vec2 p = niri_v_coords * niri_size;
    vec2 cell = floor(p / throw_pixel);
    p = (cell + 0.5) * throw_pixel;

    vec2 q = onTile(p, throw_tile_loc, throw_angle);
    vec4 color = vec4(0.0);
    if (q.x >= 0.0)
        color = sampleTile(q);
    else if (!ghost(p, cell, throw_ghost0, color) && !ghost(p, cell, throw_ghost1, color))
        ghost(p, cell, throw_ghost2, color);
    gl_FragColor = color * niri_alpha;
}
