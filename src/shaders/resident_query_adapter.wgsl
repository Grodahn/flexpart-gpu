// #171 split-coordinate adapter for #87's existing query geometry.
// Stencil/weights follow interpol_mod.f90:128-188; no field weighted sum occurs here.
// #76 compatible_column is checked per lane, never globally across batch lanes.
struct Query {
    cell_x: i32, cell_y: i32, fraction_x: f32, fraction_y: f32,
    height_agl_m: f32, is_active: u32, pad0: u32, pad1: u32,
};
struct HorizontalQuery {
    xt: f32, yt: f32, ix: u32, jy: u32, ddx: f32, ddy: f32, pad0: f32, pad1: f32,
};
struct Params {
    capacity: u32, active_count: u32, nx: u32, ny: u32,
    periodic: u32, levels: u32, model: u32, pad: u32,
};
@group(0) @binding(0) var<storage, read> queries: array<Query>;
@group(0) @binding(1) var<storage, read_write> horizontal: array<HorizontalQuery>;
@group(0) @binding(2) var<storage, read> source_heights: array<f32>;
@group(0) @binding(3) var<storage, read> source_values: array<f32>;
@group(0) @binding(4) var<storage, read_write> lane_heights: array<f32>;
@group(0) @binding(5) var<storage, read_write> query_heights: array<f32>;
@group(0) @binding(6) var<storage, read_write> status: array<atomic<u32>>;
@group(0) @binding(7) var<uniform> params: Params;

fn finite(v: f32) -> bool { return (bitcast<u32>(v) & 0x7f800000u) != 0x7f800000u; }
fn fail(lane: u32, reason: u32) {
    if (atomicLoad(&status[lane + 1u]) <= 2u) { atomicStore(&status[lane + 1u], reason); }
    atomicOr(&status[0], 1u);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.capacity) { return; }
    // Safe placeholders precede every rejection. #87/#88 never see invalid indices/heights.
    horizontal[lane] = HorizontalQuery(0.0, 0.0, 0u, 0u, 0.0, 0.0, 0.0, 0.0);
    query_heights[lane] = 0.0;
    if (params.model == 1u) {
        for (var level = 0u; level < params.levels; level += 1u) {
            lane_heights[lane * params.levels + level] = source_heights[level];
        }
    }
    let q = queries[lane];
    if (lane >= params.active_count || q.is_active == 0u) {
        atomicStore(&status[lane + 1u], 0u);
        return;
    }
    if (!finite(q.fraction_x) || !finite(q.fraction_y) || !finite(q.height_agl_m)) { fail(lane, 5u); return; }
    // Validate signed integers before any unsigned conversion, including i32::MIN.
    if (q.cell_x < 0 || q.cell_y < 0) { fail(lane, 3u); return; }
    if (q.fraction_x < 0.0 || q.fraction_x >= 1.0 || q.fraction_y < 0.0 || q.fraction_y >= 1.0) { fail(lane, 4u); return; }
    let ix = u32(q.cell_x);
    let jy = u32(q.cell_y);
    if (ix >= params.nx || jy >= params.ny
        || (params.periodic == 0u && ix == params.nx - 1u && q.fraction_x != 0.0)
        || (jy == params.ny - 1u && q.fraction_y != 0.0)) { fail(lane, 6u); return; }
    let ixp = select(min(ix + 1u, params.nx - 1u), (ix + 1u) % params.nx, params.periodic == 1u);
    let jyp = min(jy + 1u, params.ny - 1u);
    let base_column = ix + params.nx * jy;
    let dx = q.fraction_x;
    let dy = q.fraction_y;
    let cells = params.nx * params.ny;
    for (var level = 0u; level < params.levels; level += 1u) {
        for (var corner = 0u; corner < 4u; corner += 1u) {
            // Scalar selection avoids Vulkan compiler failures with dynamically indexed
            // function-local arrays while preserving the canonical corner order.
            let east = (corner & 1u) != 0u;
            let north = (corner & 2u) != 0u;
            let column = select(ix, ixp, east) + params.nx * select(jy, jyp, north);
            // Factor eligibility avoids underflow of tiny f32 weight products.
            let eligible = (!east || dx != 0.0) && (!north || dy != 0.0);
            if (!finite(source_values[level * cells + column])) { fail(lane, 8u); return; }
            if (params.model == 1u && eligible
                && source_heights[column * params.levels + level] != source_heights[base_column * params.levels + level]) { fail(lane, 7u); return; }
        }
    }
    horizontal[lane] = HorizontalQuery(0.0, 0.0, ix, jy, dx, dy, 0.0, 0.0);
    query_heights[lane] = q.height_agl_m;
    if (params.model == 1u) {
        for (var level = 0u; level < params.levels; level += 1u) {
            lane_heights[lane * params.levels + level] = source_heights[base_column * params.levels + level];
        }
    }
    if (atomicLoad(&status[lane + 1u]) == 1u) { atomicStore(&status[lane + 1u], 2u); }
}
