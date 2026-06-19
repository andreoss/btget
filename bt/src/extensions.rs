use crate::bencode::{self, Value};
use std::collections::BTreeMap;

pub const EXT_HANDSHAKE_ID: u8 = 0;
pub const UT_METADATA: &[u8] = b"ut_metadata";
pub const METADATA_PIECE_SIZE: u64 = 16384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtHandshake {
    pub ut_metadata: Option<u8>,
    pub metadata_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotADict,
    BadPayload,
}

pub fn build_ext_handshake(metadata_size: Option<u64>) -> Vec<u8> {
    let mut m = BTreeMap::new();
    m.insert(UT_METADATA.to_vec(), Value::Int(1));
    let mut top = BTreeMap::new();
    top.insert(b"m".to_vec(), Value::Dict(m));
    if let Some(size) = metadata_size {
        top.insert(b"metadata_size".to_vec(), Value::Int(size as i64));
    }
    bencode::encode(&Value::Dict(top))
}

pub fn parse_ext_handshake(payload: &[u8]) -> Result<ExtHandshake, Error> {
    let top = decode_prefix_dict(payload)?;
    let ut_metadata = match top.get(b"m".as_slice()) {
        Some(Value::Dict(m)) => match m.get(UT_METADATA) {
            Some(Value::Int(n)) if *n > 0 && *n <= 255 => Some(*n as u8),
            _ => None,
        },
        _ => None,
    };
    let metadata_size = match top.get(b"metadata_size".as_slice()) {
        Some(Value::Int(n)) if *n > 0 => Some(*n as u64),
        _ => None,
    };
    Ok(ExtHandshake {
        ut_metadata,
        metadata_size,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataMessage {
    Request { piece: u64 },
    Data { piece: u64, total_size: Option<u64>, data: Vec<u8> },
    Reject { piece: u64 },
}

pub fn build_metadata_request(piece: u64) -> Vec<u8> {
    let mut top = BTreeMap::new();
    top.insert(b"msg_type".to_vec(), Value::Int(0));
    top.insert(b"piece".to_vec(), Value::Int(piece as i64));
    bencode::encode(&Value::Dict(top))
}

pub fn build_metadata_data(piece: u64, total_size: u64, data: &[u8]) -> Vec<u8> {
    let mut top = BTreeMap::new();
    top.insert(b"msg_type".to_vec(), Value::Int(1));
    top.insert(b"piece".to_vec(), Value::Int(piece as i64));
    top.insert(b"total_size".to_vec(), Value::Int(total_size as i64));
    let mut out = bencode::encode(&Value::Dict(top));
    out.extend_from_slice(data);
    out
}

pub fn build_metadata_reject(piece: u64) -> Vec<u8> {
    let mut top = BTreeMap::new();
    top.insert(b"msg_type".to_vec(), Value::Int(2));
    top.insert(b"piece".to_vec(), Value::Int(piece as i64));
    bencode::encode(&Value::Dict(top))
}

pub fn parse_metadata_message(payload: &[u8]) -> Result<MetadataMessage, Error> {
    let (top, consumed) = decode_prefix_dict_with_len(payload)?;
    let msg_type = match top.get(b"msg_type".as_slice()) {
        Some(Value::Int(n)) => *n,
        _ => return Err(Error::BadPayload),
    };
    let piece = match top.get(b"piece".as_slice()) {
        Some(Value::Int(n)) if *n >= 0 => *n as u64,
        _ => return Err(Error::BadPayload),
    };
    match msg_type {
        0 => Ok(MetadataMessage::Request { piece }),
        1 => {
            let total_size = match top.get(b"total_size".as_slice()) {
                Some(Value::Int(n)) if *n > 0 => Some(*n as u64),
                _ => None,
            };
            Ok(MetadataMessage::Data {
                piece,
                total_size,
                data: payload[consumed..].to_vec(),
            })
        }
        2 => Ok(MetadataMessage::Reject { piece }),
        _ => Err(Error::BadPayload),
    }
}

fn decode_prefix_dict(payload: &[u8]) -> Result<BTreeMap<Vec<u8>, Value>, Error> {
    decode_prefix_dict_with_len(payload).map(|(d, _)| d)
}

fn decode_prefix_dict_with_len(
    payload: &[u8],
) -> Result<(BTreeMap<Vec<u8>, Value>, usize), Error> {
    let consumed = find_dict_end(payload).ok_or(Error::BadPayload)?;
    match bencode::decode(&payload[..consumed]) {
        Ok(Value::Dict(map)) => Ok((map, consumed)),
        Ok(_) => Err(Error::NotADict),
        Err(_) => Err(Error::BadPayload),
    }
}

fn find_dict_end(payload: &[u8]) -> Option<usize> {
    if payload.first() != Some(&b'd') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < payload.len() {
        match payload[i] {
            b'd' | b'l' => {
                depth += 1;
                i += 1;
            }
            b'i' => {
                let end = payload[i..].iter().position(|b| *b == b'e')? + i;
                i = end + 1;
            }
            b'0'..=b'9' => {
                let colon = payload[i..].iter().position(|b| *b == b':')? + i;
                let len: usize = std::str::from_utf8(&payload[i..colon]).ok()?.parse().ok()?;
                let next = colon.checked_add(1)?.checked_add(len)?;
                if next > payload.len() {
                    return None;
                }
                i = next;
            }
            b'e' => {
                depth = depth.checked_sub(1)?;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => return None,
        }
    }
    None
}
