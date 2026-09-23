use std::collections::HashMap;

use dbc::{FieldType, Schema, SchemaField};
use mpq::Chain;

use crate::Error;
use crate::bands::{Band, load_float_bands, load_int_bands};
use crate::table::{f32_at, model_path, read, str_at, u32_at};

const LIGHT: &str = "DBFilesClient\\Light.dbc";
const LIGHT_PARAMS: &str = "DBFilesClient\\LightParams.dbc";
const LIGHT_SKYBOX: &str = "DBFilesClient\\LightSkybox.dbc";

pub(crate) const DBC_UNITS_PER_YARD: f32 = 36.0;

const PARAMS_FIELD_HIGHLIGHT: usize = 1;
const PARAMS_FIELD_SKYBOX: usize = 2;
/// The client reads glow from field 4. Field 3 is zero in every record.
const PARAMS_FIELD_GLOW: usize = 4;
const PARAMS_FIELD_WATER_SHALLOW_ALPHA: usize = 5;
const PARAMS_FIELD_WATER_DEEP_ALPHA: usize = 6;
const PARAMS_FIELD_OCEAN_SHALLOW_ALPHA: usize = 7;
const PARAMS_FIELD_OCEAN_DEEP_ALPHA: usize = 8;

pub(crate) struct Light {
    pub(crate) id: u32,
    pub(crate) map: u32,
    pub(crate) pos: [f32; 3],
    pub(crate) falloff_start: f32,
    pub(crate) falloff_end: f32,
    pub(crate) global: bool,
    pub(crate) params: [u32; 5],
}

/// The lighting tables; sample them with [`LightCatalog::sample`].
pub struct LightCatalog {
    pub(crate) lights: Vec<Light>,
    pub(crate) int_bands: HashMap<u32, Band<u32>>,
    pub(crate) float_bands: HashMap<u32, Band<f32>>,
    pub(crate) glow: HashMap<u32, f32>,
    pub(crate) highlight_sky: HashMap<u32, f32>,
    pub(crate) water_alpha: HashMap<u32, [f32; 4]>,
    pub(crate) skybox_models: HashMap<u32, String>,
}

pub(crate) fn dbc_to_world(x: f32, y: f32, z: f32) -> [f32; 3] {
    const MAP_ORIGIN: f32 = 17066.666;
    [
        MAP_ORIGIN - z / DBC_UNITS_PER_YARD,
        MAP_ORIGIN - x / DBC_UNITS_PER_YARD,
        y / DBC_UNITS_PER_YARD,
    ]
}

fn light_schema() -> Schema {
    let mut s = Schema::new("Light");
    for (n, t) in [
        ("ID", FieldType::UInt32),
        ("continent", FieldType::UInt32),
        ("x", FieldType::Float32),
        ("y", FieldType::Float32),
        ("z", FieldType::Float32),
        ("falloffStart", FieldType::Float32),
        ("falloffEnd", FieldType::Float32),
    ] {
        s.add_field(SchemaField::new(n, t));
    }
    for i in 0..5 {
        s.add_field(SchemaField::new(format!("param{i}"), FieldType::UInt32));
    }
    s
}

fn light_params_schema() -> Schema {
    let mut s = Schema::new("LightParams");
    for (n, t) in [
        ("ID", FieldType::UInt32),
        ("highlightSky", FieldType::UInt32),
        ("lightSkyboxID", FieldType::UInt32),
        ("cloudTypeID", FieldType::UInt32),
        ("glow", FieldType::Float32),
        ("waterShallowAlpha", FieldType::Float32),
        ("waterDeepAlpha", FieldType::Float32),
        ("oceanShallowAlpha", FieldType::Float32),
        ("oceanDeepAlpha", FieldType::Float32),
    ] {
        s.add_field(SchemaField::new(n, t));
    }
    s
}

fn load_lights(chain: &Chain) -> Result<Vec<Light>, Error> {
    let rs = read(chain, LIGHT, light_schema())?;
    let mut lights = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(map), Some(x), Some(y), Some(z), Some(start), Some(end)) = (
            u32_at(r, 0),
            u32_at(r, 1),
            f32_at(r, 2),
            f32_at(r, 3),
            f32_at(r, 4),
            f32_at(r, 5),
            f32_at(r, 6),
        ) else {
            continue;
        };
        let mut params = [0u32; 5];
        for (i, p) in params.iter_mut().enumerate() {
            *p = u32_at(r, 7 + i).unwrap_or(0);
        }
        let global = x == 0.0 && y == 0.0 && z == 0.0;
        lights.push(Light {
            id,
            map,
            pos: if global {
                [0.0; 3]
            } else {
                dbc_to_world(x, y, z)
            },
            falloff_start: start / DBC_UNITS_PER_YARD,
            falloff_end: end / DBC_UNITS_PER_YARD,
            global,
            params,
        });
    }
    Ok(lights)
}

