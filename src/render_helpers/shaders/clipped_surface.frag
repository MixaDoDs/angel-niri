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

uniform float niri_scale;

uniform vec2 geo_size;
uniform vec4 corner_radius;
uniform mat3 input_to_geo;

// angel-niri (window_dither.rs): how much is drawn in dots — a 4×4 Bayer screen door of art
// pixels, `niri_dither_cell` physical px each; 1, or 0 where it is not set (the postprocess
// program), is whole
uniform float niri_dither;
uniform float niri_dither_cell;

// 4×4 Bayer matrix, 0..15 (by formula: NVIDIA's GL takes no constant arrays)
float niri_bayer4(vec2 c) {
    vec2 a = mod(c, 2.0);
    vec2 b = mod(floor(c / 2.0), 2.0);
    return 4.0 * (2.0 * a.x + 3.0 * a.y - 4.0 * a.x * a.y) + (2.0 * b.x + 3.0 * b.y - 4.0 * b.x * b.y);
}

float niri_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius);
vec4 postprocess(vec4 color);

void main() {
    vec3 coords_geo = input_to_geo * vec3(v_coords, 1.0);

    // Sample the texture.
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    color = postprocess(color);

    if (coords_geo.x < 0.0 || 1.0 < coords_geo.x || coords_geo.y < 0.0 || 1.0 < coords_geo.y) {
        // Clip outside geometry.
        color = vec4(0.0);
    } else {
        // Apply corner rounding inside geometry.
        color = color * niri_rounding_alpha(coords_geo.xy * geo_size, geo_size, corner_radius);
    }

    if (0.0 < niri_dither && niri_dither < 1.0) {
        vec2 cell = floor(gl_FragCoord.xy / niri_dither_cell);
        if ((niri_bayer4(cell) + 0.5) / 16.0 >= niri_dither)
            color = vec4(0.0);
    }

    // Apply final alpha and tint.
    color = color * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
