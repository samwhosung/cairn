//! Prints every file the chain at `$WOW_DATA` lists, sorted: path, size, FNV-1a hash of its bytes.

use std::error::Error;
use std::io::{BufWriter, Write};

fn main() -> Result<(), Box<dyn Error>> {
    let data = std::env::var_os("WOW_DATA").ok_or("set WOW_DATA to a 1.12.1 Data directory")?;
    let chain = mpq::Chain::open(data)?;
    let mut files = chain.list();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = BufWriter::new(std::io::stdout().lock());
    for file in &files {
        let hash = match chain.read(&file.name) {
            Ok(bytes) => format!("{:016x}", fnv1a(&bytes)),
            Err(e) => {
                eprintln!("{e}");
                "unreadable".to_owned()
            }
        };
        writeln!(out, "{}\t{}\t{hash}", file.name, file.size)?;
    }
    out.flush()?;
    Ok(())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}