impl LightCatalog {
    /// Reads `Light`, `LightIntBand`, `LightFloatBand`, `LightParams` and `LightSkybox` off the
    /// chain.
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let lights = load_lights(chain)?;
        let int_bands = load_int_bands(chain)?;
        let float_bands = load_float_bands(chain)?;
        let mut catalog = LightCatalog {
            lights,
            int_bands,
            float_bands,
            glow: HashMap::new(),
            highlight_sky: HashMap::new(),
            water_alpha: HashMap::new(),
            skybox_models: HashMap::new(),
        };
        let skybox_ids = catalog.load_params(chain)?;
        let skyboxes = load_skyboxes(chain)?;
        for (params, skybox) in skybox_ids {
            if let Some(model) = skyboxes.get(&skybox) {
                catalog.skybox_models.insert(params, model.clone());
            }
        }
        Ok(catalog)
    }

    fn load_params(&mut self, chain: &Chain) -> Result<HashMap<u32, u32>, Error> {
        let rs = read(chain, LIGHT_PARAMS, light_params_schema())?;
        let mut skybox_ids = HashMap::new();
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else { continue };
            if let Some(g) = f32_at(r, PARAMS_FIELD_GLOW) {
                self.glow.insert(id, g);
            }
            if let Some(h) = u32_at(r, PARAMS_FIELD_HIGHLIGHT) {
                self.highlight_sky
                    .insert(id, if h != 0 { 1.0 } else { 0.0 });
            }
            if let (Some(ws), Some(wd), Some(os), Some(od)) = (
                f32_at(r, PARAMS_FIELD_WATER_SHALLOW_ALPHA),
                f32_at(r, PARAMS_FIELD_WATER_DEEP_ALPHA),
                f32_at(r, PARAMS_FIELD_OCEAN_SHALLOW_ALPHA),
                f32_at(r, PARAMS_FIELD_OCEAN_DEEP_ALPHA),
            ) {
                self.water_alpha.insert(id, [ws, wd, os, od]);
            }
            match u32_at(r, PARAMS_FIELD_SKYBOX) {
                Some(0) | None => {}
                Some(sky) => {
                    skybox_ids.insert(id, sky);
                }
            }
        }
        Ok(skybox_ids)
    }
}

fn load_skyboxes(chain: &Chain) -> Result<HashMap<u32, String>, Error> {
    let mut schema = Schema::new("LightSkybox");
    schema.add_field(SchemaField::new("ID", FieldType::UInt32));
    schema.add_field(SchemaField::new("Name", FieldType::String));
    let rs = read(chain, LIGHT_SKYBOX, schema)?;
    let mut skyboxes = HashMap::new();
    for r in rs.records() {
        if let (Some(id), Some(path)) = (u32_at(r, 0), str_at(&rs, r, 1)) {
            skyboxes.insert(id, model_path(&path));
        }
    }
    Ok(skyboxes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbc_positions_convert_to_world_coordinates() {
        let centre = dbc_to_world(0.0, 0.0, 0.0).map(f32::to_bits);
        assert_eq!(centre, [17066.666f32, 17066.666, 0.0].map(f32::to_bits));
        let world = [-8480.4f32, 548.3, 80.9];
        let dbc = [
            (17066.666 - world[1]) * DBC_UNITS_PER_YARD,
            world[2] * DBC_UNITS_PER_YARD,
            (17066.666 - world[0]) * DBC_UNITS_PER_YARD,
        ];
        let back = dbc_to_world(dbc[0], dbc[1], dbc[2]);
        for i in 0..3 {
            assert!((back[i] - world[i]).abs() < 0.1, "axis {i}: {back:?}");
        }
    }

    #[test]
    fn model_paths_name_the_m2() {
        assert_eq!(
            model_path("Environments\\Stars\\DeathClouds.mdx"),
            "environments\\stars\\deathclouds.m2"
        );
        assert_eq!(model_path("A.MDL"), "a.m2");
        assert_eq!(model_path("A.m2"), "a.m2");
        assert_eq!(model_path("A"), "a");
    }
}
