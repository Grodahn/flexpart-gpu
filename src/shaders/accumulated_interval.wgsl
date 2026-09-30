// Accumulated-field interval/reset transformation kernel for issue #90.
//
// Ported from the canonical CPU semantics in `src/meteorology/accumulation.rs`
// (issue #75), which owns `reset_deaccumulation` upstream of FLEXPART
// `interpol_mod.f90:1209-1582` (`interpol_rain`).
//
// This kernel executes ONLY the supported numerical transformation for
// pre-validated canonical inputs:
//
//   amount[i]  = reset_applied[i] == 1u ? curr[i] : (curr[i] - prev[i])
//   rate_si[i] = amount[i] / duration_seconds[i]
//   rate_mmh[i] = rate_si[i] * 3600.0
//
// where `curr/prev` are source accumulated totals in kg/m2 (numerically equal
// to mm water depth), `duration_seconds` is the host-validated strictly
// positive interval length in seconds, and `reset_applied` selects the fresh
// run amount versus the within-run delta.
//
// Host responsibilities (Rust, before dispatch):
// - reject empty sequences, non-increasing valid times, backwards resets,
//   resets not before valid time, mid-window resets, uncovered gaps, negative
//   or non-finite source amounts, and same-run negative deltas;
// - derive the canonical interval window `[start, end]`, its strictly positive
//   duration, and the `reset_applied` flag exactly as defined by #75;
// - convert validated f64 amounts/durations to f32 without loss of finiteness.
//
// GPU responsibilities (this shader):
// - compute the interval amount by delta versus fresh-run selection;
// - compute the SI rate by division and the mm/h handoff rate by scaling;
// - write distinct amount and rate outputs that remain device-resident for
//   later #76 composition.
//
// Units:
// - curr/prev/amount: [kg/m2] (numerically equal to mm water depth)
// - duration_seconds: [s], strictly positive by host validation
// - rate_si: [kg/(m2 s)]
// - rate_mmh: [mm/h], exactly rate_si * 3600.0
//
// Buffer contract:
// - binding(0): transform inputs, array<AccumulatedTransformInput>, read-only
// - binding(1): interval amounts, array<f32> [kg/m2], read-write
// - binding(2): SI rates, array<f32> [kg/(m2 s)], read-write
// - binding(3): handoff rates, array<f32> [mm/h], read-write
//   (all three outputs are GPU-resident for #76; host readback only at
//   explicit validation/output boundaries)
// - binding(4): uniform params (interval_count, pad, pad, pad)

struct AccumulatedTransformInput {
    curr_amount_kg_per_square_meter: f32,
    prev_amount_kg_per_square_meter: f32,
    duration_seconds: f32,
    reset_applied: u32,
};

struct AccumulatedTransformParams {
    interval_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0)
var<storage, read> transform_inputs: array<AccumulatedTransformInput>;

@group(0) @binding(1)
var<storage, read_write> interval_amounts: array<f32>;

@group(0) @binding(2)
var<storage, read_write> interval_rates_si: array<f32>;

@group(0) @binding(3)
var<storage, read_write> interval_rates_mmh: array<f32>;

@group(0) @binding(4)
var<uniform> params: AccumulatedTransformParams;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let idx = gid.y * (nwg.x * 64u) + gid.x;
    if (idx >= params.interval_count) {
        return;
    }

    let entry = transform_inputs[idx];
    var amount: f32 = 0.0;
    if (entry.reset_applied == 1u) {
        amount = entry.curr_amount_kg_per_square_meter;
    } else {
        amount = entry.curr_amount_kg_per_square_meter - entry.prev_amount_kg_per_square_meter;
    }
    let rate_si = amount / entry.duration_seconds;
    let rate_mmh = rate_si * 3600.0;

    interval_amounts[idx] = amount;
    interval_rates_si[idx] = rate_si;
    interval_rates_mmh[idx] = rate_mmh;
}
