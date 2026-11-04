// #227 batch-wide publication gate; no physical calculation.
// A separate dispatch after precipitation_resident_rates.wgsl, so every lane
// status from every workgroup is final. Any shared fatal bit publishes no lane;
// inactive, tail and failed lanes are written as explicit quiet NaN, never as
// a dry-weather zero. `executed` proves this gate ran for the validation boundary.
struct Params { capacity: u32, active_count: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<storage, read> candidate_mmh: array<f32>;
@group(0) @binding(1) var<storage, read> status: array<u32>;
@group(0) @binding(2) var<storage, read_write> rates_mmh: array<f32>;
@group(0) @binding(3) var<storage, read_write> executed: array<u32>;
@group(0) @binding(4) var<uniform> params: Params;
const VALID: u32 = 2u;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.capacity) { return; }
    if (lane == 0u) { executed[0] = 1u; }
    let invalid = bitcast<f32>(0x7fc00000u);
    let publish = status[0] == 0u && lane < params.active_count && status[lane + 1u] == VALID;
    rates_mmh[2u * lane] = select(invalid, candidate_mmh[2u * lane], publish);
    rates_mmh[2u * lane + 1u] = select(invalid, candidate_mmh[2u * lane + 1u], publish);
}
