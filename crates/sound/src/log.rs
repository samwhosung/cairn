use std::io::Write;
use std::path::Path;

use bevy::log::warn;
use bevy::math::Vec3;

use crate::kit::Played;
use crate::mixer::amp_to_db;

pub(crate) struct PlayLog {
    file: std::io::BufWriter<std::fs::File>,
}

impl PlayLog {
    pub(crate) fn create(path: &Path) -> Option<Self> {
        match std::fs::File::create(path) {
            Ok(file) => Some(Self {
                file: std::io::BufWriter::new(file),
            }),
            Err(e) => {
                warn!("sound: cannot log plays to {}: {e}", path.display());
                None
            }
        }
    }

    pub(crate) fn play(
        &mut self,
        t: f64,
        played: &Played,
        category: &str,
        spatial: &str,
        pos: Option<Vec3>,
    ) {
        let pos = pos.map_or_else(
            || "null".to_owned(),
            |p| format!("[{},{},{}]", p.x, p.y, p.z),
        );
        let line = format!(
            r#"{{"t":{t:.6},"ev":"kit","kit":{},"file":"{}","cat":"{category}","sp":"{spatial}","db":{},"rate":{},"loop":{},"pos":{pos}}}"#,
            played.kit,
            escape(&played.path),
            amp_to_db(played.amp).0,
            played.rate,
            played.looping,
        );
        self.write(&line);
    }

    pub(crate) fn stream(&mut self, t: f64, slot: &str, kit: u32, path: &str, amp: f32) {
        let line = format!(
            r#"{{"t":{t:.6},"ev":"{slot}","kit":{kit},"file":"{}","db":{}}}"#,
            escape(path),
            amp_to_db(amp).0,
        );
        self.write(&line);
    }

    fn write(&mut self, line: &str) {
        if writeln!(self.file, "{line}")
            .and_then(|()| self.file.flush())
            .is_err()
        {
            warn!("sound: the play log stopped writing");
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
