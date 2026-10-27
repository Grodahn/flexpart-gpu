// Precipitation point-member reconstruction kernel for issue #207.
//
// Ported from flex_extract 7.1.2 `disaggregation.darain`
// (Source/Python/Mods/disaggregation.py:106-157, pinned commit
// e0005c99ac81d12faa45a8ff799debbd592b0dc0, blob 12ec74a7), which
// `EcFlexpart.deacc_fluxes` calls for LSP/CP when RRINT=0
// (EcFlexpart.py:1180-1185). Original method: P. James (2000), A. Beck (2003).
//
// For one precipitation parameter and one horizontal cell, four consecutive
// interval-mean rates [mm/h] over equal intervals of length D
//
//   xa: (T-2D, T-D]   xb: (T-D, T]   xc: (T, T+D]   xd: (T+D, T+2D]
//
// give the rate at the interval boundary T:
//
//   x    = x < 0 ? 0 : x                        (each of xa, xb, xc, xd)
//   xac  = xa + xc > 0 ? xb * xc / (xa + xc) : 0.5 * xb
//   xbd  = xb + xd > 0 ? xb * xc / (xb + xd) : 0.5 * xc
//   r(T) = xac + xbd
//
// Operation order follows the original numpy expressions; `xb * xc` is formed
// before the division in both branches.
//
// Host responsibilities (Rust, before dispatch): validate the #202 envelope
// (N >= 5 contiguous equal integer-hour intervals, one source/grid/context
// identity, RRINT=0 / PUREFC=0 / no BASETIME), and size the resources.
//
// Buffer contract:
// - binding(0): interval-mean rates [mm/h], read-only, `[interval * cells + cell]`
//   (the #90 `rate_millimeter_per_hour` grid, used in place)
// - binding(1): members [mm/h], read-write, `[member * cells + cell]`; member m
//   uses intervals m..m+3 and belongs to T = end(interval m + 1)
// - binding(2): shared fatal status word, read-write atomic; bit 1 = non-finite
//   input rate, bit 2 = non-finite member. Consumers reject any nonzero value.
// - binding(3): uniform params (member_count, cell_count, pad, pad)

struct DarainParams {
    member_count: u32,
    cell_count: u32,
    _pad0: u32,
    _pad1: u32,
};

const STATUS_NON_FINITE_RATE: u32 = 1u;
const STATUS_NON_FINITE_MEMBER: u32 = 2u;
const F32_EXPONENT_MASK: u32 = 0x7f800000u;

@group(0) @binding(0)
var<storage, read> interval_rates_mmh: array<f32>;

@group(0) @binding(1)
var<storage, read_write> members_mmh: array<f32>;

@group(0) @binding(2)
var<storage, read_write> status: atomic<u32>;

@group(0) @binding(3)
var<uniform> params: DarainParams;

fn is_finite_f32(value: f32) -> bool {
    return (bitcast<u32>(value) & F32_EXPONENT_MASK) != F32_EXPONENT_MASK;
}

// numpy `x[x < 0.] = 0.` keeps every value that is not below zero unchanged.
fn clip_below_zero(value: f32) -> f32 {
    return select(value, 0.0, value < 0.0);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let idx = gid.y * (nwg.x * 64u) + gid.x;
    if (idx >= params.member_count * params.cell_count) {
        return;
    }
    let member = idx / params.cell_count;
    let cell = idx % params.cell_count;
    let stride = params.cell_count;
    let first = member * stride + cell;

    let raw_a = interval_rates_mmh[first];
    let raw_b = interval_rates_mmh[first + stride];
    let raw_c = interval_rates_mmh[first + 2u * stride];
    let raw_d = interval_rates_mmh[first + 3u * stride];
    if (!(is_finite_f32(raw_a) && is_finite_f32(raw_b) && is_finite_f32(raw_c) && is_finite_f32(raw_d))) {
        atomicOr(&status, STATUS_NON_FINITE_RATE);
    }

    let xa = clip_below_zero(raw_a);
    let xb = clip_below_zero(raw_b);
    let xc = clip_below_zero(raw_c);
    let xd = clip_below_zero(raw_d);

    var xac: f32 = 0.5 * xb;
    if (xa + xc > 0.0) {
        xac = xb * xc / (xa + xc);
    }
    var xbd: f32 = 0.5 * xc;
    if (xb + xd > 0.0) {
        xbd = xb * xc / (xb + xd);
    }
    let member_rate = xac + xbd;
    if (!is_finite_f32(member_rate)) {
        atomicOr(&status, STATUS_NON_FINITE_MEMBER);
    }
    members_mmh[idx] = member_rate;
}
