// Test-only assignment of #180's explicit already-shared meteorological input bytes.
// This performs no sampling, normalization or boundary calculation.
@group(0) @binding(0) var<storage, read> input: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;
@group(0) @binding(2) var<uniform> params: vec4<u32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < params.x) { output[id.x] = input[id.x]; }
}
