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

// angel-niri (desk_throw.rs): the finished frame of an output with a broken monitor's damage, as
// LCD panels break: at the hit a jolt tears the picture sideways and splits its colours; then a
// black blot of leaked crystal with dark veins, running down, a bright ring and a milky patch
// around it, long lines through the whole screen (one pixel or a band of sub-pixel stripes), and
// dead pixels of the monitor's own size, flickering, then calm, then healing one by one; the blot
// pulls back into the hit the way it spread. Laid out on the upright screen in physical pixels.
uniform vec2 tex_size;
uniform vec2 g_screen;
uniform mat3 g_tex_to_screen;
uniform mat3 g_screen_to_tex;
// each hit: x, y (physical px, the edge it hit), its age in s (< 0: none), its seed
uniform vec4 g_h0;
uniform vec4 g_h1;
uniform vec4 g_h2;
uniform vec4 g_h3;
uniform vec4 g_h4;
uniform vec4 g_h5;
uniform float g_step;   // the stutter step: flickering changes with it
uniform float g_px;     // physical px per logical px

// a hit's damage lasts this long, s (desk_throw.rs LIFE_MS)
#define LIFE 6.5
#define PI 3.14159265

// no sin(): large pixel coordinates would band
float hash(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x),
               mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

float fbm(vec2 p) {
    return 0.55 * noise(p) + 0.3 * noise(p * 2.07 + 3.1) + 0.15 * noise(p * 4.3 + 7.7);
}

// a broken line's colours: white, green, magenta, cyan, black, yellow, red
vec3 lineInk(float h) {
    if (h < 0.22) return vec3(1.0);
    if (h < 0.4) return vec3(0.0, 1.0, 0.1);
    if (h < 0.55) return vec3(1.0, 0.0, 0.85);
    if (h < 0.68) return vec3(0.0, 0.9, 1.0);
    if (h < 0.8) return vec3(0.0);
    if (h < 0.9) return vec3(1.0, 0.95, 0.0);
    return vec3(1.0, 0.05, 0.1);
}

// a dead pixel: stuck on one sub-pixel, all of them, or none
vec3 pixInk(float h) {
    if (h < 0.25) return vec3(1.0, 0.0, 0.0);
    if (h < 0.5) return vec3(0.0, 1.0, 0.0);
    if (h < 0.7) return vec3(0.0, 0.25, 1.0);
    if (h < 0.85) return vec3(1.0);
    return vec3(0.0);
}

vec2 toTex(vec2 s) {
    return clamp((g_screen_to_tex * vec3(s / g_screen, 1.0)).xy, vec2(0.0), vec2(1.0));
}

// the jolt of a hit: rows torn sideways, the colours split, gone in 0.45 s
void jolt(vec4 h, vec2 s, inout float shift, inout float split) {
    if (h.z < 0.0 || h.z > 0.45)
        return;
    float j = 1.0 - h.z / 0.45;
    j *= j;
    float rowH = g_px * (3.0 + floor(hash(vec2(floor(s.y / (g_px * 28.0)), g_step + h.w)) * 22.0));
    float row = floor(s.y / rowH);
    if (hash(vec2(row, g_step + h.w * 3.0)) < 0.55)
        shift += (hash(vec2(row, g_step + 7.0 + h.w)) - 0.5) * 2.0 * g_px * 140.0 * j;
    split += g_px * 9.0 * j;
}

// how much of a hit's blot there is: it spreads in 0.7 s and pulls back the same way at the end
float grown(float age) {
    float x = min(clamp(age / 0.7, 0.0, 1.0), clamp((LIFE - age) / 0.7, 0.0, 1.0));
    return 1.0 - (1.0 - x) * (1.0 - x) * (1.0 - x);
}

