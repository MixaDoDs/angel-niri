// angel-niri (desk_throw.rs): a window in flight between desks, drawn turned by `throw_angle`
// around its middle in whole `throw_pixel` cells — it steps, it doesn't smear.
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

void main() {
    vec2 p = niri_v_coords * niri_size;
    p = (floor(p / throw_pixel) + 0.5) * throw_pixel;
    vec2 c = throw_tile_loc + throw_tile_size * 0.5;
    float s = sin(-throw_angle);
    float k = cos(-throw_angle);
    vec2 d = p - c;
    vec2 q = vec2(d.x * k - d.y * s, d.x * s + d.y * k) + throw_tile_size * 0.5;
    if (q.x < 0.0 || q.y < 0.0 || q.x >= throw_tile_size.x || q.y >= throw_tile_size.y) {
        gl_FragColor = vec4(0.0);
        return;
    }
    vec2 uv = (throw_geo_to_tex * vec3(q / throw_tile_size, 1.0)).xy;
    uv = (floor(uv * throw_tex_size) + 0.5) / throw_tex_size;
    gl_FragColor = texture2D(niri_tex, uv) * niri_alpha;
}
