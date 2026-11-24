#version 450

// Solid quad fragment stage: output the per-node tint directly.
//
// The tint's alpha is the tint's own alpha times the composed opacity; a draw
// that reads its source as an opaque image outputs the composed opacity alone.

layout(location = 0) out vec4 out_color;

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
    // A source read as an opaque image carries no alpha of its own: its colour
    // lands as it is and the composed opacity is the coverage.
    float alpha = pc.tint.a;
    if ((pc.flags.x & FLAG_OPAQUE_SOURCE) != 0u) {
        alpha = pc.opacity;
    }
    out_color = vec4(pc.tint.rgb, alpha);
}
