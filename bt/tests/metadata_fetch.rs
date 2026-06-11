mod fixtures;

use bt::bencode::{decode, encode, Value};
use bt::extensions::{
    build_metadata_data, parse_metadata_message, MetadataMessage, EXT_HANDSHAKE_ID,
    METADATA_PIECE_SIZE,
};
use bt::metadata::fetch_from_peers;
use bt::metainfo::{parse, parse_info_dict};
use bt::peer::{
    decode_handshake, encode_handshake, read_message, write_message, Handshake, Message,
};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};

fn info_bytes(torrent: &[u8]) -> Vec<u8> {
    match decode(torrent).unwrap() {
        Value::Dict(top) => encode(top.get(b"info".as_slice()).unwrap()),
        _ => unreachable!(),
    }
}

fn start_metadata_peer(metadata: Vec<u8>, corrupt: bool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let mut stream = match connection {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut raw = [0u8; 68];
            if stream.read_exact(&mut raw).is_err() {
                continue;
            }
            let theirs = match decode_handshake(&raw) {
                Ok(h) => h,
                Err(_) => continue,
            };
            let mine = Handshake {
                info_hash: theirs.info_hash,
                peer_id: *b"-SEED01-000000000000",
                extensions: true,
            };
            if stream.write_all(&encode_handshake(&mine)).is_err() {
                continue;
            }
            let metadata = metadata.clone();
            loop {
                let message = match read_message(&mut stream) {
                    Ok(m) => m,
                    Err(_) => break,
                };
                match message {
                    Message::Extended { ext, payload } => {
                        if ext == EXT_HANDSHAKE_ID {
                            let reply = Message::Extended {
                                ext: EXT_HANDSHAKE_ID,
                                payload: {
                                    let their_hs =
                                        bt::extensions::parse_ext_handshake(&payload).unwrap();
                                    assert_eq!(their_hs.ut_metadata, Some(1));
                                    let mut m = std::collections::BTreeMap::new();
                                    m.insert(b"ut_metadata".to_vec(), Value::Int(3));
                                    let mut top = std::collections::BTreeMap::new();
                                    top.insert(b"m".to_vec(), Value::Dict(m));
                                    top.insert(
                                        b"metadata_size".to_vec(),
                                        Value::Int(metadata.len() as i64),
                                    );
                                    encode(&Value::Dict(top))
                                },
                            };
                            if write_message(&mut stream, &reply).is_err() {
                                break;
                            }
                        } else if ext == 3 {
                            match parse_metadata_message(&payload) {
                                Ok(MetadataMessage::Request { piece }) => {
                                    let start = (piece * METADATA_PIECE_SIZE) as usize;
                                    let end = std::cmp::min(
                                        start + METADATA_PIECE_SIZE as usize,
                                        metadata.len(),
                                    );
                                    let mut chunk = metadata[start..end].to_vec();
                                    if corrupt {
                                        chunk[0] ^= 0xff;
                                    }
                                    let reply = Message::Extended {
                                        ext: 1,
                                        payload: build_metadata_data(
                                            piece,
                                            metadata.len() as u64,
                                            &chunk,
                                        ),
                                    };
                                    if write_message(&mut stream, &reply).is_err() {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });
    addr
}

#[test]
fn metadata_fetch_reproduces_info_dict() {
    let torrent = fixtures::multi_file_torrent();
    let meta = parse(&torrent).unwrap();
    let info = info_bytes(&torrent);
    let peer = start_metadata_peer(info.clone(), false);
    let fetched = fetch_from_peers(
        meta.info_hash,
        *b"-BG0001-abcdefghijkl",
        &[peer],
        std::time::Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(fetched, info);
    let rebuilt = parse_info_dict(&fetched, meta.trackers.clone()).unwrap();
    assert_eq!(rebuilt, meta);
}

#[test]
fn multi_piece_metadata_reassembles() {
    let mut pieces = Vec::new();
    for i in 0..1000u32 {
        let mut hash = [0u8; 20];
        hash[0..4].copy_from_slice(&i.to_be_bytes());
        pieces.extend_from_slice(&hash);
    }
    let mut info = std::collections::BTreeMap::new();
    info.insert(b"length".to_vec(), Value::Int(1000 * 16384));
    info.insert(b"name".to_vec(), Value::Bytes(b"big.bin".to_vec()));
    info.insert(b"piece length".to_vec(), Value::Int(16384));
    info.insert(b"pieces".to_vec(), Value::Bytes(pieces));
    let info = encode(&Value::Dict(info));
    assert!(info.len() > METADATA_PIECE_SIZE as usize);
    let expected = parse_info_dict(&info, vec![]).unwrap();
    let peer = start_metadata_peer(info.clone(), false);
    let fetched = fetch_from_peers(
        expected.info_hash,
        *b"-BG0001-abcdefghijkl",
        &[peer],
        std::time::Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(fetched, info);
}

#[test]
fn corrupted_metadata_rejected() {
    let torrent = fixtures::single_file_torrent();
    let meta = parse(&torrent).unwrap();
    let info = info_bytes(&torrent);
    let peer = start_metadata_peer(info, true);
    let result = fetch_from_peers(
        meta.info_hash,
        *b"-BG0001-abcdefghijkl",
        &[peer],
        std::time::Duration::from_secs(10),
    );
    assert!(result.unwrap_err().contains("hash mismatch"));
}

#[test]
#[ignore]
fn live_metadata_matches_torrent_file() {
    use bt::metainfo::InfoHash;
    use bt::tracker::{http_announce, AnnounceRequest, Event};

    let torrent = std::fs::read(std::env::var("LIVE_TORRENT_FILE").unwrap()).unwrap();
    let meta = parse(&torrent).unwrap();
    let base = std::env::var("LIVE_HTTP_TRACKER").unwrap();
    let hash = InfoHash::from_hex(&std::env::var("LIVE_INFO_HASH").unwrap()).unwrap();
    assert_eq!(meta.info_hash, hash);
    let response = http_announce(
        &base,
        &AnnounceRequest {
            info_hash: hash,
            peer_id: *b"-BG0001-abcdefghijkl",
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            left: 1,
            event: Event::Started,
        },
        std::time::Duration::from_secs(15),
    )
    .unwrap();
    let fetched = fetch_from_peers(
        hash,
        *b"-BG0001-abcdefghijkl",
        &response.peers,
        std::time::Duration::from_secs(20),
    )
    .unwrap();
    assert_eq!(fetched, info_bytes(&torrent));
    println!("metadata fetched live: {} bytes", fetched.len());
}
