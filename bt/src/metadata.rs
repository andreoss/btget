use crate::extensions::{
    build_ext_handshake, build_metadata_request, parse_ext_handshake, parse_metadata_message,
    MetadataMessage, EXT_HANDSHAKE_ID, METADATA_PIECE_SIZE,
};
use crate::metainfo::InfoHash;
use crate::peer::{exchange_handshake, read_message, write_message, Handshake, Message};
use crate::verify::piece_digest;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

pub const LOCAL_UT_METADATA_ID: u8 = 1;
const FETCH_DEADLINE: Duration = Duration::from_secs(60);
const MAX_METADATA_SIZE: u64 = 8 * 1024 * 1024;

pub fn fetch_from_stream<S: Read + Write>(
    stream: &mut S,
    info_hash: InfoHash,
) -> Result<Vec<u8>, String> {
    write_message(
        stream,
        &Message::Extended {
            ext: EXT_HANDSHAKE_ID,
            payload: build_ext_handshake(None),
        },
    )
    .map_err(|e| format!("{:?}", e))?;
    let deadline = Instant::now() + FETCH_DEADLINE;
    let (their_id, size) = loop {
        if Instant::now() > deadline {
            return Err("no extension handshake".to_string());
        }
        match read_message(stream).map_err(|e| format!("{:?}", e))? {
            Message::Extended { ext, payload } if ext == EXT_HANDSHAKE_ID => {
                let handshake =
                    parse_ext_handshake(&payload).map_err(|e| format!("{:?}", e))?;
                match (handshake.ut_metadata, handshake.metadata_size) {
                    (Some(id), Some(size)) if size <= MAX_METADATA_SIZE => break (id, size),
                    _ => return Err("peer lacks metadata support".to_string()),
                }
            }
            _ => {}
        }
    };
    let piece_count = size.div_ceil(METADATA_PIECE_SIZE);
    let mut metadata = vec![0u8; size as usize];
    for piece in 0..piece_count {
        write_message(
            stream,
            &Message::Extended {
                ext: their_id,
                payload: build_metadata_request(piece),
            },
        )
        .map_err(|e| format!("{:?}", e))?;
        loop {
            if Instant::now() > deadline {
                return Err("metadata fetch timed out".to_string());
            }
            match read_message(stream).map_err(|e| format!("{:?}", e))? {
                Message::Extended { ext, payload } if ext != EXT_HANDSHAKE_ID => {
                    match parse_metadata_message(&payload) {
                        Ok(MetadataMessage::Data {
                            piece: got, data, ..
                        }) if got == piece => {
                            let start = (piece * METADATA_PIECE_SIZE) as usize;
                            if start + data.len() > metadata.len() {
                                return Err("metadata piece overflow".to_string());
                            }
                            metadata[start..start + data.len()].copy_from_slice(&data);
                            break;
                        }
                        Ok(MetadataMessage::Reject { .. }) => {
                            return Err("metadata request rejected".to_string())
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    if piece_digest(&metadata) != info_hash.0 {
        return Err("metadata hash mismatch".to_string());
    }
    Ok(metadata)
}

pub fn fetch_from_peers(
    info_hash: InfoHash,
    peer_id: [u8; 20],
    peers: &[SocketAddr],
    per_peer_timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mine = Handshake {
        info_hash,
        peer_id,
        extensions: true,
    };
    let mut last = "no peers".to_string();
    for addr in peers {
        let attempt = (|| -> Result<Vec<u8>, String> {
            let mut stream = TcpStream::connect_timeout(addr, Duration::from_secs(5))
                .map_err(|e| e.to_string())?;
            stream
                .set_read_timeout(Some(per_peer_timeout))
                .map_err(|e| e.to_string())?;
            stream
                .set_write_timeout(Some(per_peer_timeout))
                .map_err(|e| e.to_string())?;
            let theirs =
                exchange_handshake(&mut stream, &mine).map_err(|e| format!("{:?}", e))?;
            if !theirs.extensions {
                return Err("peer lacks extension protocol".to_string());
            }
            fetch_from_stream(&mut stream, info_hash)
        })();
        match attempt {
            Ok(metadata) => return Ok(metadata),
            Err(e) => last = format!("{}: {}", addr, e),
        }
    }
    Err(last)
}
