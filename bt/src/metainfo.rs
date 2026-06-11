use crate::bencode::{self, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InfoHash(pub [u8; 20]);

impl InfoHash {
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn from_hex(text: &str) -> Option<InfoHash> {
        if text.len() != 40 || !text.is_ascii() {
            return None;
        }
        let mut out = [0u8; 20];
        for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            out[i] = (hi * 16 + lo) as u8;
        }
        Some(InfoHash(out))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: Vec<String>,
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metainfo {
    pub trackers: Vec<Vec<String>>,
    pub info_hash: InfoHash,
    pub name: String,
    pub piece_length: u64,
    pub pieces: Vec<[u8; 20]>,
    pub files: Vec<FileEntry>,
    pub total_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Bencode(bencode::Error),
    NotADict,
    MissingKey(&'static str),
    WrongType(&'static str),
    BadPieces,
    BadPath,
    BadName,
    BadLength,
    PieceCountMismatch,
    BothLengthAndFiles,
    NoFiles,
}

impl From<bencode::Error> for Error {
    fn from(e: bencode::Error) -> Self {
        Error::Bencode(e)
    }
}

pub fn parse(input: &[u8]) -> Result<Metainfo, Error> {
    let top = match bencode::decode(input)? {
        Value::Dict(map) => map,
        _ => return Err(Error::NotADict),
    };
    let info_value = top.get(b"info".as_slice()).ok_or(Error::MissingKey("info"))?;
    let info_bytes = bencode::encode(info_value);
    let info = match info_value {
        Value::Dict(map) => map,
        _ => return Err(Error::WrongType("info")),
    };
    build(info, &info_bytes, parse_trackers(&top))
}

pub fn parse_info_dict(
    info_bytes: &[u8],
    trackers: Vec<Vec<String>>,
) -> Result<Metainfo, Error> {
    let info = match bencode::decode(info_bytes)? {
        Value::Dict(map) => map,
        _ => return Err(Error::NotADict),
    };
    build(&info, info_bytes, trackers)
}

fn build(
    info: &BTreeMap<Vec<u8>, Value>,
    info_bytes: &[u8],
    trackers: Vec<Vec<String>>,
) -> Result<Metainfo, Error> {
    let digest = Sha1::digest(info_bytes);
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&digest);
    let name = utf8(req_bytes(info, "name")?).ok_or(Error::BadName)?;
    check_component(&name)?;
    let piece_length = req_u64(info, "piece length")?;
    if piece_length == 0 {
        return Err(Error::BadLength);
    }
    let pieces_raw = req_bytes(info, "pieces")?;
    if pieces_raw.is_empty() || pieces_raw.len() % 20 != 0 {
        return Err(Error::BadPieces);
    }
    let pieces: Vec<[u8; 20]> = pieces_raw
        .chunks(20)
        .map(|c| {
            let mut p = [0u8; 20];
            p.copy_from_slice(c);
            p
        })
        .collect();
    let files = parse_files(info)?;
    let total_length: u64 = files.iter().map(|f| f.length).sum();
    if total_length == 0 {
        return Err(Error::BadLength);
    }
    let expected = total_length.div_ceil(piece_length);
    if expected != pieces.len() as u64 {
        return Err(Error::PieceCountMismatch);
    }
    Ok(Metainfo {
        trackers,
        info_hash: InfoHash(hash),
        name,
        piece_length,
        pieces,
        files,
        total_length,
    })
}

fn parse_files(info: &BTreeMap<Vec<u8>, Value>) -> Result<Vec<FileEntry>, Error> {
    let length = info.get(b"length".as_slice());
    let files = info.get(b"files".as_slice());
    match (length, files) {
        (Some(_), Some(_)) => Err(Error::BothLengthAndFiles),
        (Some(Value::Int(n)), None) if *n > 0 => Ok(vec![FileEntry {
            path: Vec::new(),
            length: *n as u64,
        }]),
        (Some(_), None) => Err(Error::WrongType("length")),
        (None, Some(Value::List(items))) => {
            if items.is_empty() {
                return Err(Error::NoFiles);
            }
            items.iter().map(parse_file_entry).collect()
        }
        (None, Some(_)) => Err(Error::WrongType("files")),
        (None, None) => Err(Error::MissingKey("length")),
    }
}

fn parse_file_entry(value: &Value) -> Result<FileEntry, Error> {
    let map = match value {
        Value::Dict(map) => map,
        _ => return Err(Error::WrongType("files entry")),
    };
    let length = req_u64(map, "length")?;
    let path_value = map.get(b"path".as_slice()).ok_or(Error::MissingKey("path"))?;
    let items = match path_value {
        Value::List(items) => items,
        _ => return Err(Error::WrongType("path")),
    };
    if items.is_empty() {
        return Err(Error::BadPath);
    }
    let mut path = Vec::new();
    for item in items {
        let component = match item {
            Value::Bytes(b) => utf8(b).ok_or(Error::BadPath)?,
            _ => return Err(Error::WrongType("path component")),
        };
        check_component(&component)?;
        path.push(component);
    }
    Ok(FileEntry { path, length })
}

fn check_component(component: &str) -> Result<(), Error> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.contains('/')
        || component.contains('\\')
        || component.contains('\0')
    {
        return Err(Error::BadPath);
    }
    Ok(())
}

fn parse_trackers(top: &BTreeMap<Vec<u8>, Value>) -> Vec<Vec<String>> {
    if let Some(Value::List(tiers)) = top.get(b"announce-list".as_slice()) {
        let parsed: Vec<Vec<String>> = tiers
            .iter()
            .filter_map(|tier| match tier {
                Value::List(urls) => {
                    let tier: Vec<String> = urls
                        .iter()
                        .filter_map(|u| match u {
                            Value::Bytes(b) => utf8(b),
                            _ => None,
                        })
                        .collect();
                    if tier.is_empty() {
                        None
                    } else {
                        Some(tier)
                    }
                }
                _ => None,
            })
            .collect();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    if let Some(Value::Bytes(b)) = top.get(b"announce".as_slice()) {
        if let Some(url) = utf8(b) {
            return vec![vec![url]];
        }
    }
    Vec::new()
}

fn utf8(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes).ok().map(|s| s.to_string())
}

fn req_bytes<'a>(map: &'a BTreeMap<Vec<u8>, Value>, key: &'static str) -> Result<&'a [u8], Error> {
    match map.get(key.as_bytes()) {
        Some(Value::Bytes(b)) => Ok(b),
        Some(_) => Err(Error::WrongType(key)),
        None => Err(Error::MissingKey(key)),
    }
}

fn req_u64(map: &BTreeMap<Vec<u8>, Value>, key: &'static str) -> Result<u64, Error> {
    match map.get(key.as_bytes()) {
        Some(Value::Int(n)) if *n >= 0 => Ok(*n as u64),
        Some(_) => Err(Error::WrongType(key)),
        None => Err(Error::MissingKey(key)),
    }
}
