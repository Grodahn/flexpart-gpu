// WGSL per-species radioactive decay kernel.
//
// Ported from FLEXPART decay handling:
// - `drydepo_mod.f90:337-338` (`decfact = exp(-|lsynctime| * decay)`)
// - `unc_mod.f90:225` (`exp(-outstep * decay)` in `deposit_decay`)
//
// Applies to every active particle regardless of position:
//   mass[s] *= exp(-max(lambda_s, 0) * |dt|)
//
// The decay survival factor commutes with deposition survival factors, so
// dispatch order relative to dry/wet deposition does not affect the result.
//
// Units:
//   lambda: [1/s]
//   dt:     [s]
//   exponent argument: dimensionless
//
// Buffer contract:
// - binding(0): particles storage buffer (read_write)
// - binding(1): uniform params (particle_count, dt_seconds, pad, decay_constants_s_inv vec4)
const FLAG_ACTIVE: u32 = 1u;

struct Particle {
    pos_x: f32,
    pos_y: f32,
    pos_z: f32,
    cell_x: i32,
    cell_y: i32,
    flags: u32,
    mass: array<f32, 4>,
    vel_u: f32,
    vel_v: f32,
    vel_w: f32,
    turb_u: f32,
    turb_v: f32,
    turb_w: f32,
    time: i32,
    timestep: i32,
    time_mem: i32,
    time_split: i32,
    release_point: i32,
    class_id: i32,
    cbt: i32,
    pad0: u32,
};

struct DecayDispatchParams {
    particle_count: u32,
    dt_seconds: f32,
    _pad0: f32,
    _pad1: f32,
    decay_constants_s_inv: vec4<f32>,
};

@group(0) @binding(0)
var<storage, read_write> particles: array<Particle>;

@group(0) @binding(1)
var<uniform> params: DecayDispatchParams;

@compute @workgroup_size(__WORKGROUP_SIZE_X__)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let particle_id = gid.y * (nwg.x * __WORKGROUP_SIZE_X__u) + gid.x;
    if (particle_id >= params.particle_count) {
        return;
    }

    var particle = particles[particle_id];
    if ((particle.flags & FLAG_ACTIVE) == 0u) {
        return;
    }

    let dt = abs(params.dt_seconds);
    for (var s = 0; s < 4; s++) {
        let lambda = max(params.decay_constants_s_inv[s], 0.0);
        if (lambda > 0.0 && dt > 0.0) {
            particle.mass[s] = particle.mass[s] * clamp(exp(-lambda * dt), 0.0, 1.0);
        }
    }

    particles[particle_id] = particle;
}
