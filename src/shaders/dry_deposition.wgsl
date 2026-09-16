// WGSL per-species dry deposition probability kernel (D-02).
//
// Ported from FLEXPART `advance.f90` / `get_vdep_prob.f90`.
//
// For active particles in the dry-deposition layer (z < 2*href), compute per
// species slot `s`:
//   survival_s = exp(-max(vdep_s, 0) * |dt| / (2*href))
//   probability_s = 1 - survival_s
//
// Units:
//   vdep: [m/s]
//   dt:   [s]
//   href: [m]
//   exponent argument: dimensionless
//
// Side effect:
//   particle.mass[s] *= survival_s for each species slot.
//
// Buffer contract:
// - binding(0): particles storage buffer (read_write)
// - binding(1): deposition_velocity_m_s per particle per species as vec4 (read)
// - binding(2): deposition_probability per particle per species as vec4 (read_write)
// - binding(3): uniform params (particle_count, dt_seconds, reference_height_m, pad)

const FLAG_ACTIVE: u32 = 1u;
const HREF_MIN_M: f32 = 0.1;

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

struct DryDepositionDispatchParams {
    particle_count: u32,
    dt_seconds: f32,
    reference_height_m: f32,
    _pad0: f32,
};

@group(0) @binding(0)
var<storage, read_write> particles: array<Particle>;

@group(0) @binding(1)
var<storage, read> deposition_velocity_m_s: array<vec4<f32>>;

@group(0) @binding(2)
var<storage, read_write> deposition_probability: array<vec4<f32>>;

@group(0) @binding(3)
var<uniform> params: DryDepositionDispatchParams;

fn compute_survival_factor(vdep_m_s: f32, dt_seconds: f32, href_m: f32) -> f32 {
    let href = max(href_m, HREF_MIN_M);
    let exponent = -max(vdep_m_s, 0.0) * abs(dt_seconds) / (2.0 * href);
    return clamp(exp(exponent), 0.0, 1.0);
}

@compute @workgroup_size(__WORKGROUP_SIZE_X__)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let particle_id = gid.y * (nwg.x * __WORKGROUP_SIZE_X__u) + gid.x;
    if (particle_id >= params.particle_count) {
        return;
    }

    var particle = particles[particle_id];
    if ((particle.flags & FLAG_ACTIVE) == 0u) {
        deposition_probability[particle_id] = vec4<f32>(0.0);
        return;
    }

    let href = max(params.reference_height_m, HREF_MIN_M);
    if (particle.pos_z >= 2.0 * href) {
        deposition_probability[particle_id] = vec4<f32>(0.0);
        return;
    }

    let vdep = deposition_velocity_m_s[particle_id];
    var probability = vec4<f32>(0.0);
    for (var s = 0; s < 4; s++) {
        let survival = compute_survival_factor(vdep[s], params.dt_seconds, href);
        probability[s] = 1.0 - survival;
        particle.mass[s] = particle.mass[s] * survival;
    }

    particles[particle_id] = particle;
    deposition_probability[particle_id] = probability;
}
