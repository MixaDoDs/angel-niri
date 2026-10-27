// angel-niri (desk_throw.rs): a window in flight between desks, drawn turned by `throw_angle`
// around its middle in whole `throw_pixel` cells — it steps, it doesn't smear. Behind it up to
// three afterimages (`throw_ghostN`: offset x, y from it, their turn, how opaque), dithered in
// the same cells by a 4×4 Bayer: the speed shows without a blur. Where it hit hardest it is
// dented (`throw_dent`: the spot on its edge, how deep, how far along it; `throw_dent_in`: the way
// in, all in its own px): the edge pushed in, the picture crushed up behind it, a dark crease.
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
uniform float throw_pixel;
uniform vec4 throw_ghost0;
uniform vec4 throw_ghost1;
uniform vec4 throw_ghost2;
uniform vec4 throw_dent;
uniform vec2 throw_dent_in;

// where `p` falls on the window placed at `loc` turned by `angle`, in its own px; < 0 outside
vec2 onTile(vec2 p, vec2 loc, float angle) {
    vec2 c = loc + throw_tile_size * 0.5;
    float s = sin(-angle);
    float k = cos(-angle);
    vec2 d = p - c;
    vec2 q = vec2(d.x * k - d.y * s, d.x * s + d.y * k) + throw_tile_size * 0.5;
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

// the dent at `q` (its own px): where to take the picture from (x < 0: pushed in, nothing there)
// and how lit it is
vec3 dented(vec2 q) {
    if (throw_dent.z <= 0.0)
        return vec3(q, 1.0);
    vec2 n = throw_dent_in;
    vec2 t = vec2(-n.y, n.x);
    vec2 d = q - throw_dent.xy;
    float a = dot(d, t);
    float b = dot(d, n);
    float reach = throw_dent.w;
    if (abs(a) >= reach || b >= reach)
        return vec3(q, 1.0);
    float k = 1.0 - (a / reach) * (a / reach);
    float deep = floor(throw_dent.z * k * k / throw_pixel + 0.5) * throw_pixel;
    if (b < deep)
        return vec3(-1.0, -1.0, 0.0);
    // the picture behind it crushed up into what is left, darker the nearer the crease
    float left = (b - deep) / max(reach - deep, 1.0);
    vec2 from = throw_dent.xy + t * a + n * (left * reach);
    if (b - deep < throw_pixel)
        return vec3(from, 0.45);
    float fold = mod(floor(a / (throw_pixel * 3.0)), 2.0) * 0.07;
    float lit = 1.0 - floor((1.0 - left) * 3.0) / 3.0 * 0.22 - fold * (1.0 - left);
    return vec3(from, lit);
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
    vec3 dq = q.x >= 0.0 ? dented(q) : vec3(-1.0);
    if (dq.x >= 0.0)
        color = sampleTile(dq.xy) * vec4(vec3(dq.z), 1.0);
    else if (q.x >= 0.0) {
        // pushed in: what is behind it shows
    } else if (!ghost(p, cell, throw_ghost0, color) && !ghost(p, cell, throw_ghost1, color))
        ghost(p, cell, throw_ghost2, color);
    gl_FragColor = color * niri_alpha;
}
