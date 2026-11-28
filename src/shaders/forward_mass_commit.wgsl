// The budget pass owns eligibility for this particle publication.
@group(0) @binding(0) var<storage, read> ledger: array<u32>;
@group(0) @binding(1) var<storage, read> staged: array<u32>;
@group(0) @binding(2) var<storage, read_write> committed: array<u32>;
@group(0) @binding(3) var<uniform> params: vec4<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let slot = gid.y * groups.x * 64u + gid.x;
    if (slot >= params.x || ledger[0] != 0u || ledger[3] != 1u) { return; }
    for (var word = 0u; word < 24u; word++) {
        committed[slot * 24u + word] = staged[slot * 24u + word];
    }
}
