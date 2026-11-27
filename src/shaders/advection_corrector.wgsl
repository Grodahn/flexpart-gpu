// advance_mod.f90:664-779: correct the boundary-adjusted predictor by half the wind difference.
// Only private timestep state is written; publication follows all status checks and downstream physics.
// advance_mod.f90:879,893: params.top_margin_m is the original f32 100*(nxmax/3.e5), evaluated
// once on the host because WGSL division is not correctly rounded and may be contracted.
struct Particle {
    pos_x: f32, pos_y: f32, pos_z: f32, cell_x: i32, cell_y: i32, flags: u32,
    mass: array<f32, 4>, vel_u: f32, vel_v: f32, vel_w: f32,
    turb_u: f32, turb_v: f32, turb_w: f32, time: i32, timestep: i32,
    time_mem: i32, time_split: i32, release_point: i32, class_id: i32, cbt: i32, pad0: u32,
};
struct Params { capacity: u32, active_count: u32, nx: u32, ny: u32,
    dt: f32, x_scale: f32, y_scale: f32, target_levels: u32,
    top_margin_m: f32, stop_route: u32, forward_exits: u32, periodic_x: u32 };
@group(0) @binding(0) var<storage, read> original: array<Particle>;
@group(0) @binding(1) var<storage, read> velocities: array<f32>;
@group(0) @binding(2) var<storage, read_write> status: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read_write> staged: array<Particle>;
@group(0) @binding(4) var<storage, read> target_heights: array<f32>;
@group(0) @binding(5) var<storage, read> target_status: array<u32>;
@group(0) @binding(6) var<storage, read> predictor_z: array<f32>;
@group(0) @binding(7) var<storage, read_write> stops: array<u32>;
@group(0) @binding(8) var<uniform> params: Params;
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
// advance_mod.f90:857-867 uses an exclusive east and inclusive north edge.
fn horizontal_stop(p: Particle, dx: f32, dy: f32) -> u32 {
    if (params.forward_exits == 0u) { return 0u; }
    let x = p.pos_x + dx;
    let y = p.pos_y + dy;
    if (y < -f32(p.cell_y) || y > f32(i32(params.ny - 1u) - p.cell_y)) {
        // A longitudinally periodic grid needs a separate polar transport proof.
        return select(4u, 8u, params.periodic_x != 0u);
    }
    if (params.periodic_x == 0u &&
        (x < -f32(p.cell_x) || x >= f32(i32(params.nx - 1u) - p.cell_x))) { return 4u; }
    return 0u;
}
fn moved_x(p: Particle, dx: f32) -> vec2<f32> {
    if (params.forward_exits != 0u && params.periodic_x != 0u) {
        let period = f32(params.nx);
        let local = (p.pos_x + dx) % period;
        let shift = floor(local);
        let cell = ((p.cell_x + i32(shift)) % i32(params.nx) + i32(params.nx)) % i32(params.nx);
        return vec2<f32>(f32(cell - p.cell_x), local - shift);
    }
    return moved(p.cell_x, p.pos_x, dx, params.nx);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.active_count || failed()) { return; }
    var p = original[lane];
    if ((p.flags & 1u) == 0u || (stops[lane] & 12u) != 0u) { return; }
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
    let placed_z = top - params.top_margin_m;
    // Consume the predictor's own height so both stages share one boundary decision.
    let raw_predictor_z = predictor_z[lane];
    // Retain the existing lower boundary; only the upper action changes the correction base.
    var base_z = raw_predictor_z;
    // A stopped predictor retains its raw state. Only the ordinary route can place it.
    // On the stop route, consume bit 0 without repeating the predictor predicate.
    let predictor_stopped = (stops[lane] & 1u) != 0u;
    if (!predictor_stopped && params.stop_route == 0u) {
        base_z = select(raw_predictor_z, placed_z, raw_predictor_z >= top);
    }
    let correction_m = ((w1 - w0) * 0.5) * params.dt;
    let corrected_z = base_z + correction_m;
    if (!finite(u) || !finite(v) || !finite(w) || !finite(dx) || !finite(dy) || !finite(corrected_z)
        || target_status[0] != 2u || !finite(raw_predictor_z)
        || !finite(top) || !finite(placed_z) || placed_z <= 0.0 || placed_z >= top) {
        atomicStore(&status[6u * (params.capacity + 1u)], 1u);
        return;
    }
    let horizontal = horizontal_stop(p, dx, dy);
    if (horizontal != 0u) { stops[lane] |= horizontal; return; }
    let x = moved_x(p, dx);
    let y = moved(p.cell_y, p.pos_y, dy, params.ny);
    p.cell_x += i32(x.x); p.pos_x = x.y;
    p.cell_y += i32(y.x); p.pos_y = y.y;
    let stopped = params.stop_route != 0u && corrected_z >= top;
    if (stopped) { stops[lane] |= 2u; }
    p.pos_z = max(select(corrected_z, placed_z, corrected_z >= top && !stopped), 0.0);
    p.vel_u = u; p.vel_v = v; p.vel_w = w;
    staged[lane] = p;
}
