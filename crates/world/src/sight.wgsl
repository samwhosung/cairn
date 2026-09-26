#define_import_path world::sight

// The GPU may write a colour to an sRGB target a byte off; with each byte 4n + 2, one a byte off
// still reads as n.
fn sight_colour(index: u32) -> vec3<f32> {
    let code = vec3<u32>(index & 63u, (index >> 6u) & 63u, index >> 12u);
    return srgb_to_linear((vec3<f32>(code) * 4.0 + 2.0) / 255.0);
}

fn srgb_to_linear(s: vec3<f32>) -> vec3<f32> {
    return select(pow((s + 0.055) / 1.055, vec3<f32>(2.4)), s / 12.92, s <= vec3<f32>(0.04045));
}
