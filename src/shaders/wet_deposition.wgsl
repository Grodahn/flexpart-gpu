// WGSL per-species wet deposition probability kernel (D-04).
//
// Ported from FLEXPART `wetdepo.f90` mass-loss update form, applied per
// species slot `s`:
//   p_s = grfraction * (1 - exp(-wetscav_s * |dt|))
//
// Here:
// - wetscav_s is provided as per-particle per-species scavenging coefficient [1/s]
// - grfraction is provided as per-particle precipitating fraction [-] (shared
//   across species: the precipitating sub-grid area is geometric, see
//   `get_wetscav.f90` `grfraction` logic)
// - dt is a dispatch-wide timestep [s]
//
// MVP assumption:
// - wet scavenging coefficients/pathway branching are precomputed upstream
//   (D-03 CPU formulas), and this kernel only applies the wetdepo step.
//
// Side effect:
//   particle.mass[s] *= (1 - p_s) for each species slot.
//
// Buffer contract:
// - binding(0): particles storage buffer (read_write)
// - binding(1): scavenging_coefficient_s_inv per particle per species as vec4 (read)
// - binding(2): precipitating_fraction per particle (read)
// - binding(3): wet_deposition_probability per particle per species as vec4 (read_write)
// - binding(4): uniform params (particle_count, dt_seconds, pad0, pad1)
//
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

struct WetDepositionDispatchParams {
    particle_count: u32,
    dt_seconds: f32,
    _pad0: f32,
    _pad1: f32,
};

@group(0) @binding(0)
var<storage, read_write> particles: array<Particle>;

@group(0) @binding(1)
var<storage, read> scavenging_coefficient_s_inv: array<vec4<f32>>;

@group(0) @binding(2)
var<storage, read> precipitating_fraction: array<f32>;

@group(0) @binding(3)
var<storage, read_write> wet_deposition_probability: array<vec4<f32>>;

@group(0) @binding(4)
var<uniform> params: WetDepositionDispatchParams;

fn wet_probability(lambda_s_inv: f32, dt_seconds: f32, precip_fraction: f32) -> f32 {
    let lambda = max(lambda_s_inv, 0.0);
    let dt = abs(dt_seconds);
    let fraction = clamp(precip_fraction, 0.0, 1.0);
    if (lambda <= 0.0 || dt <= 0.0 || fraction <= 0.0) {
        return 0.0;
    }
    let local_probability = 1.0 - exp(-lambda * dt);
    return clamp(local_probability * fraction, 0.0, 1.0);
}

@compute @workgroup_size(__WORKGROUP_SIZE_X__)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let particle_id = gid.y * (nwg.x * __WORKGROUP_SIZE_X__u) + gid.x;
    if (particle_id >= params.particle_count) {
        return;
    }

    var particle = particles[particle_id];
    if ((particle.flags & FLAG_ACTIVE) == 0u) {
        wet_deposition_probability[particle_id] = vec4<f32>(0.0);
        return;
    }

    let lambda = scavenging_coefficient_s_inv[particle_id];
    let fraction = precipitating_fraction[particle_id];
    var probability = vec4<f32>(0.0);
    for (var s = 0; s < 4; s++) {
        let p = wet_probability(lambda[s], params.dt_seconds, fraction);
        probability[s] = p;
        particle.mass[s] = particle.mass[s] * (1.0 - p);
    }

    particles[particle_id] = particle;
    wet_deposition_probability[particle_id] = probability;
}
