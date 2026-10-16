// #185 prerequisite gate only; no interpolation or remapping equations.
@group(0) @binding(0) var<storage, read> target_status: array<u32>;
@group(0) @binding(1) var<storage, read> preparation: array<u32>;
@group(0) @binding(2) var<storage, read_write> status: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read> prepared_values: array<f32>;
@group(0) @binding(4) var<uniform> unused: vec4<u32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let column = id.x;
    let columns = arrayLength(&preparation);
    if (column >= columns) { return; }
    var valid = target_status[0] == 2u;
    valid = valid && preparation[column] == 2u;
    for (var index = column; index < arrayLength(&prepared_values); index += columns) {
        valid = valid && (bitcast<u32>(prepared_values[index]) & 0x7f800000u) != 0x7f800000u;
    }
    if (!valid) { atomicOr(&status[0], 1u); }
}
