// The client's ring elevations: sky colours at 90°, 16.8°, 9.8°, 3.7° and 1.8°, fog at and below
// the horizon, linear between rings (per dome vertex in the client, per fragment here). The client
// bakes the dawn/dusk warp at 24 azimuth segments.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}

struct SkyColors {
    sky0: vec4<f32>,
    sky1: vec4<f32>,
    sky2: vec4<f32>,
    sky3: vec4<f32>,
    sky4: vec4<f32>,
    fog: vec4<f32>,
    // x = warp strength (0 = none), y = the sun's azimuth in radians.
    warp: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> sky: SkyColors;

// The warp's glow factor by azimuth phase, the client's six-key table read piecewise-linearly:
// +1 on the sun's bearing (phase 0.125) leaves the ring as it is, -0.7 opposite it is darkest.
fn azimuth_glow(phase: f32) -> f32 {
    let p = fract(phase);
    if (p < 0.125) { return mix(0.0, 1.0, (p + 0.125) / 0.25); }
    else if (p < 0.375) { return mix(1.0, 0.0, (p - 0.125) / 0.25); }
    else if (p < 0.5) { return mix(0.0, -0.5, (p - 0.375) / 0.125); }
    else if (p < 0.625) { return mix(-0.5, -0.7, (p - 0.5) / 0.125); }
    else if (p < 0.75) { return mix(-0.7, -0.5, (p - 0.625) / 0.125); }
    else if (p < 0.875) { return mix(-0.5, 0.0, (p - 0.75) / 0.125); }
    else { return mix(0.0, 1.0, (p - 0.875) / 0.25); }
}

fn warp_one(base: vec3<f32>, g: f32, s: f32) -> vec3<f32> {
    let s2 = s * s;
    if (g >= 0.0) {
        return mix(base, sky.sky1.rgb, (1.0 - g) * s2);
    }
    return mix(mix(base, sky.sky1.rgb, s), sky.sky0.rgb, 0.7 * (-g) * s2);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = normalize(in.world_position.xyz - view.world_position.xyz);
    let elev = degrees(asin(clamp(dir.y, -1.0, 1.0)));

    var s1 = sky.sky1.rgb;
    var s2c = sky.sky2.rgb;
    var s3 = sky.sky3.rgb;
    var s4 = sky.sky4.rgb;
    let warp_s = sky.warp.x;
    if (warp_s > 0.0) {
        let az = fract((atan2(dir.z, dir.x) - sky.warp.y) / 6.2831853 + 0.125);
        let seg = az * 24.0;
        let s0 = floor(seg);
        let f = seg - s0;
        let g0 = azimuth_glow(s0 / 24.0);
        let g1 = azimuth_glow((s0 + 1.0) / 24.0);
        s1 = mix(warp_one(sky.sky1.rgb, g0, warp_s), warp_one(sky.sky1.rgb, g1, warp_s), f);
        s2c = mix(warp_one(sky.sky2.rgb, g0, warp_s), warp_one(sky.sky2.rgb, g1, warp_s), f);
        s3 = mix(warp_one(sky.sky3.rgb, g0, warp_s), warp_one(sky.sky3.rgb, g1, warp_s), f);
        s4 = mix(warp_one(sky.sky4.rgb, g0, warp_s), warp_one(sky.sky4.rgb, g1, warp_s), f);
    }

    var col: vec3<f32>;
    if (elev <= 0.0) {
        col = sky.fog.rgb;
    } else if (elev < 1.8) {
        col = mix(sky.fog.rgb, s4, elev / 1.8);
    } else if (elev < 3.7) {
        col = mix(s4, s3, (elev - 1.8) / (3.7 - 1.8));
    } else if (elev < 9.8) {
        col = mix(s3, s2c, (elev - 3.7) / (9.8 - 3.7));
    } else if (elev < 16.8) {
        col = mix(s2c, s1, (elev - 9.8) / (16.8 - 9.8));
    } else {
        col = mix(s1, sky.sky0.rgb, (elev - 16.8) / (90.0 - 16.8));
    }

    let rgb = col;
    return vec4<f32>(rgb, 1.0);
}
