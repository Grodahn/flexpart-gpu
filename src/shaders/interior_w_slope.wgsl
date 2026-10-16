// FLEXPART 11.1 verttransform_mod.f90:625-655; windfields_mod.f90:630-631.
// W += interpolated native-center-height slope_x * U_shared * dxconst / cos(latitude)
//    + interpolated native-center-height slope_y * V_shared * dyconst.
// Same current-column native_center_height indices are used in every neighbor; no terrain term.
struct Parameters {
    nx:u32, ny:u32, levels:u32, padding:u32,
    dx_deg:f32, dy_deg:f32, latitude_origin_deg:f32, padding2:f32,
    x_enabled:u32, y_enabled:u32, padding3:vec2<u32>,
}
// Pinned par_mod.f90:61-62: radius in metres; angles in radians.
const EARTH_RADIUS_M:f32=6.371e6;
const PI:f32=3.14159265;
const DEGREE_TO_RADIAN:f32=PI/180.0;
@group(1) @binding(0) var<storage,read> native_uv:array<vec4<f32>>;
@group(1) @binding(1) var<storage,read> u:array<f32>;
@group(1) @binding(2) var<storage,read> v:array<f32>;
@group(1) @binding(3) var<storage,read> heights:array<f32>;
@group(1) @binding(4) var<storage,read> target_status:array<u32>;
@group(1) @binding(5) var<storage,read> uv_status:array<u32>;
@group(1) @binding(6) var<storage,read_write> output:array<f32>;
@group(1) @binding(7) var<storage,read_write> status:array<u32>;
@group(2) @binding(0) var<uniform> params:Parameters;
fn finite(v:f32)->bool{return abs(v)<=3.402823466e38;}
fn native_center_height(level:u32,column:u32)->f32{return native_uv[level*params.nx*params.ny+column].x;}
// Evaluate the pinned cosine on the accepted nonpolar interval [-pi/2,pi/2].
// Native GPU cosine error can exceed the final velocity budget at cancellation.
// The even Maclaurin polynomial through x^14 has < 7e-11 truncation error here.
// This retains f32 arithmetic and the pinned latitude factor.
fn latitude_cosine(angle:f32)->f32 {
    let t=angle*angle;
    return 1.0+t*(-0.5+t*(1.0/24.0+t*(-1.0/720.0+t*(1.0/40320.0+
        t*(-1.0/3628800.0+t*(1.0/479001600.0+t*(-1.0/87178291200.0)))))));
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    let column=id.x;let columns=params.nx*params.ny;
    if(column>=columns){return;}
    if(status[column]!=1u){return;}
    status[column]=3u;
    if(target_status[0]!=2u||uv_status[column]!=2u){return;}
    let x=column%params.nx;let y=column/params.nx;
    let interior=x>0u&&x<params.nx-1u&&y>0u&&y<params.ny-1u;
    if(!finite(params.dx_deg)||!finite(params.dy_deg)||params.dx_deg<=0.0||params.dy_deg<=0.0){return;}
    let latitude=(f32(y)*params.dy_deg+params.latitude_origin_deg)*DEGREE_TO_RADIAN;
    let cosine=latitude_cosine(latitude);
    let dxconst=180.0/(params.dx_deg*EARTH_RADIUS_M*PI);
    let dyconst=180.0/(params.dy_deg*EARTH_RADIUS_M*PI);
    if(!finite(cosine)||cosine<=0.0||!finite(dxconst)||!finite(dyconst)||dxconst<=0.0||dyconst<=0.0){return;}
    let inverse_latitude_cosine=1.0/cosine;
    // Validate full native native_center_height columns before using any same-index neighbor.
    var previous=-1.0;
    for(var level=0u;level<params.levels;level++) {
        let z=native_center_height(level,column);
        if(!finite(z)||z<=previous||(level==0u&&z!=0.0)){return;}
        previous=z;
    }
    if(interior) {
        if(uv_status[column-1u]!=2u||uv_status[column+1u]!=2u||uv_status[column-params.nx]!=2u||uv_status[column+params.nx]!=2u){return;}
    }
    var upper_index=1u;
    for(var level=1u;level<params.levels-1u;level++) {
        let z=heights[level];
        while(upper_index<params.levels-1u&&native_center_height(upper_index,column)<z){upper_index++;}
        let lower_height=native_center_height(upper_index-1u,column);let upper_height=native_center_height(upper_index,column);
        if(!finite(z)||z<=lower_height||z>upper_height){return;}
        let index=level*columns+column;
        if(!finite(u[index])||!finite(v[index])){status[column]=4u;return;}
        if(interior) {
            let dz1=z-lower_height;let dz2=upper_height-z;let dz=dz1+dz2;
            let slope_x_lower=(native_center_height(upper_index-1u,column+1u)-native_center_height(upper_index-1u,column-1u))*0.5;
            let slope_x_upper=(native_center_height(upper_index,column+1u)-native_center_height(upper_index,column-1u))*0.5;
            let slope_y_lower=(native_center_height(upper_index-1u,column+params.nx)-native_center_height(upper_index-1u,column-params.nx))*0.5;
            let slope_y_upper=(native_center_height(upper_index,column+params.nx)-native_center_height(upper_index,column-params.nx))*0.5;
            let slope_x=(slope_x_lower*dz2+slope_x_upper*dz1)/dz;
            let slope_y=(slope_y_lower*dz2+slope_y_upper*dz1)/dz;
            let x_contribution=slope_x*u[index]*dxconst*inverse_latitude_cosine*f32(params.x_enabled);
            let y_contribution=slope_y*v[index]*dyconst*f32(params.y_enabled);
            let value=output[index]+x_contribution+y_contribution;
            if(!finite(x_contribution)||!finite(y_contribution)||!finite(value)){status[column]=4u;return;}
            output[index]=value;
        }
    }
    status[column]=2u;
}
