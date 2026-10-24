struct Uniform {
    viewport_min: vec2<f32>,
    viewport_size: vec2<f32>,
    content_left: f32,
    content_bottom: f32,
    scale_x: f32,
    scale_y: f32,
    horizontal: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> u: Uniform;

struct Instance {
    @location(0) column: f32,
    @location(1) start: f32,
    @location(2) end: f32,
    @location(3) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

fn gamma_to_linear(srgb: vec3<f32>) -> vec3<f32> {
    let cutoff = srgb < vec3<f32>(0.04045);
    let lower = srgb / vec3<f32>(12.92);
    let higher = pow((srgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, instance: Instance) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];

    var left: f32;
    var right: f32;
    var top: f32;
    var bottom: f32;

    if (u.horizontal > 0.5) {
        left = u.content_left + instance.start * u.scale_x + 0.2;
        right = max(u.content_left + instance.end * u.scale_x - 0.2, left + 0.5);
        bottom = u.content_bottom - instance.column * u.scale_y - 0.3;
        top = min(u.content_bottom - (instance.column + 1.0) * u.scale_y + 0.3, bottom - 0.5);
    } else {
        left = u.content_left + instance.column * u.scale_x + 0.3;
        right = max(u.content_left + (instance.column + 1.0) * u.scale_x - 0.3, left + 0.5);
        top = u.content_bottom - instance.end * u.scale_y + 0.2;
        bottom = max(u.content_bottom - instance.start * u.scale_y - 0.2, top + 0.5);
    }

    let point = vec2<f32>(
        mix(left, right, corner.x),
        mix(top, bottom, corner.y),
    );

    let ndc = (point - u.viewport_min) / u.viewport_size * 2.0 - vec2<f32>(1.0, 1.0);
    var out: VertexOutput;
    out.position = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.color = instance.color;
    return out;
}

@fragment
fn fs_main_srgb(in: VertexOutput) -> @location(0) vec4<f32> {
    let linear = gamma_to_linear(in.color.rgb);
    return vec4<f32>(linear * in.color.a, in.color.a);
}

@fragment
fn fs_main_gamma(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.rgb * in.color.a, in.color.a);
}
