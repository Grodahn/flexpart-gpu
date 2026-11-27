// #38 stable forward inventory. Transport stop events carry unchanged airborne
// mass before the trajectory is terminated (advance_mod.f90:857-867).
// Particle mass begins at byte 24, which is not vec4-aligned.
struct ParticleWords { words: array<u32, 24> };
struct Totals {
    before: vec4<f32>, release: vec4<f32>, sample: vec4<f32>,
    exit: vec4<f32>, airborne: vec4<f32>,
}
struct State {
    // Sticky error code, first bad step, first bad particle/species lane, commit decision.
    status: vec4<u32>,
    injected: vec4<f32>, airborne: vec4<f32>, horizontal: vec4<f32>,
    injected_correction: vec4<f32>, horizontal_correction: vec4<f32>,
    wet: vec4<f32>, dry: vec4<f32>, decayed: vec4<f32>, age_exit: vec4<f32>,
    min_mass_exit: vec4<f32>, age_and_mass_exit: vec4<f32>,
    last: Totals,
}
struct Params { capacity: u32, first_release: u32, release_count: u32, species: u32,
    step: u32, outgoing: u32, pad_a: u32, pad_b: u32 };
@group(0) @binding(0) var<storage, read> original: array<ParticleWords>;
@group(0) @binding(1) var<storage, read> released: array<ParticleWords>;
@group(0) @binding(2) var<storage, read> batch: array<ParticleWords>;
@group(0) @binding(3) var<storage, read> staged: array<ParticleWords>;
@group(0) @binding(4) var<storage, read> stops: array<u32>;
@group(0) @binding(5) var<storage, read> advection: array<u32>;
@group(0) @binding(6) var<storage, read_write> ledger: State;
@group(0) @binding(7) var<uniform> params: Params;
var<workgroup> subtotals: array<Totals, 64>;
var<workgroup> first_fault: array<u32, 64>;
var<workgroup> proceed: u32;
const NO_FAULT: u32 = 0xffffffffu;
const RELATIVE_TOLERANCE: f32 = 1e-4;

fn alive(p: ParticleWords) -> bool { return (p.words[5] & 1u) != 0u; }
fn masses(p: ParticleWords) -> vec4<f32> {
    return bitcast<vec4<f32>>(vec4<u32>(p.words[6], p.words[7], p.words[8], p.words[9]));
}
fn valid(m: f32) -> bool {
    return m >= 0.0 && (bitcast<u32>(m) & 0x7f800000u) != 0x7f800000u;
}
fn closes(expected: f32, observed: f32, scale: f32) -> bool {
    return valid(expected) && valid(observed) && abs(expected - observed) <= RELATIVE_TOLERANCE * scale;
}
fn transport_failed() -> bool {
    if (params.outgoing == 0u) { return false; }
    for (var i = 0u; i < 6u; i++) {
        if (advection[i * (params.capacity + 1u)] != 0u) { return true; }
    }
    return advection[6u * (params.capacity + 1u)] != 0u;
}
struct Compensated { sum: vec4<f32>, correction: vec4<f32> }
fn compensated_add(sum: vec4<f32>, correction: vec4<f32>, value: vec4<f32>) -> Compensated {
    let corrected = value - correction;
    let next = sum + corrected;
    return Compensated(next, (next - sum) - corrected);
}
fn add(a: Totals, b: Totals) -> Totals {
    return Totals(a.before + b.before, a.release + b.release, a.sample + b.sample,
        a.exit + b.exit, a.airborne + b.airborne);
}
fn reject(code: u32, lane: u32) {
    ledger.status = vec4<u32>(code, params.step, lane, 0u);
}

