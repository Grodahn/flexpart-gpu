// WGSL gravitational settling velocity kernel for issue #35.
//
// Ported from FLEXPART 11.1 `settling_mod.f90:get_settling` (sphere branch,
// `ishape == 0`) with initialization from `drydepo_mod.f90:part0` and
// constants from `par_mod.f90`.
//
// Spherical settling only. Non-spherical shapes (`PSHAPE != 0`, Bagheri &
// Bonadonna) are rejected on the host and never reach this kernel.
//
// For one query with volume-equivalent diameter `d`, particle density
// `rho_p`, air temperature `T` and moist-air density `rho_air`:
//
//   viscosity (Sutherland, `settling_mod.f90:viscosity`):
//     mu = ETA0 * (T0 + C) / (T + C) * (T / T0)^1.5
//   slip correction (`drydepo_mod.f90:part0`, single diameter bin):
//     Kn = 2 * LAM / d_m
//     alpha = 1.257 + 0.4 * exp(-1.1 / Kn)  (1.257 when Kn <= 0.0126, Fortran
//       underflow guard for `eps = 1.2e-38`)
//     Cun = 1 + alpha * Kn
//   Stokes init (`part0`: `vsh = g * rho_p * d_m^2 * Cun / (18 * MYL)`):
//     v_old = -vsh
//     Re = d_m * |v_old| / (mu / rho_air)
//   Clift-Gauvin iteration (`settling_mod.f90:get_settling`, up to 20 steps,
//   exit when `|v - v_old| / |v| < 0.01`):
//     Cd = 24/Re                         (Re <= 0.02)
//     Cd = 24/Re*(1+0.15*Re^0.687) + 0.42/(1+42500/Re^1.16)  (otherwise)
//     v = -sqrt(4*g*d_m*rho_p*Cun / (3*Cd*rho_air))
//     Re = d_m * |v| / (mu / rho_air)
//
// Units:
//   diameter_um: [um], converted to [m] via /1e6
//   particle density, air density: [kg/m3]
//   temperature: [K]
//   settling velocity: [m/s], negative (downward, added to upward-positive
//     vertical wind). Settling alone never removes mass.
//
// Buffer contract:
// - binding(0): settling queries as `array<SettlingQuery>` (read)
// - binding(1): settling velocities as `array<f32>` (read_write)
// - binding(2): uniform params (query_count, pad, pad, pad)
//
// Host validates all inputs (finite, positive, in-domain, spherical) before
// dispatch. The kernel assumes valid inputs; non-finite outputs are rejected
// on download at the explicit validation boundary.

struct SettlingQuery {
    diameter_um: f32,
    particle_density_kg_m3: f32,
    temperature_k: f32,
    air_density_kg_m3: f32,
};

struct SettlingParams {
    query_count: u32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0)
var<storage, read> queries: array<SettlingQuery>;

@group(0) @binding(1)
var<storage, read_write> velocities: array<f32>;

@group(0) @binding(2)
var<uniform> params: SettlingParams;

// Dynamic viscosity of air [kg/(m*s)] via Sutherland's formula.
// Ported from `settling_mod.f90:viscosity`.
fn sutherland_viscosity(temperature_k: f32) -> f32 {
    let c: f32 = 120.0;
    let t0: f32 = 291.15;
    let eta0: f32 = 1.827e-5;
    return eta0 * (t0 + c) / (temperature_k + c) * pow(temperature_k / t0, 1.5);
}

// Cunningham slip correction [-] for sphere diameter `d_m` [m].
// Ported from `drydepo_mod.f90:part0` (single bin, `dmean == d`).
fn cunningham_slip(diameter_m: f32) -> f32 {
    let lam: f32 = 6.53e-8;
    let kn: f32 = 2.0 * lam / diameter_m;
    // Fortran guard `(-1.1/Kn) <= ln(1.2e-38)` avoids `exp` underflow;
    // `exp` underflows to zero identically, so both paths yield 1.257.
    var alpha: f32 = 1.257;
    if (kn > 0.012597) {
        alpha = 1.257 + 0.4 * exp(-1.1 / kn);
    }
    return 1.0 + alpha * kn;
}

// Drag coefficient [-] for Reynolds number `re` (Clift-Gauvin 1971).
// Ported from `settling_mod.f90:get_settling` sphere branch.
fn drag_coefficient(reynolds: f32) -> f32 {
    if (reynolds <= 0.02) {
        return 24.0 / reynolds;
    }
    return (24.0 / reynolds) * (1.0 + 0.15 * pow(reynolds, 0.687))
        + 0.42 / (1.0 + 42500.0 / pow(reynolds, 1.16));
}

fn settling_velocity_sphere(
    diameter_um: f32,
    particle_density: f32,
    temperature_k: f32,
    air_density: f32,
) -> f32 {
    let ga: f32 = 9.81;
    let myl: f32 = 1.81e-5;
    let diameter_m: f32 = diameter_um / 1.0e6;
    let cun: f32 = cunningham_slip(diameter_m);
    // Stokes initialization from `part0` (`vsetaver = -vsh`).
    let stokes_magnitude: f32 =
        ga * particle_density * diameter_m * diameter_m * cun / (18.0 * myl);
    var settling_old: f32 = -stokes_magnitude;
    let vis_dyn: f32 = sutherland_viscosity(temperature_k);
    let vis_kin: f32 = vis_dyn / air_density;
    var reynolds: f32 = diameter_m * abs(settling_old) / vis_kin;
    var settling: f32 = settling_old;
    for (var i = 0; i < 20; i++) {
        let cd: f32 = drag_coefficient(reynolds);
        settling = -sqrt(
            4.0 * ga * diameter_m * particle_density * cun / (3.0 * cd * air_density)
        );
        if (abs((settling - settling_old) / settling) < 0.01) {
            break;
        }
        reynolds = diameter_m * abs(settling) / vis_kin;
        settling_old = settling;
    }
    return settling;
}

@compute @workgroup_size(__WORKGROUP_SIZE_X__)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let query_id = gid.y * (nwg.x * __WORKGROUP_SIZE_X__u) + gid.x;
    if (query_id >= params.query_count) {
        return;
    }
    let query = queries[query_id];
    velocities[query_id] = settling_velocity_sphere(
        query.diameter_um,
        query.particle_density_kg_m3,
        query.temperature_k,
        query.air_density_kg_m3,
    );
}
