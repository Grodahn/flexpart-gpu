// Canonical horizontal meteorology interpolation kernel for issue #87.
//
// Ported from FLEXPART 11.1 `interpol_mod.f90:128-166` (`find_grid_indices`,
// mother-grid path), `interpol_mod.f90:168-188` (`find_grid_distances`) and
// `interpol_mod.f90:481-504` (`hor_interpol_4d`):
//   ix = int(xt), jy = int(yt)
//   ixp = ix + 1 (periodic seam uses duplicate ghost column nx, regional exact
//     last column collapses to ix)
//   jyp = jy + 1 (exact last row collapses to jy, the pinned "temporary fix")
//   ddx = xt - ix, ddy = yt - jy
//   p1 = (1-ddx)*(1-ddy) for (ix,jy), p2 = ddx*(1-ddy) for (ixp,jy),
//   p3 = (1-ddx)*ddy for (ix,jyp), p4 = ddx*ddy for (ixp,jyp)
//   output = p1*f(ix,jy) + p2*f(ixp,jy) + p3*f(ix,jyp) + p4*f(ixp,jyp)
//
// Host responsibilities (Rust, before dispatch) preserve the exact #72
// contract in f64:
// - validate canonical #29 grid metadata and periodicity (nx*dx == 360);
// - require CellCenter staggering; face staggering fails closed;
// - validate field shape nx*ny in x-fastest order (offset = x + nx*y);
// - validate coordinates in the supported domain (non-periodic
//   0 <= xt <= nx-1 and 0 <= yt <= ny-1; periodic 0 <= xt < nx and
//   0 <= yt <= ny-1) and longitude convention; out-of-domain fails closed;
// - validate sampled corners are finite; geographic lon/lat mapping with exact
//   endpoint snapping stays on the host so provider metadata never enters here.
//
// GPU responsibilities (this shader, f32):
// - recompute indices, fractional distances, weights and the weighted sum for
//   pre-validated coordinates entirely on the device;
// - preserve x/y flattening, weight ordering and the exact evaluation order
//   p1*f00 + p2*f10 + p3*f01 + p4*f11;
// - map the periodic ghost column nx to stored column 0; collapse exact last
//   row/column exactly as #72;
// - write NaN for defensively detected out-of-domain or out-of-bounds work so
//   the host fails closed; no silent clamping or wrapping.
//
// Buffer contract:
// - binding(0): field values, array<f32> with nx*ny elements in x-fastest
//   order, read-only (device-resident for #76 composition)
// - binding(1): queries, array<HorizontalQuery> with query_count elements,
//   read-only (per-dispatch lifetime)
// - binding(2): sampled outputs, array<f32> with query_count elements,
//   read-write (device-resident for #76; host readback only at explicit
//   validation/output boundaries)
// - binding(3): uniform params (nx, ny, query_count, is_periodic_x)

struct HorizontalQuery {
    xt: f32,
    yt: f32,
    _pad0: f32,
    _pad1: f32,
};

struct HorizontalParams {
    nx: u32,
    ny: u32,
    query_count: u32,
    is_periodic_x: u32,
};

@group(0) @binding(0)
var<storage, read> field_values: array<f32>;

@group(0) @binding(1)
var<storage, read> queries: array<HorizontalQuery>;

@group(0) @binding(2)
var<storage, read_write> sampled_values: array<f32>;

@group(0) @binding(3)
var<uniform> params: HorizontalParams;

fn write_invalid(idx: u32) {
    // Quiet NaN sentinel for defensively detected invalid work. The host
    // fails closed on non-finite outputs. A bitcast avoids a WGSL
    // constant-folded NaN literal, which parsers reject.
    sampled_values[idx] = bitcast<f32>(0x7fc00000u);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let idx = gid.y * (nwg.x * 64u) + gid.x;
    if (idx >= params.query_count) {
        return;
    }
    if (params.nx == 0u || params.ny == 0u) {
        write_invalid(idx);
        return;
    }

    let query = queries[idx];
    let xt = query.xt;
    let yt = query.yt;
    let nx_f = f32(params.nx);
    let ny_f = f32(params.ny);

    // Defensive domain check. The host already validated the f64 coordinates
    // against the supported #72 domain and fails closed before dispatch.
    // NaN fails every comparison below and lands here as invalid.
    var in_domain = false;
    if (params.is_periodic_x == 1u) {
        in_domain = xt >= 0.0 && xt < nx_f && yt >= 0.0 && yt <= ny_f - 1.0;
    } else {
        in_domain = xt >= 0.0 && xt <= nx_f - 1.0 && yt >= 0.0 && yt <= ny_f - 1.0;
    }
    if (!in_domain) {
        write_invalid(idx);
        return;
    }

    let ix = u32(floor(xt));
    let jy = u32(floor(yt));
    if (ix >= params.nx || jy >= params.ny) {
        write_invalid(idx);
        return;
    }
    let ddx = xt - f32(ix);
    let ddy = yt - f32(jy);
    if (ddx < 0.0 || ddx > 1.0 || ddy < 0.0 || ddy > 1.0) {
        write_invalid(idx);
        return;
    }

    var ixp: u32;
    var jyp: u32;
    if (params.is_periodic_x == 1u) {
        ixp = ix + 1u;
        if (ixp > params.nx) {
            write_invalid(idx);
            return;
        }
        if (jy + 1u >= params.ny) {
            jyp = jy;
        } else {
            jyp = jy + 1u;
        }
    } else {
        if (ix + 1u >= params.nx) {
            ixp = ix;
        } else {
            ixp = ix + 1u;
        }
        if (jy + 1u >= params.ny) {
            jyp = jy;
        } else {
            jyp = jy + 1u;
        }
    }

    let rddx = 1.0 - ddx;
    let rddy = 1.0 - ddy;
    let p1 = rddx * rddy;
    let p2 = ddx * rddy;
    let p3 = rddx * ddy;
    let p4 = ddx * ddy;

    // Periodic ghost column nx mirrors stored column 0; it is not stored.
    var stored_ixp = ixp;
    if (params.is_periodic_x == 1u && ixp == params.nx) {
        stored_ixp = 0u;
    }
    if (ix >= params.nx || stored_ixp >= params.nx || jy >= params.ny || jyp >= params.ny) {
        write_invalid(idx);
        return;
    }

    let nx = params.nx;
    let offset_00 = ix + nx * jy;
    let offset_10 = stored_ixp + nx * jy;
    let offset_01 = ix + nx * jyp;
    let offset_11 = stored_ixp + nx * jyp;
    let field_len = arrayLength(&field_values);
    if (offset_00 >= field_len || offset_10 >= field_len || offset_01 >= field_len || offset_11 >= field_len) {
        write_invalid(idx);
        return;
    }

    let value_00 = field_values[offset_00];
    let value_10 = field_values[offset_10];
    let value_01 = field_values[offset_01];
    let value_11 = field_values[offset_11];

    // Exact FLEXPART ordering: p1*f(ix,jy) + p2*f(ixp,jy) + p3*f(ix,jyp) + p4*f(ixp,jyp).
    sampled_values[idx] = p1 * value_00 + p2 * value_10 + p3 * value_01 + p4 * value_11;
}