@compute @workgroup_size(64)
fn main(@builtin(local_invocation_index) lane: u32) {
    if (lane == 0u) {
        proceed = 0u;
        if (ledger.status.x == 0u) {
            if (transport_failed()) { reject(1u, NO_FAULT); }
            else { proceed = 1u; }
        }
    }
    if (workgroupUniformLoad(&proceed) == 0u) { return; }
    let zero = vec4<f32>(0.0);
    var sum = Totals(zero, zero, zero, zero, zero);
    var err = sum;
    var fault = NO_FAULT;
    for (var slot = lane; slot < params.capacity; slot += 64u) {
        let old = original[slot];
        let pre = released[slot];
        let post = staged[slot];
        var stop = 0u;
        if (params.outgoing != 0u) { stop = stops[slot]; }
        let exits = stop == 4u;
        let is_release = slot >= params.first_release && slot - params.first_release < params.release_count;
        var credit = zero;
        if (is_release) { credit = masses(batch[slot - params.first_release]); }
        let before = select(zero, masses(old), alive(old));
        let sample = select(zero, masses(pre), alive(pre));
        let airborne = select(zero, masses(post), alive(post));
        // The stop producer supplies the classification and retains the stock until
        // this consumer credits it. The closing stock comes from live flags only.
        let exit = select(zero, masses(post), exits);
        if ((stop != 0u && !exits)
            || (is_release && (alive(old) || !alive(pre)))
            || (!is_release && alive(old) != alive(pre))
            || (exits && !alive(pre))
            || (alive(post) != (alive(pre) && !exits))) {
            fault = min(fault, slot * 4u);
        }
        for (var species = 0u; species < 4u; species++) {
            if (!valid(before[species]) || !valid(credit[species]) || !valid(sample[species])
                || !valid(airborne[species]) || !valid(exit[species])
                || (species >= params.species && (before[species] != 0.0
                    || sample[species] != 0.0 || airborne[species] != 0.0 || exit[species] != 0.0))
                || sample[species] != select(before[species], credit[species], is_release)
                || sample[species] != select(airborne[species], exit[species], exits)) {
                fault = min(fault, slot * 4u + species);
            }
        }
        let b = compensated_add(sum.before, err.before, before);
        let r = compensated_add(sum.release, err.release, credit);
        let s = compensated_add(sum.sample, err.sample, sample);
        let e = compensated_add(sum.exit, err.exit, exit);
        let a = compensated_add(sum.airborne, err.airborne, airborne);
        sum = Totals(b.sum, r.sum, s.sum, e.sum, a.sum);
        err = Totals(b.correction, r.correction, s.correction, e.correction, a.correction);
    }
    subtotals[lane] = sum;
    first_fault[lane] = fault;
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride >>= 1u) {
        if (lane < stride) {
            subtotals[lane] = add(subtotals[lane], subtotals[lane + stride]);
            first_fault[lane] = min(first_fault[lane], first_fault[lane + stride]);
        }
        workgroupBarrier();
    }
    if (lane != 0u) { return; }
    if (first_fault[0] != NO_FAULT) { reject(2u, first_fault[0]); return; }
    let step = subtotals[0];
    let injection = compensated_add(ledger.injected, ledger.injected_correction, step.release);
    let exits = compensated_add(ledger.horizontal, ledger.horizontal_correction, step.exit);
    let injected = injection.sum;
    let horizontal = exits.sum;
    let accounted = step.airborne + horizontal + ledger.wet + ledger.dry + ledger.decayed
        + ledger.age_exit + ledger.min_mass_exit + ledger.age_and_mass_exit;
    for (var species = 0u; species < 4u; species++) {
        if (ledger.wet[species] != 0.0 || ledger.dry[species] != 0.0 || ledger.decayed[species] != 0.0
            || ledger.age_exit[species] != 0.0 || ledger.min_mass_exit[species] != 0.0
            || ledger.age_and_mass_exit[species] != 0.0
            || !closes(ledger.airborne[species], step.before[species], ledger.airborne[species])
            || !closes(step.before[species] + step.release[species], step.sample[species], step.release[species])
            || !closes(step.sample[species], step.airborne[species] + step.exit[species], step.exit[species])
            || !closes(injected[species], accounted[species], injected[species])) {
            reject(3u, species); return;
        }
    }
    let total = dot(injected, vec4<f32>(1.0));
    if (!closes(total, dot(accounted, vec4<f32>(1.0)), total)) {
        reject(4u, NO_FAULT); return;
    }
    ledger.injected = injected;
    ledger.horizontal = horizontal;
    ledger.airborne = step.airborne;
    ledger.injected_correction = injection.correction;
    ledger.horizontal_correction = exits.correction;
    ledger.last = step;
    ledger.status = vec4<u32>(0u, params.step, NO_FAULT, 1u);
}
