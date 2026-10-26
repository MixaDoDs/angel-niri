#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// angel-niri (desk_throw.rs): the finished frame of an output, broken around an impact like a
// cyberpunk implant glitching — bands torn sideways, the colours split, pixels, flickering broken
// blocks, stuck columns, scanlines. In texels of the captured frame:
uniform vec2 tex_size;
uniform vec2 g_center;   // the impact
uniform float g_reach;   // how far it reaches
uniform float g_power;   // 0 … 1
uniform float g_step;    // the stutter step: everything random changes with it
uniform float g_cell;    // one art pixel

float hash(vec2 p) {
    return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
}

vec3 ink(float h) {
    // the glitch's own colours: hot red, cyan, magenta, acid yellow, white
    if (h < 0.3) return vec3(1.0, 0.0, 0.235);
    if (h < 0.55) return vec3(0.0, 0.94, 1.0);
    if (h < 0.72) return vec3(1.0, 0.1, 0.9);
    if (h < 0.86) return vec3(0.95, 1.0, 0.1);
    return vec3(1.0);
}

void main() {
    vec2 px = v_coords * tex_size;
    vec4 base = texture2D(tex, v_coords);

    // inside: a blobby, blocky area around the impact, wider than tall, its edge jumping
    float B = g_cell * 6.0;
    vec2 blk = floor(px / B);
    float jag = hash(blk + floor(g_step / 3.0) * 0.37);
    vec2 d = (blk * B + B * 0.5 - g_center) * vec2(1.0, 1.7);
    float reach = g_reach * (0.7 + 0.6 * jag);
    float inside = reach - length(d);
    if (inside <= 0.0 || g_power <= 0.0) {
        gl_FragColor = base * alpha;
        return;
    }
    // harder toward the impact
    float k = clamp(inside / (g_reach * 0.45 + 1.0), 0.0, 1.0) * g_power;

    // bands torn sideways, each its own height and shift
    float bandH = g_cell * (2.0 + floor(hash(vec2(floor(px.y / (g_cell * 10.0)), g_step)) * 9.0));
    float band = floor(px.y / bandH);
    float tear = hash(vec2(band, g_step));
    float shift = tear < 0.5 ? (hash(vec2(band, g_step + 7.0)) - 0.5) * 2.0 * g_cell * 40.0 * k : 0.0;
    shift = floor(shift / g_cell) * g_cell;
    vec2 sp = px + vec2(shift, 0.0);

    // to pixels, bigger toward the impact
    float pc = g_cell * (1.0 + floor(k * 3.0));
    sp = (floor(sp / pc) + 0.5) * pc;

    // the colours split
    float ca = g_cell * (1.0 + 5.0 * k);
    vec2 uv = clamp(sp / tex_size, vec2(0.0), vec2(1.0));
    vec2 off = vec2(ca / tex_size.x, 0.0);
    vec4 c = texture2D(tex, uv);
    vec3 col = vec3(texture2D(tex, uv + off).r, c.g, texture2D(tex, uv - off).b);

    // broken blocks, flickering
    float CB = g_cell * 8.0;
    vec2 cb = floor(px / CB);
    float h = hash(cb + vec2(g_step * 1.31, g_step * 0.73));
    if (h > 1.0 - 0.22 * k) {
        float m = hash(cb + 3.1);
        if (m < 0.3)
            col = ink(hash(cb + g_step));
        else if (m < 0.5)
            col = vec3(0.0);
        else if (m < 0.7)
            col = 1.0 - col;
        else
            // smeared: the block's first texel stretched over it
            col = texture2D(tex, clamp(cb * CB / tex_size, vec2(0.0), vec2(1.0))).rgb;
    }

    // stuck columns: a dead LCD line here and there, running through
    float cx = floor(px.x / g_cell);
    if (hash(vec2(cx, 9.0)) > 0.982)
        col = mix(col, ink(hash(vec2(cx, 3.0))), 0.85 * g_power);

    // scanlines
    if (mod(floor(px.y / g_cell), 2.0) < 1.0)
        col *= 1.0 - 0.3 * k;

    vec4 color = vec4(col, 1.0);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    color = color * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
