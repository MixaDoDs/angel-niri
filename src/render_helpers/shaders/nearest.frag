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

// angel-niri (privacy_look.rs): the texture's size in texels; every texel is drawn as a hard
// block, so pixel art and the privacy mosaic stay sharp however much they are scaled up.
uniform vec2 tex_size;

void main() {
    vec2 coords = (floor(v_coords * tex_size) + 0.5) / tex_size;
    vec4 color = texture2D(tex, coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif

    // Apply final alpha and tint.
    color = color * alpha;

#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif

    gl_FragColor = color;
}
