use crate::bencode::{self, Value};
use crate::metainfo::InfoHash;
use crate::pieces::Bitfield;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn state_path(output_dir: &Path, name: &str) -> PathBuf {
    output_dir.join(format!(".{}.resume", name))
}

pub fn save(path: &Path, info_hash: InfoHash, have: &Bitfield) -> std::io::Result<()> {
    let mut top = BTreeMap::new();
    top.insert(b"hash".to_vec(), Value::Bytes(info_hash.0.to_vec()));
    top.insert(b"have".to_vec(), Value::Bytes(have.as_bytes().to_vec()));
    top.insert(b"pieces".to_vec(), Value::Int(have.pieces() as i64));
    std::fs::write(path, bencode::encode(&Value::Dict(top)))
}

pub fn load(path: &Path, info_hash: InfoHash, pieces: u32) -> Option<Bitfield> {
    let raw = std::fs::read(path).ok()?;
    let top = match bencode::decode(&raw) {
        Ok(Value::Dict(map)) => map,
        _ => return None,
    };
    match top.get(b"hash".as_slice()) {
        Some(Value::Bytes(h)) if h.as_slice() == info_hash.0 => {}
        _ => return None,
    }
    match top.get(b"pieces".as_slice()) {
        Some(Value::Int(n)) if *n == pieces as i64 => {}
        _ => return None,
    }
    match top.get(b"have".as_slice()) {
        Some(Value::Bytes(bits)) => Bitfield::from_bytes(bits, pieces).ok(),
        _ => None,
    }
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}
