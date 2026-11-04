// FLEXPART 11.1 interpol_mod.f90:1302-1316,1549-1550, eta=no, numpf=1 (#227).
// Lane-parallel form of precipitation_temporal.wgsl (#119): one forward step
// shares itime and the memory pair, so dt1/dt2/dt are uniform for all lanes.
// r = (r0 * real(t1-t) + r1 * real(t-t0)) / (real(t1-t0) / 3).
// Inputs are lane-aligned #87 samples of four #207 point-rate planes [mm/h].
// At the final genuine timestamp only: dt=0, dt1=0 -> dt=1, dt2=1.
// Keep division by f32(1/3), separate products and the sum order; no FMA.
// Candidates are not a handoff: precipitation_resident_publish.wgsl releases
// them only after every lane status of this dispatch is final.
struct Parameters {
    capacity: u32,
    active_count: u32,
    dt1_seconds: i32,
    dt2_seconds: i32,
    dt_seconds: i32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};
@group(0) @binding(0) var<storage, read> lsp0: array<f32>;
@group(0) @binding(1) var<storage, read> lsp1: array<f32>;
@group(0) @binding(2) var<storage, read> cp0: array<f32>;
@group(0) @binding(3) var<storage, read> cp1: array<f32>;
@group(0) @binding(4) var<storage, read> producer_status: array<u32>;
@group(0) @binding(5) var<storage, read_write> status: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write> candidate_mmh: array<f32>;
@group(0) @binding(7) var<uniform> params: Parameters;
// Shared word: #171 lane failure bit, non-finite #87/#119 sample bit and the
// #207 status shifted above them. Lane codes follow the #171 status ABI.
const LANE_FAILURE: u32 = 1u;
const NON_FINITE_SAMPLE: u32 = 4u;
const PRODUCER_STATUS_SHIFT: u32 = 8u;
const VALID: u32 = 2u;
const SAMPLED_RESULT: u32 = 9u;
fn finite(value: f32) -> bool {
    return (bitcast<u32>(value) & 0x7f800000u) != 0x7f800000u;
}
fn fail(lane: u32, shared_bits: u32) {
    atomicStore(&status[lane + 1u], SAMPLED_RESULT);
    atomicOr(&status[0], shared_bits);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.capacity) { return; }
    let invalid = bitcast<f32>(0x7fc00000u);
    candidate_mmh[2u * lane] = invalid;
    candidate_mmh[2u * lane + 1u] = invalid;
    // The #207 producer status is batch-wide and independent of the active prefix.
    if (lane == 0u && producer_status[0] != 0u) {
        atomicOr(&status[0], producer_status[0] << PRODUCER_STATUS_SHIFT);
    }
    if (lane >= params.active_count) { return; }
    let reason = atomicLoad(&status[lane + 1u]);
    if (reason == 0u || reason > VALID) { return; }
    if (reason != VALID) {
        fail(lane, LANE_FAILURE);
        return;
    }
    let a = lsp0[lane];
    let b = lsp1[lane];
    let c = cp0[lane];
    let d = cp1[lane];
    if (!(finite(a) && finite(b) && finite(c) && finite(d))) {
        fail(lane, LANE_FAILURE | NON_FINITE_SAMPLE);
        return;
    }
    let dt1 = f32(params.dt1_seconds);
    var dt2 = f32(params.dt2_seconds);
    var dt = f32(params.dt_seconds);
    if (dt == 0.0 && dt1 == 0.0) {
        dt = 1.0;
        dt2 = 1.0;
    }
    let dtt = dt / 3.0;
    let lsp_left = a * dt2;
    let lsp_right = b * dt1;
    let cp_left = c * dt2;
    let cp_right = d * dt1;
    let lsp = (lsp_left + lsp_right) / dtt;
    let cp = (cp_left + cp_right) / dtt;
    if (!(finite(lsp) && finite(cp))) {
        fail(lane, LANE_FAILURE | NON_FINITE_SAMPLE);
        return;
    }
    candidate_mmh[2u * lane] = lsp;
    candidate_mmh[2u * lane + 1u] = cp;
}
