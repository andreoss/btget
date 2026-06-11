use crate::metainfo::InfoHash;
use std::io::{Read, Write};

pub const PSTR: &[u8; 19] = b"BitTorrent protocol";
pub const MAX_FRAME: u32 = 1 << 20;
const EXTENSION_BIT: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handshake {
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
    pub extensions: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request { index: u32, begin: u32, length: u32 },
    Piece { index: u32, begin: u32, data: Vec<u8> },
    Cancel { index: u32, begin: u32, length: u32 },
    Port(u16),
    Extended { ext: u8, payload: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Io(String),
    BadHandshake,
    WrongInfoHash,
    UnknownId(u8),
    BadFrame,
    FrameTooLong(u32),
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

pub fn encode_message(message: &Message) -> Vec<u8> {
    let mut body = Vec::new();
    match message {
        Message::KeepAlive => {}
        Message::Choke => body.push(0),
        Message::Unchoke => body.push(1),
        Message::Interested => body.push(2),
        Message::NotInterested => body.push(3),
        Message::Have(index) => {
            body.push(4);
            body.extend_from_slice(&index.to_be_bytes());
        }
        Message::Bitfield(bits) => {
            body.push(5);
            body.extend_from_slice(bits);
        }
        Message::Request { index, begin, length } => {
            body.push(6);
            body.extend_from_slice(&index.to_be_bytes());
            body.extend_from_slice(&begin.to_be_bytes());
            body.extend_from_slice(&length.to_be_bytes());
        }
        Message::Piece { index, begin, data } => {
            body.push(7);
            body.extend_from_slice(&index.to_be_bytes());
            body.extend_from_slice(&begin.to_be_bytes());
            body.extend_from_slice(data);
        }
        Message::Cancel { index, begin, length } => {
            body.push(8);
            body.extend_from_slice(&index.to_be_bytes());
            body.extend_from_slice(&begin.to_be_bytes());
            body.extend_from_slice(&length.to_be_bytes());
        }
        Message::Port(port) => {
            body.push(9);
            body.extend_from_slice(&port.to_be_bytes());
        }
        Message::Extended { ext, payload } => {
            body.push(20);
            body.push(*ext);
            body.extend_from_slice(payload);
        }
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn parse_frame(body: &[u8]) -> Result<Message, Error> {
    if body.is_empty() {
        return Ok(Message::KeepAlive);
    }
    let payload = &body[1..];
    match body[0] {
        0 => expect_empty(payload, Message::Choke),
        1 => expect_empty(payload, Message::Unchoke),
        2 => expect_empty(payload, Message::Interested),
        3 => expect_empty(payload, Message::NotInterested),
        4 => Ok(Message::Have(read_u32(payload, 0, payload.len() == 4)?)),
        5 => Ok(Message::Bitfield(payload.to_vec())),
        6 => {
            let (index, begin, length) = read_triple(payload)?;
            Ok(Message::Request { index, begin, length })
        }
        7 => {
            if payload.len() < 8 {
                return Err(Error::BadFrame);
            }
            Ok(Message::Piece {
                index: read_u32(payload, 0, true)?,
                begin: read_u32(payload, 4, true)?,
                data: payload[8..].to_vec(),
            })
        }
        8 => {
            let (index, begin, length) = read_triple(payload)?;
            Ok(Message::Cancel { index, begin, length })
        }
        9 => {
            if payload.len() != 2 {
                return Err(Error::BadFrame);
            }
            Ok(Message::Port(u16::from_be_bytes([payload[0], payload[1]])))
        }
        20 => {
            if payload.is_empty() {
                return Err(Error::BadFrame);
            }
            Ok(Message::Extended {
                ext: payload[0],
                payload: payload[1..].to_vec(),
            })
        }
        other => Err(Error::UnknownId(other)),
    }
}

fn expect_empty(payload: &[u8], message: Message) -> Result<Message, Error> {
    if payload.is_empty() {
        Ok(message)
    } else {
        Err(Error::BadFrame)
    }
}

fn read_u32(payload: &[u8], offset: usize, size_ok: bool) -> Result<u32, Error> {
    if !size_ok || payload.len() < offset + 4 {
        return Err(Error::BadFrame);
    }
    Ok(u32::from_be_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
    ]))
}

fn read_triple(payload: &[u8]) -> Result<(u32, u32, u32), Error> {
    if payload.len() != 12 {
        return Err(Error::BadFrame);
    }
    Ok((
        read_u32(payload, 0, true)?,
        read_u32(payload, 4, true)?,
        read_u32(payload, 8, true)?,
    ))
}

pub fn read_message<R: Read>(reader: &mut R) -> Result<Message, Error> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes)?;
    let len = u32::from_be_bytes(len_bytes);
    if len > MAX_FRAME {
        return Err(Error::FrameTooLong(len));
    }
    let mut body = vec![0u8; len as usize];
    reader.read_exact(&mut body)?;
    parse_frame(&body)
}

pub fn write_message<W: Write>(writer: &mut W, message: &Message) -> Result<(), Error> {
    writer.write_all(&encode_message(message))?;
    Ok(())
}
