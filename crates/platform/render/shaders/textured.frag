#version 450

// Textured quad fragment stage: sample the bound image and modulate it by the
// per-node tint (RGB is linear; A carries the tint's alpha times the composed
// opacity).

layout(location = 0) in vec2 in_uv;
layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform sampler2D u_texture;

layout(push_constant) uniform PushConstants {
    mat4 mvp;
    vec4 tint;
    vec4 uv_rect;
    uint mode;
    float opacity;
    vec2 target_size;
    uvec4 flags;
} pc;

// The lane and bit the draw sets when it reads its source as an opaque image.
const uint FLAG_OPAQUE_SOURCE = 1u;

void main() {
    vec4 sampled = texture(u_texture, in_uv);
    // A source read as an opaque image carries no alpha of its own: its colour
    // lands as it is and the composed opacity is the coverage.
    float alpha = sampled.a * pc.tint.a;
    if ((pc.flags.x & FLAG_OPAQUE_SOURCE) != 0u) {
        alpha = pc.opacity;
    }
    out_color = vec4(sampled.rgb * pc.tint.rgb, alpha);
}
