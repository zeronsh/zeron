// A raymarched Mandelbulb in the style of cznull's volume shader: distance
// estimation, an orbit camera from iCamera, and a transparent sky.
float bulb(vec3 p, out float trap) {
    vec3 z = p;
    float dr = 1.0;
    float r = 0.0;
    trap = 1e9;
    for (int i = 0; i < 8; i++) {
        r = length(z);
        if (r > 2.0) break;
        trap = min(trap, r);
        float theta = acos(clamp(z.z / r, -1.0, 1.0)) * 8.0;
        float phi = atan(z.y, z.x) * 8.0;
        dr = pow(r, 7.0) * 8.0 * dr + 1.0;
        z = pow(r, 8.0) * vec3(sin(theta) * cos(phi), sin(phi) * sin(theta), cos(theta)) + p;
    }
    return 0.5 * log(r) * r / dr;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
    float yaw = iCamera.x;
    float pitch = iCamera.y;
    vec3 eye = vec3(cos(pitch) * sin(yaw), sin(pitch), cos(pitch) * cos(yaw)) * 2.6 / iCamera.z;
    vec3 forward = normalize(-eye);
    vec3 right = normalize(cross(forward, vec3(0.0, 1.0, 0.0)));
    vec3 up = cross(right, forward);
    vec3 dir = normalize(forward * 1.6 + uv.x * right + uv.y * up);
    float t = 0.0;
    float trap = 0.0;
    bool hit = false;
    for (int i = 0; i < 128; i++) {
        float d = bulb(eye + dir * t, trap);
        if (d < 0.0008 * t) { hit = true; break; }
        t += d;
        if (t > 6.0) break;
    }
    if (!hit) { fragColor = vec4(0.0); return; }
    vec3 p = eye + dir * t;
    vec2 e = vec2(0.0005, 0.0);
    float tt;
    vec3 n = normalize(vec3(
        bulb(p + e.xyy, tt) - bulb(p - e.xyy, tt),
        bulb(p + e.yxy, tt) - bulb(p - e.yxy, tt),
        bulb(p + e.yyx, tt) - bulb(p - e.yyx, tt)));
    vec3 light = normalize(vec3(0.3, 0.9, 0.3));
    float diffuse = max(dot(n, light), 0.0);
    vec3 base = 0.5 + 0.5 * sin(trap * 10.0 + vec3(0.0, 2.05, -2.05));
    fragColor = vec4(base * (0.3 + 0.7 * diffuse), 1.0);
}
