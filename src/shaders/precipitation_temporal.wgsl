// FLEXPART 11.1 interpol_mod.f90:1302-1316,1549-1550, eta=no, numpf=1.
// r = (r0 * real(t1-t) + r1 * real(t-t0)) / (real(t1-t0) / 3).
// Inputs are #87 spatial samples of #207 reconstructed point rates [mm/h].
// At the final genuine timestamp only: dt=0, dt1=0 -> dt=1, dt2=1.
// Keep division by f32(1/3), including its rounding; do not replace by 3*r0.
struct Parameters {
    dt1_seconds: i32,
    dt2_seconds: i32,
    dt_seconds: i32,
    output_pair: u32,
};
@group(0) @binding(0) var<storage, read> lsp0: array<f32>;
@group(0) @binding(1) var<storage, read> lsp1: array<f32>;
@group(0) @binding(2) var<storage, read> cp0: array<f32>;
@group(0) @binding(3) var<storage, read> cp1: array<f32>;
@group(0) @binding(4) var<storage, read_write> rates_mmh: array<f32>;
@group(0) @binding(5) var<storage, read_write> status: atomic<u32>;
@group(0) @binding(6) var<uniform> params: Parameters;
const NON_FINITE_SAMPLE: u32 = 4u;
fn finite(value: f32) -> bool {
    return (bitcast<u32>(value) & 0x7f800000u) != 0x7f800000u;
}
@compute @workgroup_size(1)
fn main() {
    let invalid = bitcast<f32>(0x7fc00000u);
    let offset = 2u * params.output_pair;
    rates_mmh[offset] = invalid;
    rates_mmh[offset + 1u] = invalid;
    if (atomicLoad(&status) != 0u) { return; }
    let a = lsp0[0];
    let b = lsp1[0];
    let c = cp0[0];
    let d = cp1[0];
    if (!(finite(a) && finite(b) && finite(c) && finite(d))) {
        atomicOr(&status, NON_FINITE_SAMPLE);
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
        atomicOr(&status, NON_FINITE_SAMPLE);
        return;
    }
    rates_mmh[offset] = lsp;
    rates_mmh[offset + 1u] = cp;
}
