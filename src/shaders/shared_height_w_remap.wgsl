// FLEXPART 11.1 verttransform_mod.f90:489-545,615-621 (eta=no).
// W = (normalized_lower*dz2 + normalized_upper*dz1)/(dz1+dz2).
// Distinct W/interface heights; endpoint target levels copy native interfaces.
struct Parameters {
    nx:u32, ny:u32, levels:u32, padding:u32,
    dx_deg:f32, dy_deg:f32, latitude_origin_deg:f32, padding2:f32,
    x_enabled:u32, y_enabled:u32, padding3:vec2<u32>,
}
@group(1) @binding(0) var<storage,read> native_w:array<vec2<f32>>;
@group(1) @binding(1) var<storage,read> heights:array<f32>;
@group(1) @binding(2) var<storage,read> target_status:array<u32>;
@group(1) @binding(3) var<storage,read_write> output:array<f32>;
@group(1) @binding(4) var<storage,read_write> status:array<u32>;
@group(2) @binding(0) var<uniform> params:Parameters;
fn finite(v:f32)->bool {return abs(v)<=3.402823466e38;}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    let column=id.x; let columns=params.nx*params.ny;
    if(column>=columns){return;}
    status[column]=3u;
    if(target_status[0]!=2u){return;}
    var previous=-1.0; var previous_target=-1.0;
    for(var level=0u;level<params.levels;level++) {
        let lane=native_w[level*columns+column]; let z=heights[level];
        if(!finite(lane.x)||!finite(lane.y)||lane.x<=previous||!finite(z)||z<=previous_target||
            (level==0u&&(lane.x!=0.0||z!=0.0))){return;}
        previous=lane.x; previous_target=z;
    }
    var upper_index=1u;
    for(var level=0u;level<params.levels;level++) {
        let z=heights[level]; var value=native_w[(params.levels-1u)*columns+column].y;
        if(level==0u){value=native_w[column].y;}
        else if(level!=params.levels-1u) {
            while(upper_index<params.levels-1u&&native_w[upper_index*columns+column].x<z){upper_index++;}
            let lower_lane=native_w[(upper_index-1u)*columns+column];let upper_lane=native_w[upper_index*columns+column];
            if(z<=lower_lane.x||z>upper_lane.x){return;}
            let dz1=z-lower_lane.x;let dz2=upper_lane.x-z;
            value=(lower_lane.y*dz2+upper_lane.y*dz1)/(dz1+dz2);
        }
        if(!finite(value)){status[column]=4u;return;}
        output[level*columns+column]=value;
    }
    // Remapped only: slope pass is the sole owner of completed status 2.
    status[column]=1u;
}
