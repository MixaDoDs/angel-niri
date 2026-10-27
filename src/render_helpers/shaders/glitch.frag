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
// LCD panels break: at a hit the screen jolts and a tear shifts rows sideways, splitting colours;
// then black blots of leaked crystal with dark veins, running down, that run together into one
// as they meet (one field, smoothly joined), the picture around them pushed aside and fringed
// like a pressed panel, a bright rim, a milky patch; long lines through the whole screen (one
// pixel or a band of sub-pixel stripes) and dead pixels of the monitor's own size, flickering,
// then calm; pieces of the window that hit, stuck where the picture froze at the hit, torn
// sideways, in fewer shades, settling back in steps. The dead pixels and lines heal first, one by one; a couple of seconds later the blots
// pull back into their hits. Every hit makes every blot bigger; they heal together. Laid out on the
// upright screen in physical pixels. Over it all the glass is cracked from each hit.
uniform vec2 tex_size;
// the frames kept at the hits (desk_throw.rs GlitchFrame::snap), the same way round as tex
uniform sampler2D g_snap;
uniform float g_snap_on;
uniform vec2 g_screen;
uniform mat3 g_tex_to_screen;
uniform mat3 g_screen_to_tex;
// each blot: x, y (physical px, the edge it hit), its age in s (< 0: none), its seed
uniform vec4 g_h0;
uniform vec4 g_h1;
uniform vec4 g_h2;
uniform vec4 g_h3;
uniform vec4 g_h4;
uniform vec4 g_h5;
// how many later hits swelled each blot (g_h0…2, g_h3…5)
uniform vec3 g_wa;
uniform vec3 g_wb;
// how hard each hit was, 0…1 (g_h0…2, g_h3…5): below BLOT only cracks, fewer and shorter
uniform vec3 g_pa;
uniform vec3 g_pb;
// the latest hit's age (s, < 0: none), how far the screen shakes (physical px), which way (the
// inward normal of the edge it hit)
uniform vec4 g_jolt;
uniform float g_step;   // the stutter step: flickering changes with it
uniform float g_px;     // physical px per logical px
uniform float g_grow;   // every blot this much bigger for the hits so far
uniform float g_left;   // s until it has all healed: the hits heal together
#define PI 3.14159265
// the blots pull back over the last this many s
#define BLOT_HEAL 1.8
// the dead pixels and lines are gone this long before the end, healing over the span before
#define PIX_END 4.3
#define PIX_SPAN 2.0
// a hit this hard leaks a blot, lines and stuck pieces too
#define BLOT 0.55
// the cracks: rays round the hit, straight bits in each
#define RAYS 11
#define SEGS 5

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

