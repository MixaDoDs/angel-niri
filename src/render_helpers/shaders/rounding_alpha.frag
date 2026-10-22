// angel-niri (pixel_corners.rs): > 0 — staircase corners of this pixel (logical px), hard-edged
uniform float niri_pixel;

float niri_rounding_alpha(vec2 coords, vec2 size, vec4 corner_radius) {
    vec2 center;
    float radius;
    // (angel-niri) the distance from the corner itself, along each side
    vec2 from_corner;

    if (coords.x < corner_radius.x && coords.y < corner_radius.x) {
        radius = corner_radius.x;
        center = vec2(radius, radius);
        from_corner = coords;
    } else if (size.x - corner_radius.y < coords.x && coords.y < corner_radius.y) {
        radius = corner_radius.y;
        center = vec2(size.x - radius, radius);
        from_corner = vec2(size.x - coords.x, coords.y);
    } else if (size.x - corner_radius.z < coords.x && size.y - corner_radius.z < coords.y) {
        radius = corner_radius.z;
        center = vec2(size.x - radius, size.y - radius);
        from_corner = size - coords;
    } else if (coords.x < corner_radius.w && size.y - corner_radius.w < coords.y) {
        radius = corner_radius.w;
        center = vec2(radius, size.y - radius);
        from_corner = vec2(coords.x, size.y - coords.y);
    } else {
        return 1.0;
    }

    // angel-niri: a staircase — the pixel (i, j) from the corner is cut while i + j < steps
    if (niri_pixel > 0.0) {
        float steps = floor(radius / niri_pixel + 0.5);
        vec2 ij = floor(from_corner / niri_pixel);
        return ij.x + ij.y < steps ? 0.0 : 1.0;
    }

    float dist = distance(coords, center);

    // Manual smoothstep() between radius - half_px and radius + half_px
    // to avoid a division in clamp().
    float t = clamp((dist - radius) * niri_scale + 0.5, 0.0, 1.0);
    return 1.0 - t * t * (3.0 - 2.0 * t);
}
