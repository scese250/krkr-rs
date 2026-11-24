#version 450

// Shared vertex stage for both the textured and solid quad programs.
//
// The unit quad has local coordinates in [0, 1] x [0, 1] with the origin at
// the top-left (y grows downward). `pc.mvp` maps that unit square directly to
// Vulkan normalised device coordinates; `pc.uv_rect` maps the unit square into
// the source sub-rectangle of the bound texture.
//
// The push-constant block is the one the fragment stages declare in full; the
// vertex stage reads its first two members and declares the rest so every
// stage agrees on the layout.

layout(location = 0) in vec2 in_pos;
layout(location = 0) out vec2 out_uv;

layout(push_constant) uniform PushConstants {
    mat4 mvp;
    vec4 tint;
    vec4 uv_rect;
    uint mode;
    float opacity;
    vec2 target_size;
    uvec4 flags;
} pc;

void main() {
    gl_Position = pc.mvp * vec4(in_pos, 0.0, 1.0);
    out_uv = pc.uv_rect.xy + in_pos * pc.uv_rect.zw;
}
