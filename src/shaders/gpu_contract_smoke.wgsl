// Infrastructure smoke equation: output = multiplicand * multiplier + addend.
// This kernel is intentionally non-scientific. It proves that host-provided
// input crosses H2D, WGSL arithmetic executes, and the result crosses D2H.

struct SmokeInput {
    multiplicand: u32,
    multiplier: u32,
    addend: u32,
    padding: u32,
}

@group(0) @binding(0)
var<uniform> smoke_input: SmokeInput;

@group(0) @binding(1)
var<storage, read_write> smoke_output: array<u32>;

@compute @workgroup_size(1)
fn main() {
    smoke_output[0] = smoke_input.multiplicand * smoke_input.multiplier + smoke_input.addend;
}