float smin(float a, float b, float k) {
    float h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
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

// a pressed panel's fringe, in steps: violet, blue, cyan, green, yellow, red
vec3 fringeInk(float i) {
    float h = fract(i / 6.0) * 6.0;
    if (h < 1.0) return vec3(0.6, 0.2, 1.0);
    if (h < 2.0) return vec3(0.15, 0.35, 1.0);
    if (h < 3.0) return vec3(0.0, 0.95, 1.0);
    if (h < 4.0) return vec3(0.2, 1.0, 0.3);
    if (h < 5.0) return vec3(1.0, 0.95, 0.1);
    return vec3(1.0, 0.2, 0.2);
}

vec2 toTex(vec2 s) {
    return clamp((g_screen_to_tex * vec3(s / g_screen, 1.0)).xy, vec2(0.0), vec2(1.0));
}

// how much of a blot there is: it spreads in 0.7 s and pulls back over the last BLOT_HEAL s, in
// fits — steps with the stutter, now and then a little back out — not a smooth shrink
float grown(float age) {
    float back = clamp(g_left / BLOT_HEAL, 0.0, 1.0);
    if (back < 1.0)
        back = clamp(floor(back * 7.0 + hash(vec2(g_step, 4.2)) * 0.9) / 7.0, 0.0, 1.0);
    float x = min(clamp(age / 0.7, 0.0, 1.0), back);
    return 1.0 - (1.0 - x) * (1.0 - x) * (1.0 - x);
}

float radius(vec4 h, float w) {
    return g_px * (60.0 + 50.0 * hash(vec2(h.w, 1.3))) * g_grow * (1.0 + 0.25 * w);
}

// away from the edge it hit
float away(vec4 h) {
    return h.y < g_screen.y * 0.5 ? 1.0 : -1.0;
}

// a blot's share of the field: `v` (< 0 inside, all blots joined), which way is out of it, how
// big the biggest is, and whether `s` is on one of its veins
void blot(vec4 h, float w, vec2 s, inout float v, inout vec2 out_, inout float reach,
          inout bool vein) {
    if (h.z < 0.0)
        return;
    float size = grown(h.z);
    if (size <= 0.0)
        return;
    float seed = h.w;
    float R = radius(h, w);
    float up = away(h);
    vec2 d = s - h.xy;
    vec2 q = d;
    if (d.y * up > 0.0)
        q.y *= 0.7;
    float qd = length(q * vec2(1.0, 1.15));
    float n = fbm(s / (g_px * 55.0) + seed * 5.3);
    float vi = qd - R * size * (0.5 + 0.95 * n);
    v = smin(v, vi, g_px * 45.0);
    out_ += d / (length(d) + 1.0) / (1.0 + max(vi, 0.0) / (g_px * 30.0));
    reach = max(reach, R * size);

    for (int i = 0; i < 7; i++) {
        float fi = float(i);
        float a = (0.12 + 0.76 * hash(vec2(seed, fi + 150.0))) * PI;
        vec2 dir = vec2(cos(a), sin(a) * up);
        float along = dot(d, dir);
        float len = R * size * (1.3 + 1.8 * hash(vec2(seed, fi + 170.0)));
        if (along <= 0.0 || along > len)
            continue;
        float wob = (noise(vec2(along / (g_px * 14.0), fi + seed)) - 0.5) * g_px * 22.0;
        float perp = abs(dot(d, vec2(-dir.y, dir.x)) + wob);
        if (perp < g_px * (1.0 + 4.0 * (1.0 - along / len)))
            vein = true;
    }
}

// pieces of the window that hit, stuck: blocks of the frame kept at the hit, a coarse mosaic of
// its colours, crowding around the blot's rim (`v`: the joined blots' field, < 0 inside); torn
// off their place at first and sliding back in steps as the panel settles, now and then a
// channel off; they heal one by one, a little before the dead pixels are all gone
void stuck(vec4 h, float w, vec2 s, float v, inout vec3 col) {
    if (h.z < 0.0 || g_snap_on < 0.5 || v < 0.0)
        return;
    float seed = h.w;
    float R = radius(h, w);
    // rows of their own heights, cut into blocks of their own widths
    float band = floor(s.y / (g_px * 30.0));
    float rowH = g_px * (8.0 + floor(hash(vec2(band, seed + 3.0)) * 20.0));
    float row = floor(s.y / rowH);
    float colW = g_px * (20.0 + floor(hash(vec2(row, seed + 5.0)) * 70.0));
    vec2 blk = vec2(floor((s.x + hash(vec2(row, seed)) * colW) / colW), row);
    float near = 1.0 - length((s - h.xy) / vec2(R * 3.4, R * 2.4));
    float rim = clamp(1.0 - v / (R * 1.2), 0.0, 1.0);
    if (near <= 0.0 || hash(blk + seed * 1.7) > 0.35 * near + 0.35 * rim * near)
        return;
    if (g_left < PIX_END + 0.3 + PIX_SPAN * hash(blk + seed * 2.9))
        return;
    if (h.z < 0.6 && hash(blk + vec2(g_step, seed)) < 0.3)
        return;
    float settle = floor(clamp(h.z / 2.4, 0.0, 1.0) * 6.0) / 6.0;
    vec2 off = (vec2(hash(blk + seed * 4.1), hash(blk + seed * 6.3)) - 0.5) * vec2(180.0, 60.0);
    off = floor(off * (1.0 - settle) / 2.0) * 2.0 * g_px;
    float cell = g_px * 6.0;
    vec2 at = (floor((s + off) / cell) + 0.5) * cell;
    vec3 p = texture2D(g_snap, toTex(at)).rgb;
    // the app's colours, not its dark title bar
    if (dot(p, vec3(0.3, 0.59, 0.11)) < 0.12)
        return;
    p = min(floor(p * 4.0 + 0.5) / 4.0 * 1.15, vec3(1.0));
    float k = hash(blk + seed * 8.8);
    if (k < 0.12)
        p = p.brg;
    else if (k < 0.16)
        p = vec3(1.0) - p;
    col = p;
}

// the nearest crack line so far to `p`: segment a→b, signed by its side (the lit side > 0), its
// half width for a line one physical pixel thick, and how far out along its ray it is, 0…1
void crackSeg(vec2 p, vec2 a, vec2 b, float out0, float out1, inout float line,
              inout float half_, inout float tip) {
    vec2 ab = b - a;
    float ll = max(dot(ab, ab), 1e-4);
    float k = clamp(dot(p - a, ab) / ll, 0.0, 1.0);
    float dist = length(p - a - ab * k);
    if (dist >= abs(line))
        return;
    float side = ab.x * (p.y - a.y) - ab.y * (p.x - a.x);
    line = side >= 0.0 ? dist : -dist;
    half_ = 0.5 * max(abs(ab.x), abs(ab.y)) / sqrt(ll) + 0.02;
    tip = mix(out0, out1, k);
}

// the web between two neighbouring rays: chords from one's bend to the other's, bowed out a little
void crackChord(vec2 p, vec2 q, vec2 r, float seed, float fi, float fj, float ring,
                inout float line, inout float half_, inout float tip) {
    if (hash(vec2(seed + fj * 5.1, fi + 400.0)) < 0.3)
        return;
    vec2 m = (q + r) * 0.5;
    m += normalize(m + vec2(1e-3)) * length(r - q) * (0.04 + 0.08 * hash(vec2(seed + fj, fi + 420.0)));
    crackSeg(p, q, m, ring, ring, line, half_, tip);
    crackSeg(p, m, r, ring, ring, line, half_, tip);
}

// the glass cracked at the hit, all round it: jagged rays of straight bits with sharp bends,
// branching off now and then and breaking up at their tips; a web of chords between them; in the
// middle the glass crushed into little shards, each its own bit of the picture, a little off and
// lit its own way. Lines one physical pixel thick: dark on the picture, catching the light on the
// black of the leaked crystal. They shoot out in the first 0.12 s and go with the blot.
// the hit `h` if it was hard enough for a blot, else none
vec4 hard(vec4 h, float p) {
    return p >= BLOT ? h : vec4(h.xy, -1.0, h.w);
}

void cracks(vec4 h, float w, float power, vec2 s, float v, inout vec3 col) {
    if (h.z < 0.0)
        return;
    float size = grown(h.z);
    if (size <= 0.0)
        return;
    float seed = h.w;
    vec2 d = s - h.xy;
    float r = length(d);
    float full = radius(h, w) * (2.6 + 1.2 * hash(vec2(seed, 7.7))) * size * mix(0.3, 1.0, power);
    if (r > full * clamp(h.z / 0.12, 0.0, 1.0))
        return;
    float pa = atan(d.y, d.x);
    bool dark = v < 0.0;

    // the crushed middle: shards, their edges cracks too
    float core = full * 0.14 * (0.8 + 0.4 * noise(vec2(pa * 2.0 + seed, seed)));
    if (power >= BLOT && r < core * 1.4) {
        float cs = g_px * 9.0;
        vec2 g = d / cs;
        vec2 gi = floor(g);
        float n1 = 1e5;
        float n2 = 1e5;
        vec2 id = vec2(0.0);
        vec2 pt = vec2(0.0);
        for (int y = -1; y <= 1; y++) {
            for (int x = -1; x <= 1; x++) {
                vec2 c = gi + vec2(float(x), float(y));
                vec2 o = c + vec2(hash(c + seed), hash(c + seed + 3.1));
                float dd = length(g - o);
                if (dd < n1) {
                    n2 = n1;
                    n1 = dd;
                    id = c;
                    pt = o;
                } else if (dd < n2) {
                    n2 = dd;
                }
            }
        }
        if (length(pt) * cs < core) {
            if ((n2 - n1) * cs < 1.0) {
                col = dark ? vec3(0.62, 0.58, 0.72) : mix(col, vec3(0.04), 0.85);
                return;
            }
            float lit = hash(id + seed * 2.3);
            if (dark) {
                if (lit > 0.6)
                    col = vec3(0.1, 0.08, 0.15) * (lit - 0.4) * 2.5;
            } else {
                vec2 shift = floor((vec2(hash(id + seed * 4.7), hash(id + seed * 5.9)) - 0.5)
                                 * g_px * 8.0);
                col = texture2D(tex, toTex(s + shift)).rgb * (0.78 + 0.4 * lit);
            }
            return;
        }
    }

    float line = 1e5;
    float half_ = 0.5;
    float tip = 0.0;
    // the first ray's bends (the web closes round) and the previous one's
    vec2 f1 = vec2(0.0);
    vec2 f2 = vec2(0.0);
    vec2 f3 = vec2(0.0);
    vec2 q1 = vec2(0.0);
    vec2 q2 = vec2(0.0);
    vec2 q3 = vec2(0.0);
    float first = 0.0;
    float prev = 0.0;
    for (int i = 0; i < RAYS; i++) {
        float fi = float(i);
        float base = (fi + 0.8 * hash(vec2(seed, fi + 200.0))) / float(RAYS) * 2.0 * PI;
        float len = full * (0.55 + 0.45 * hash(vec2(seed, fi + 220.0)));
        float off = abs(mod(pa - base + PI, 2.0 * PI) - PI);
        // a light hit: a few rays only
        bool near = (off < 0.75 || r < g_px * 16.0) && fi < mix(3.0, float(RAYS), power);
        vec2 a = vec2(0.0);
        vec2 p1 = vec2(0.0);
        vec2 p2 = vec2(0.0);
        vec2 p3 = vec2(0.0);
        for (int j = 0; j < SEGS; j++) {
            float fj = float(j);
            // its bits keep to its way, each bent off it its own way: sharp bends, no drift
            float dir = base + (hash(vec2(seed + fj * 1.7, fi + 260.0)) - 0.5) * 0.7;
            float l = len / float(SEGS) * (0.7 + 0.6 * hash(vec2(seed + fj * 2.3, fi + 280.0)));
            vec2 b = a + l * vec2(cos(dir), sin(dir));
            if (near) {
                crackSeg(d, a, b, fj / float(SEGS), (fj + 1.0) / float(SEGS), line, half_, tip);
                if ((j == 1 || j == 3) && hash(vec2(seed + fj, fi + 300.0)) < 0.55) {
                    float turn = hash(vec2(seed + fj, fi + 320.0)) < 0.5 ? -1.0 : 1.0;
                    float ba = dir + turn * (0.5 + 0.45 * hash(vec2(seed + fj, fi + 340.0)));
                    float bl = len * (0.1 + 0.1 * hash(vec2(seed + fj, fi + 360.0)));
                    vec2 c = b + bl * vec2(cos(ba), sin(ba));
                    float bb = ba + (hash(vec2(seed + fj, fi + 380.0)) - 0.5) * 0.6;
                    vec2 e = c + bl * 0.8 * vec2(cos(bb), sin(bb));
                    float at = (fj + 1.0) / float(SEGS);
                    crackSeg(d, b, c, at, at + 0.2, line, half_, tip);
                    crackSeg(d, c, e, at + 0.2, at + 0.4, line, half_, tip);
                }
            }
            if (j == 0)
                p1 = b;
            else if (j == 1)
                p2 = b;
            else if (j == 2)
                p3 = b;
            a = b;
        }
        if (i == 0) {
            f1 = p1;
            f2 = p2;
            f3 = p3;
            first = base;
        } else if (power > 0.45 && abs(mod(pa - (prev + base) * 0.5 + PI, 2.0 * PI) - PI) < 0.9) {
            crackChord(d, q1, p1, seed, fi, 0.0, 0.25, line, half_, tip);
            crackChord(d, q2, p2, seed, fi, 1.0, 0.45, line, half_, tip);
            crackChord(d, q3, p3, seed, fi, 2.0, 0.65, line, half_, tip);
        }
        q1 = p1;
        q2 = p2;
        q3 = p3;
        prev = base;
    }
    // the last ray to the first, round the circle
    if (power > 0.45 && abs(mod(pa - (prev + first + 2.0 * PI) * 0.5 + PI, 2.0 * PI) - PI) < 0.9) {
        crackChord(d, q1, f1, seed, float(RAYS), 0.0, 0.25, line, half_, tip);
        crackChord(d, q2, f2, seed, float(RAYS), 1.0, 0.45, line, half_, tip);
        crackChord(d, q3, f3, seed, float(RAYS), 2.0, 0.65, line, half_, tip);
    }

    // the contact itself: a white star of crushed glass flashing for a few frames
    if (h.z < 0.1 && r < g_px * (8.0 + 46.0 * h.z / 0.1)
            && hash(floor(s / (g_px * 2.0)) + vec2(g_step, seed)) < 0.6 * (1.0 - h.z / 0.1) + 0.2)
        col = mix(col, vec3(1.0), 0.9 * (1.0 - h.z / 0.1));
    float t = abs(line);
    // out at the tips the line breaks up into single pixels
    if (tip > 0.65 && hash(floor(s) + seed) < (tip - 0.65) * 2.2)
        return;
    if (t < half_)
        col = dark ? vec3(0.62, 0.58, 0.72) : mix(col, vec3(0.04), 0.85);
    else if (!dark && line > 0.0 && t < half_ + 1.0)
        col = mix(col, vec3(1.0), 0.45);
}

// a hit's dead pixels, milky patch and lines
void marks(vec4 h, float w, vec2 s, inout vec3 col) {
    float age = h.z;
    if (age < 0.0)
        return;
    vec2 c = h.xy;
    float seed = h.w;
    float size = grown(age);
    float R = radius(h, w);
    float dist = length(s - c);

    // dead pixels, one physical pixel each: crowded near the hit, a few all over; flickering at
    // first, then still, then healing one by one before the blots do
    vec2 p = floor(s);
    float dens = 0.0004 + 0.02 * clamp(1.0 - dist / (R * 3.5), 0.0, 1.0);
    if (hash(p + seed * 17.13) < dens) {
        float heal = PIX_END + PIX_SPAN * hash(p + seed * 3.71);
        if (g_left > heal && (age > 2.2 || hash(p + vec2(g_step, seed)) > 0.45))
            col = pixInk(hash(p + seed * 9.2));
    }

    // a milky patch around the blot, where the panel is pressed
    float n = fbm(s / (g_px * 55.0) + seed * 5.3);
    float cloud = clamp(1.0 - dist / (R * 2.6 * size + 1.0), 0.0, 1.0);
    col = mix(col, vec3(0.92, 0.9, 1.0), cloud * cloud * 0.35 * n * size);

    // long lines through the whole screen: up and down near the hit, across past it; they
    // flicker at first, then hold, then go one by one with the dead pixels
    for (int i = 0; i < 6; i++) {
        float fi = float(i);
        float born = 0.03 + 0.3 * hash(vec2(seed, fi + 30.0));
        float gone = PIX_END + PIX_SPAN * hash(vec2(seed, fi + 50.0));
        if (age < born || g_left < gone)
            continue;
        if (age < 1.8 && hash(vec2(g_step, fi + seed)) < 0.3)
            continue;
        float lw = 1.0 + floor(hash(vec2(seed, fi + 70.0)) * 3.0);
        if (hash(vec2(seed, fi + 90.0)) > 0.75)
            lw *= 5.0;
        float h1 = hash(vec2(seed, fi + 10.0));
        if (i < 4) {
            float dx = s.x - floor(c.x + (h1 * 2.0 - 1.0) * g_screen.x * 0.3);
            // a band shows its sub-pixels: stripes of its colours
            if (dx >= 0.0 && dx < lw)
                col = lineInk(fract(hash(vec2(seed, fi + 110.0)) + floor(dx) * 0.37));
        } else {
            float dy = s.y - floor(c.y + away(h) * (0.15 + h1 * 0.7) * g_screen.y);
            if (dy >= 0.0 && dy < lw)
                col = lineInk(fract(hash(vec2(seed, fi + 130.0)) + floor(dy) * 0.37));
        }
    }
}

void main() {
    vec2 s = (g_tex_to_screen * vec3(v_coords, 1.0)).xy * g_screen;

    // the latest hit: the whole screen knocked away from the edge hit and shaking back, the
    // harder the further; a hard one tears rows sideways too
    vec2 sp = s;
    float split = 0.0;
    float ja = g_jolt.x;
    if (ja >= 0.0 && ja < 0.5) {
        float j = 1.0 - ja / 0.5;
        j *= j;
        float step2 = 2.0 * g_px;
        sp += g_jolt.zw * floor(g_jolt.y * j * cos(ja * 2.0 * PI * 7.0) / step2 + 0.5) * step2;
        float tear = clamp(g_jolt.y / (g_px * 12.0) - 0.4, 0.0, 1.0) * j;
        float rowH = g_px * (3.0 + floor(hash(vec2(floor(s.y / (g_px * 28.0)), g_step)) * 22.0));
        float row = floor(s.y / rowH);
        if (hash(vec2(row, g_step)) < 0.55)
            sp.x += (hash(vec2(row, g_step + 7.0)) - 0.5) * 2.0 * g_px * 140.0 * tear;
        split += g_px * 9.0 * tear;
    }

    // the blots, joined
    float v = 1e6;
    vec2 out_ = vec2(0.0);
    float reach = 0.0;
    bool vein = false;
    blot(hard(g_h0, g_pa.x), g_wa.x, s, v, out_, reach, vein);
    blot(hard(g_h1, g_pa.y), g_wa.y, s, v, out_, reach, vein);
    blot(hard(g_h2, g_pa.z), g_wa.z, s, v, out_, reach, vein);
    blot(hard(g_h3, g_pb.x), g_wb.x, s, v, out_, reach, vein);
    blot(hard(g_h4, g_pb.y), g_wb.y, s, v, out_, reach, vein);
    blot(hard(g_h5, g_pb.z), g_wb.z, s, v, out_, reach, vein);

    // around them the picture is pushed aside, wobbling, its colours split and fringed
    float press = 0.0;
    if (reach > 0.0 && v > 0.0) {
        press = clamp(1.0 - v / (reach * 0.6 + g_px * 20.0), 0.0, 1.0);
        press *= press;
        vec2 dir = length(out_) > 0.0 ? normalize(out_) : vec2(0.0);
        sp -= dir * g_px * 26.0 * press;
        sp += (vec2(noise(s / (g_px * 9.0)), noise(s / (g_px * 9.0) + 5.7)) - 0.5) * g_px * 12.0 * press;
        split += g_px * 7.0 * press;
    }

    vec3 col = texture2D(tex, toTex(sp)).rgb;
    if (split > 0.0) {
        col.r = texture2D(tex, toTex(sp + vec2(split, 0.0))).r;
        col.b = texture2D(tex, toTex(sp - vec2(split, 0.0))).b;
    }
    if (press > 0.15)
        col = mix(col, fringeInk(floor(v / (g_px * 4.0))), 0.22 * press);

    stuck(hard(g_h0, g_pa.x), g_wa.x, s, v, col);
    stuck(hard(g_h1, g_pa.y), g_wa.y, s, v, col);
    stuck(hard(g_h2, g_pa.z), g_wa.z, s, v, col);
    stuck(hard(g_h3, g_pb.x), g_wb.x, s, v, col);
    stuck(hard(g_h4, g_pb.y), g_wb.y, s, v, col);
    stuck(hard(g_h5, g_pb.z), g_wb.z, s, v, col);

    marks(hard(g_h0, g_pa.x), g_wa.x, s, col);
    marks(hard(g_h1, g_pa.y), g_wa.y, s, col);
    marks(hard(g_h2, g_pa.z), g_wa.z, s, col);
    marks(hard(g_h3, g_pb.x), g_wb.x, s, col);
    marks(hard(g_h4, g_pb.y), g_wb.y, s, col);
    marks(hard(g_h5, g_pb.z), g_wb.z, s, col);

    // the leaked crystal: black, its own dim sheen here and there; a bright rim where it pushes
    // the rest aside. The pointer under it is gone too.
    if (reach > 0.0) {
        float rim = g_px * (2.0 + 3.0 * fbm(s / (g_px * 55.0)));
        // healing, the panel resyncs: rows of the picture come back through the black in flashes
        float healing = 1.0 - clamp(g_left / BLOT_HEAL, 0.0, 1.0);
        bool resync = healing > 0.0
            && hash(vec2(floor(s.y / (g_px * 3.0)), g_step + 9.0)) < 0.4 * healing;
        if (resync && (v < 0.0 || vein)) {
            col = min(col * 1.25, vec3(1.0));
        } else if (v < 0.0 || vein) {
            col = vec3(0.0);
            if (v < 0.0 && hash(floor(s / (g_px * 2.0)) + 0.37) > 0.992)
                col = vec3(0.16, 0.05, 0.3);
        } else if (v < rim) {
            col = mix(col, vec3(0.85, 0.75, 1.0), 0.75);
        }
    }

    cracks(g_h0, g_wa.x, g_pa.x, s, v, col);
    cracks(g_h1, g_wa.y, g_pa.y, s, v, col);
    cracks(g_h2, g_wa.z, g_pa.z, s, v, col);
    cracks(g_h3, g_wb.x, g_pb.x, s, v, col);
    cracks(g_h4, g_wb.y, g_pb.y, s, v, col);
    cracks(g_h5, g_wb.z, g_pb.z, s, v, col);

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
