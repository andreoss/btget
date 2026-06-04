use crate::metainfo::InfoHash;
use std::io::{Read, Write};

pub const PSTR: &[u8; 19] = b"BitTorrent protocol";
const EXTENSION_BIT: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handshake {
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
    pub extensions: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Io(String),
    BadHandshake,
    WrongInfoHash,
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub fn encode_handshake(handshake: &Handshake) -> [u8; 68] {
    let mut out = [0u8; 68];
    out[0] = 19;
    out[1..20].copy_from_slice(PSTR);
    if handshake.extensions {
        out[25] |= EXTENSION_BIT;
    }
    out[28..48].copy_from_slice(&handshake.info_hash.0);
    out[48..68].copy_from_slice(&handshake.peer_id);
    out
}

pub fn decode_handshake(raw: &[u8; 68]) -> Result<Handshake, Error> {
    if raw[0] != 19 || &raw[1..20] != PSTR {
        return Err(Error::BadHandshake);
    }
    let mut info_hash = [0u8; 20];
    info_hash.copy_from_slice(&raw[28..48]);
    let mut peer_id = [0u8; 20];
    peer_id.copy_from_slice(&raw[48..68]);
    Ok(Handshake {
        info_hash: InfoHash(info_hash),
        peer_id,
        extensions: raw[25] & EXTENSION_BIT != 0,
    })
}

pub fn exchange_handshake<S: Read + Write>(
    stream: &mut S,
    ours: &Handshake,
) -> Result<Handshake, Error> {
    stream.write_all(&encode_handshake(ours))?;
    let mut raw = [0u8; 68];
    stream.read_exact(&mut raw)?;
    let theirs = decode_handshake(&raw)?;
    if theirs.info_hash != ours.info_hash {
        return Err(Error::WrongInfoHash);
    }
    Ok(theirs)
}
