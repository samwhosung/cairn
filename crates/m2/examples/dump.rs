//! Prints a model's textures, materials, skin 0 batches, attachments and event markers:
//! `cargo run -p m2 --example dump -- 'Creature\Kobold\Kobold.m2'`, with `WOW_DATA` set.

use std::error::Error;
use std::io::Cursor;

fn main() -> Result<(), Box<dyn Error>> {
    let data = std::env::var_os("WOW_DATA").ok_or("set WOW_DATA to a 1.12.1 Data directory")?;
    let name = std::env::args()
        .nth(1)
        .ok_or("usage: dump <model path in the chain>")?;
    let bytes = mpq::Chain::open(data)?.read(&name)?;
    let format = m2::parse_m2(&mut Cursor::new(bytes.as_slice()))?;
    let model = format.model();

    for (i, t) in model.textures.iter().enumerate() {
        let file = t.filename.string.to_string_lossy();
        println!("texture {i}: {:?} {file:?}", t.texture_type);
    }
    for (i, m) in model.materials.iter().enumerate() {
        let (flags, blend) = (m.flags.bits(), m.blend_mode.bits());
        println!("material {i}: flags {flags:#06x} blend {blend}");
    }
    println!("texture lookup: {:?}", model.raw_data.texture_lookup_table);
    println!("transparency lookup: {:?}", model.transparency_lookup);

    let skin = model.parse_embedded_skin(&bytes, 0)?;
    for (i, s) in skin.submeshes().iter().enumerate() {
        let (id, start, count) = (s.id, s.triangle_start, s.triangle_count);
        println!("section {i}: geoset {id} triangles {start}+{count}");
    }
    for (i, b) in skin.batches().iter().enumerate() {
        println!(
            "batch {i}: section {} material {} textures {}x{} color {} weight {} transform {}",
            b.skin_section_index,
            b.material_index,
            b.texture_combo_index,
            b.texture_count,
            b.color_index,
            b.weight_combo_index,
            b.texture_transform_combo_index
        );
    }

    let parent = |bone: u16| model.bones.get(bone as usize).map_or(-1, |b| b.parent);
    for a in &model.attachments {
        let [x, y, z] = a.position;
        let (id, bone) = (a.id, a.bone);
        println!(
            "attachment {id:2}: bone {bone:3} (parent {:3}) at ({x:+.3}, {y:+.3}, {z:+.3})",
            parent(bone)
        );
    }
    for e in &model.event_markers {
        let [x, y, z] = e.position;
        let ident = String::from_utf8_lossy(&e.ident);
        let bone = e.bone;
        println!(
            "event {ident}: bone {bone:3} (parent {:3}) at ({x:+.3}, {y:+.3}, {z:+.3})",
            parent(bone)
        );
    }
    Ok(())
}
