// advance_mod.f90:664-779: correct the boundary-adjusted predictor by half the wind difference.
// Only private timestep state is written; publication follows all status checks and downstream physics.
// Original regional eps uses nxmax / 3.e5; the vertical placement scales eps by 100 metres.
const EPS_NX_DIVISOR: f32 = 3.e5;
const TOP_MARGIN_SCALE_M: f32 = 100.0;
struct Particle {
    pos_x: f32, pos_y: f32, pos_z: f32, cell_x: i32, cell_y: i32, flags: u32,
    mass: array<f32, 4>, vel_u: f32, vel_v: f32, vel_w: f32,
    turb_u: f32, turb_v: f32, turb_w: f32, time: i32, timestep: i32,
    time_mem: i32, time_split: i32, release_point: i32, class_id: i32, cbt: i32, pad0: u32,
};
struct Params { capacity: u32, active_count: u32, nx: u32, ny: u32,
    dt: f32, x_scale: f32, y_scale: f32, target_levels: u32 };
@group(0) @binding(0) var<storage, read> original: array<Particle>;
@group(0) @binding(1) var<storage, read> velocities: array<f32>;
@group(0) @binding(2) var<storage, read_write> status: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read_write> staged: array<Particle>;
struct Query { cell_x: i32, cell_y: i32, fraction_x: f32, fraction_y: f32,
    height_agl_m: f32, is_active: u32, pad0: u32, pad1: u32 };
@group(0) @binding(4) var<storage, read> target_heights: array<f32>;
@group(0) @binding(5) var<storage, read> target_status: array<u32>;
@group(0) @binding(6) var<storage, read> predictor: array<Query>;
@group(0) @binding(7) var<uniform> params: Params;
fn failed() -> bool {
    for (var stage = 0u; stage < 6u; stage++) {
        if (atomicLoad(&status[stage * (params.capacity + 1u)]) != 0u) { return true; }
    }
    return false;
}
fn finite(v: f32) -> bool { return (bitcast<u32>(v) & 0x7f800000u) != 0x7f800000u; }
// Split-cell arithmetic never reconstructs a signed cell as a floating-point position.
fn moved(cell: i32, fraction: f32, delta: f32, n: u32) -> vec2<f32> {
    let local = fraction + delta;
    let bounded = clamp(local, -f32(cell), f32(i32(n - 1u) - cell));
    let shift = floor(bounded);
    return vec2<f32>(shift, bounded - shift);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.active_count || failed()) { return; }
    var p = original[lane];
    if ((p.flags & 1u) == 0u) { return; }
    let u0 = velocities[lane] + p.turb_u;
    let v0 = velocities[params.capacity + lane] + p.turb_v;
    let w0 = velocities[2u * params.capacity + lane];
    let u1 = velocities[3u * params.capacity + lane] + p.turb_u;
    let v1 = velocities[4u * params.capacity + lane] + p.turb_v;
    let w1 = velocities[5u * params.capacity + lane];
    let u = 0.5 * (u0 + u1);
    let v = 0.5 * (v0 + v1);
    let w = 0.5 * (w0 + w1);
    let dx = u * params.x_scale * params.dt;
    let dy = v * params.y_scale * params.dt;
    let top = target_heights[params.target_levels - 1u];
    let raw_predictor_z = p.pos_z + w0 * params.dt;
    // Retain the existing lower boundary; only the upper action changes the correction base.
    let base_z = select(raw_predictor_z, predictor[lane].height_agl_m, raw_predictor_z >= top);
    let correction_m = ((w1 - w0) * 0.5) * params.dt;
    let corrected_z = base_z + correction_m;
    let eps = f32(params.nx) / EPS_NX_DIVISOR;
    let margin_m = TOP_MARGIN_SCALE_M * eps;
    let placed_z = top - margin_m;
    if (!finite(u) || !finite(v) || !finite(w) || !finite(dx) || !finite(dy) || !finite(corrected_z)
        || target_status[0] != 2u || predictor[lane].is_active != 1u
        || !finite(top) || !finite(placed_z) || placed_z <= 0.0 || placed_z >= top) {
        atomicStore(&status[6u * (params.capacity + 1u)], 1u);
        return;
    }
    let x = moved(p.cell_x, p.pos_x, dx, params.nx);
    let y = moved(p.cell_y, p.pos_y, dy, params.ny);
    p.cell_x += i32(x.x); p.pos_x = x.y;
    p.cell_y += i32(y.x); p.pos_y = y.y;
    p.pos_z = max(select(corrected_z, placed_z, corrected_z >= top), 0.0);
    p.vel_u = u; p.vel_v = v; p.vel_w = w;
    staged[lane] = p;
}
