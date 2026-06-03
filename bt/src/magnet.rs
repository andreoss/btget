use crate::metainfo::InfoHash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Magnet {
    pub info_hash: InfoHash,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotMagnet,
    MissingTopic,
    UnsupportedTopic,
    BadHash,
    BadEncoding,
}

pub fn parse(input: &str) -> Result<Magnet, Error> {
    let query = input.strip_prefix("magnet:?").ok_or(Error::NotMagnet)?;
    let mut info_hash = None;
    let mut display_name = None;
    let mut trackers = Vec::new();
    for pair in query.split('&') {
        let (key, raw) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match key {
            "xt" => {
                let value = percent_decode(raw)?;
                if let Some(rest) = value.strip_prefix("urn:btih:") {
                    info_hash = Some(decode_btih(rest)?);
                } else if value.starts_with("urn:") {
                    return Err(Error::UnsupportedTopic);
                }
            }
            "dn" => display_name = Some(percent_decode(raw)?),
            "tr" => trackers.push(percent_decode(raw)?),
            _ => {}
        }
    }
    Ok(Magnet {
        info_hash: info_hash.ok_or(Error::MissingTopic)?,
        display_name,
        trackers,
    })
}

fn decode_btih(text: &str) -> Result<InfoHash, Error> {
    match text.len() {
        40 => InfoHash::from_hex(text).ok_or(Error::BadHash),
        32 => base32_decode(text).map(InfoHash).ok_or(Error::BadHash),
        _ => Err(Error::BadHash),
    }
}

fn base32_decode(text: &str) -> Option<[u8; 20]> {
    let mut bits = 0u64;
    let mut bit_count = 0u32;
    let mut out = Vec::with_capacity(20);
    for c in text.chars() {
        let value = match c {
            'A'..='Z' => c as u64 - 'A' as u64,
            'a'..='z' => c as u64 - 'a' as u64,
            '2'..='7' => c as u64 - '2' as u64 + 26,
            _ => return None,
        };
        bits = (bits << 5) | value;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
        }
    }
    if out.len() != 20 {
        return None;
    }
    let mut hash = [0u8; 20];
    hash.copy_from_slice(&out);
    Some(hash)
}

fn percent_decode(raw: &str) -> Result<String, Error> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hi = hex_digit(*bytes.get(i + 1).ok_or(Error::BadEncoding)?)?;
                let lo = hex_digit(*bytes.get(i + 2).ok_or(Error::BadEncoding)?)?;
                out.push(hi * 16 + lo);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| Error::BadEncoding)
}

fn hex_digit(b: u8) -> Result<u8, Error> {
    (b as char).to_digit(16).map(|d| d as u8).ok_or(Error::BadEncoding)
}
