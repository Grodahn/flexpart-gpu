// Petterssen (1940): x_predict = x_current + dt * (canonical mean wind + horizontal turbulence).
// Sampling belongs exclusively to #87/#88/#89. Particle scientific state is read-only.
struct Particle {
    pos_x: f32, pos_y: f32, pos_z: f32, cell_x: i32, cell_y: i32, flags: u32,
    mass: array<f32, 4>, vel_u: f32, vel_v: f32, vel_w: f32,
    turb_u: f32, turb_v: f32, turb_w: f32, time: i32, timestep: i32,
    time_mem: i32, time_split: i32, release_point: i32, class_id: i32, cbt: i32, pad0: u32,
};
struct Query {
    cell_x: i32, cell_y: i32, fraction_x: f32, fraction_y: f32,
    height_agl_m: f32, is_active: u32, pad0: u32, pad1: u32,
};
struct Params { capacity: u32, active_count: u32, nx: u32, ny: u32,
    dt: f32, x_scale: f32, y_scale: f32, top_m: f32 };
@group(0) @binding(0) var<storage, read> particles: array<Particle>;
@group(0) @binding(1) var<storage, read> velocities: array<f32>;
@group(0) @binding(2) var<storage, read> status: array<u32>;
@group(0) @binding(3) var<storage, read_write> queries: array<Query>;
@group(0) @binding(4) var<uniform> params: Params;
fn failed() -> bool {
    for (var stage = 0u; stage < 6u; stage++) {
        if (status[stage * (params.capacity + 1u)] != 0u) { return true; }
    }
    return false;
}
fn finite(v: f32) -> bool { return abs(v) <= 3.402823466e38; }
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
    if (lane >= params.capacity) { return; }
    queries[lane] = Query(0, 0, 0.0, 0.0, 0.0, 0u, 0u, 0u);
    if (lane >= params.active_count) { return; }
    let p = particles[lane];
    if ((p.flags & 1u) == 0u) { return; }
    // Preserve an initial failure in its own status owner; later resets cannot erase it.
    for (var stage = 0u; stage < 3u; stage++) {
        if (status[stage * (params.capacity + 1u)] != 0u) { return; }
    }
    let u = velocities[lane] + p.turb_u;
    let v = velocities[params.capacity + lane] + p.turb_v;
    let w = velocities[2u * params.capacity + lane];
    let dx = u * params.x_scale * params.dt;
    let dy = v * params.y_scale * params.dt;
    let dz = w * params.dt;
    if (!finite(dx) || !finite(dy) || !finite(dz)) {
        queries[lane] = Query(p.cell_x, p.cell_y, bitcast<f32>(0x7fc00000u), p.pos_y, p.pos_z, 1u, 0u, 0u);
        return;
    }
    let x = moved(p.cell_x, p.pos_x, dx, params.nx);
    let y = moved(p.cell_y, p.pos_y, dy, params.ny);
    queries[lane] = Query(p.cell_x + i32(x.x), p.cell_y + i32(y.x), x.y, y.y,
        clamp(p.pos_z + dz, 0.0, params.top_m), 1u, 0u, 0u);
}
