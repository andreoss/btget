mod fixtures;

use bt::bencode::Value;
use bt::engine::{download, EngineConfig, Event};
use bt::metainfo::{parse, InfoHash, Metainfo};
use bt::peer::{
    decode_handshake, encode_handshake, read_message, write_message, Handshake, Message,
};
use bt::pieces::Bitfield;
use bt::resume::{load, save, state_path};
use bt::storage::Storage;
use sha1::{Digest, Sha1};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../scratch/tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn content() -> Vec<u8> {
    let mut out = Vec::new();
    let mut state = 42u32;
    while out.len() < 3 * 16384 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.extend_from_slice(&state.to_be_bytes());
    }
    out.truncate(3 * 16384);
    out
}

fn torrent(content: &[u8]) -> Vec<u8> {
    let mut pieces = Vec::new();
    for chunk in content.chunks(16384) {
        pieces.extend_from_slice(&Sha1::digest(chunk));
    }
    let info = fixtures::dict(vec![
        (b"length".as_slice(), Value::Int(content.len() as i64)),
        (b"name".as_slice(), fixtures::bytes_value(b"resume.bin")),
        (b"piece length".as_slice(), Value::Int(16384)),
        (b"pieces".as_slice(), Value::Bytes(pieces)),
    ]);
    let top = fixtures::dict(vec![
        (
            b"announce".as_slice(),
            fixtures::bytes_value(b"http://127.0.0.1:1/announce"),
        ),
        (b"info".as_slice(), info),
    ]);
    bt::bencode::encode(&top)
}

fn start_tracker(peers: Vec<SocketAddr>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let mut stream = match connection {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let mut compact = Vec::new();
            for peer in &peers {
                if let SocketAddr::V4(v4) = peer {
                    compact.extend_from_slice(&v4.ip().octets());
                    compact.extend_from_slice(&v4.port().to_be_bytes());
                }
            }
            let mut body = format!("d8:intervali1800e5:peers{}:", compact.len()).into_bytes();
            body.extend_from_slice(&compact);
            body.push(b'e');
            let mut response =
                format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
            response.extend_from_slice(&body);
            let _ = stream.write_all(&response);
        }
    });
    addr
}

fn start_seeder(meta: Metainfo, content: Vec<u8>, served: Arc<Mutex<Vec<u32>>>) -> SocketAddr {
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
                extensions: false,
            };
            if stream.write_all(&encode_handshake(&mine)).is_err() {
                continue;
            }
            let pieces = meta.pieces.len() as u32;
            let mut bits = vec![0u8; pieces.div_ceil(8) as usize];
            for i in 0..pieces {
                bits[(i / 8) as usize] |= 0x80 >> (i % 8);
            }
            if write_message(&mut stream, &Message::Bitfield(bits)).is_err() {
                continue;
            }
            let content = content.clone();
            let served = served.clone();
            let piece_length = meta.piece_length;
            loop {
                match read_message(&mut stream) {
                    Ok(Message::Interested) => {
                        if write_message(&mut stream, &Message::Unchoke).is_err() {
                            break;
                        }
                    }
                    Ok(Message::Request { index, begin, length }) => {
                        served.lock().unwrap().push(index);
                        let start = (index as u64 * piece_length + begin as u64) as usize;
                        let data = content[start..start + length as usize].to_vec();
                        if write_message(&mut stream, &Message::Piece { index, begin, data })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
    });
    addr
}

fn config(dir: &PathBuf) -> EngineConfig {
    EngineConfig {
        output_dir: dir.clone(),
        peer_id: *b"-BG0001-abcdefghijkl",
        port: 6881,
        max_peers: 2,
        bootstrap_peers: vec![],
    }
}

#[test]
fn state_round_trip_and_hash_check() {
    let dir = scratch("resume-state");
    let path = state_path(&dir, "resume.bin");
    let mut have = Bitfield::new(3);
    have.set(0);
    have.set(2);
    save(&path, InfoHash([5u8; 20]), &have).unwrap();
    assert_eq!(load(&path, InfoHash([5u8; 20]), 3), Some(have));
    assert_eq!(load(&path, InfoHash([6u8; 20]), 3), None);
    assert_eq!(load(&path, InfoHash([5u8; 20]), 4), None);
}

#[test]
fn resume_skips_verified_pieces() {
    let data = content();
    let meta_bytes = torrent(&data);
    let mut meta = parse(&meta_bytes).unwrap();
    let dir = scratch("resume-skip");
    let storage = Storage::from_metainfo(&meta, &dir);
    storage.allocate().unwrap();
    storage.write_block(0, 0, &data[..16384]).unwrap();
    storage.write_block(1, 0, &data[16384..32768]).unwrap();
    let mut have = Bitfield::new(3);
    have.set(0);
    have.set(1);
    save(&state_path(&dir, "resume.bin"), meta.info_hash, &have).unwrap();
    let served = Arc::new(Mutex::new(Vec::new()));
    let seeder = start_seeder(meta.clone(), data.clone(), served.clone());
    let tracker = start_tracker(vec![seeder]);
    meta.trackers = vec![vec![format!("http://{}/announce", tracker)]];
    let mut events = Vec::new();
    download(&meta, &config(&dir), &mut |e| events.push(e.clone())).unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Resumed { have: 2, total: 3, .. })));
    let served = served.lock().unwrap();
    assert!(!served.contains(&0), "piece 0 refetched: {:?}", served);
    assert!(!served.contains(&1), "piece 1 refetched: {:?}", served);
    assert!(served.contains(&2));
    assert_eq!(std::fs::read(dir.join("resume.bin")).unwrap(), data);
    assert!(!state_path(&dir, "resume.bin").exists());
}

#[test]
fn corrupt_claimed_piece_is_refetched() {
    let data = content();
    let meta_bytes = torrent(&data);
    let mut meta = parse(&meta_bytes).unwrap();
    let dir = scratch("resume-corrupt");
    let storage = Storage::from_metainfo(&meta, &dir);
    storage.allocate().unwrap();
    storage.write_block(0, 0, &vec![0xff; 16384]).unwrap();
    storage.write_block(1, 0, &data[16384..32768]).unwrap();
    let mut have = Bitfield::new(3);
    have.set(0);
    have.set(1);
    save(&state_path(&dir, "resume.bin"), meta.info_hash, &have).unwrap();
    let served = Arc::new(Mutex::new(Vec::new()));
    let seeder = start_seeder(meta.clone(), data.clone(), served.clone());
    let tracker = start_tracker(vec![seeder]);
    meta.trackers = vec![vec![format!("http://{}/announce", tracker)]];
    download(&meta, &config(&dir), &mut |_| {}).unwrap();
    let served = served.lock().unwrap();
    assert!(served.contains(&0));
    assert!(!served.contains(&1));
    assert_eq!(std::fs::read(dir.join("resume.bin")).unwrap(), data);
}
