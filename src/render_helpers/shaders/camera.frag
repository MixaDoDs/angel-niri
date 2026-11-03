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

// angel-niri (camera.rs): the finished frame of an output seen through a camera that follows the
// pointer — zoomed about `focus`, smeared along the pan and outward while zooming, as a camera
// with its shutter open for a moment. Laid out on the upright screen in physical pixels.
uniform vec2 g_screen;
uniform mat3 g_tex_to_screen;
uniform mat3 g_screen_to_tex;
// focus x, y (physical px of the content at the screen's centre), zoom, taps (1 = sharp)
uniform vec4 g_view;
// smear along the pan (content physical px, whole length), outward (share per side), 0, 0
uniform vec4 g_smear;

vec2 toTex(vec2 s) {
    return clamp((g_screen_to_tex * vec3(s / g_screen, 1.0)).xy, vec2(0.0), vec2(1.0));
}

void main() {
    vec2 s = (g_tex_to_screen * vec3(v_coords, 1.0)).xy * g_screen;
    vec2 c = g_view.xy + (s - 0.5 * g_screen) / g_view.z;

    vec3 col;
    float n = g_view.w;
    if (n < 1.5) {
        col = texture2D(tex, toTex(c)).rgb;
    } else {
        // a tent of taps centred on the point: the smear trails both ways like a real shutter
        vec3 acc = vec3(0.0);
        float wsum = 0.0;
        for (int i = 0; i < 13; i++) {
            if (float(i) >= n)
                break;
            float t = float(i) / (n - 1.0) - 0.5;
            vec2 p = c - g_smear.xy * t - (c - g_view.xy) * g_smear.z * t;
            float w = 1.0 - abs(t) * 1.2;
            acc += texture2D(tex, toTex(p)).rgb * w;
            wsum += w;
        }
        col = acc / wsum;
    }

    vec4 color = vec4(col, 1.0);
    color = color * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