void damage(vec4 h, vec2 s, inout vec3 col) {
    float age = h.z;
    if (age < 0.0)
        return;
    vec2 c = h.xy;
    float seed = h.w;
    float size = grown(age);
    float R = g_px * (95.0 + 80.0 * hash(vec2(seed, 1.3)));
    vec2 d = s - c;
    float dist = length(d);
    // away from the edge it hit
    float away = c.y < g_screen.y * 0.5 ? 1.0 : -1.0;

    // dead pixels, one physical pixel each: crowded near the hit, a few all over
    vec2 p = floor(s);
    float dens = 0.0004 + 0.02 * clamp(1.0 - dist / (R * 3.5), 0.0, 1.0);
    if (hash(p + seed * 17.13) < dens) {
        float heal = LIFE * (0.3 + 0.65 * hash(p + seed * 3.71));
        if (age < heal && (age > 2.2 || hash(p + vec2(g_step, seed)) > 0.45))
            col = pixInk(hash(p + seed * 9.2));
    }

    // a milky patch around the blot, where the panel is pressed
    float n = fbm(s / (g_px * 55.0) + seed * 5.3);
    float cloud = clamp(1.0 - dist / (R * 2.6 * size + 1.0), 0.0, 1.0);
    col = mix(col, vec3(0.92, 0.9, 1.0), cloud * cloud * 0.35 * n * size);

    // long lines through the whole screen: up and down near the hit, across past it; they
    // flicker at first, then hold, then go one by one
    for (int i = 0; i < 9; i++) {
        float fi = float(i);
        float born = 0.03 + 0.3 * hash(vec2(seed, fi + 30.0));
        float gone = LIFE * (0.35 + 0.55 * hash(vec2(seed, fi + 50.0)));
        if (age < born || age > gone)
            continue;
        if (age < 1.8 && hash(vec2(g_step, fi + seed)) < 0.3)
            continue;
        float w = 1.0 + floor(hash(vec2(seed, fi + 70.0)) * 3.0);
        if (hash(vec2(seed, fi + 90.0)) > 0.75)
            w *= 5.0;
        float h1 = hash(vec2(seed, fi + 10.0));
        if (i < 6) {
            float dx = s.x - floor(c.x + (h1 * 2.0 - 1.0) * g_screen.x * 0.3);
            // a band shows its sub-pixels: stripes of its colours
            if (dx >= 0.0 && dx < w)
                col = lineInk(fract(hash(vec2(seed, fi + 110.0)) + floor(dx) * 0.37));
        } else {
            float dy = s.y - floor(c.y + away * (0.15 + h1 * 0.7) * g_screen.y);
            if (dy >= 0.0 && dy < w)
                col = lineInk(fract(hash(vec2(seed, fi + 130.0)) + floor(dy) * 0.37));
        }
    }

    if (size <= 0.0)
        return;
    // the blot: leaked crystal, black, ragged, running down; dark veins out of it
    vec2 q = d;
    if (d.y * away > 0.0)
        q.y *= 0.7;
    float qd = length(q * vec2(1.0, 1.15));
    float edge = R * size * (0.5 + 0.95 * n);
    bool vein = false;
    for (int i = 0; i < 7; i++) {
        float fi = float(i);
        float a = (0.12 + 0.76 * hash(vec2(seed, fi + 150.0))) * PI;
        vec2 dir = vec2(cos(a), sin(a) * away);
        float along = dot(d, dir);
        float len = R * size * (1.3 + 1.8 * hash(vec2(seed, fi + 170.0)));
        if (along <= 0.0 || along > len)
            continue;
        float wob = (noise(vec2(along / (g_px * 14.0), fi + seed)) - 0.5) * g_px * 22.0;
        float perp = abs(dot(d, vec2(-dir.y, dir.x)) + wob);
        if (perp < g_px * (1.0 + 4.0 * (1.0 - along / len)))
            vein = true;
    }
    if (qd < edge || vein) {
        col = vec3(0.0);
        // the crystal's own dim sheen here and there
        if (qd < edge && hash(floor(s / (g_px * 2.0)) + seed) > 0.992)
            col = vec3(0.16, 0.05, 0.3);
    } else if (qd < edge + g_px * (2.0 + 3.0 * n)) {
        // its rim: a bright ring where the crystal is pushed aside
        col = mix(col, vec3(0.85, 0.75, 1.0), 0.75);
    }
}

void main() {
    vec2 s = (g_tex_to_screen * vec3(v_coords, 1.0)).xy * g_screen;

    float shift = 0.0;
    float split = 0.0;
    jolt(g_h0, s, shift, split);
    jolt(g_h1, s, shift, split);
    jolt(g_h2, s, shift, split);
    jolt(g_h3, s, shift, split);
    jolt(g_h4, s, shift, split);
    jolt(g_h5, s, shift, split);

    vec2 sp = s + vec2(shift, 0.0);
    vec3 col = texture2D(tex, toTex(sp)).rgb;
    if (split > 0.0) {
        col.r = texture2D(tex, toTex(sp + vec2(split, 0.0))).r;
        col.b = texture2D(tex, toTex(sp - vec2(split, 0.0))).b;
    }

    damage(g_h0, s, col);
    damage(g_h1, s, col);
    damage(g_h2, s, col);
    damage(g_h3, s, col);
    damage(g_h4, s, col);
    damage(g_h5, s, col);

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
