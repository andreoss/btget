mod fixtures;

use bt::bencode::Value;
use bt::engine::{download, EngineConfig, Event};
use bt::metainfo::{parse, Metainfo};
use bt::peer::{
    decode_handshake, encode_handshake, read_message, write_message, Handshake, Message,
};
use sha1::{Digest, Sha1};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../scratch/tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn big_content() -> Vec<u8> {
    let mut out = Vec::with_capacity(5 * 16384 + 5000);
    let mut state = 0x1234_5678u32;
    while out.len() < 5 * 16384 + 5000 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.extend_from_slice(&state.to_be_bytes());
    }
    out.truncate(5 * 16384 + 5000);
    out
}

fn big_torrent(content: &[u8]) -> Vec<u8> {
    let piece_length = 16384usize;
    let mut pieces = Vec::new();
    for chunk in content.chunks(piece_length) {
        pieces.extend_from_slice(&Sha1::digest(chunk));
    }
    let info = fixtures::dict(vec![
        (b"length".as_slice(), Value::Int(content.len() as i64)),
        (b"name".as_slice(), fixtures::bytes_value(b"big.bin")),
        (b"piece length".as_slice(), Value::Int(piece_length as i64)),
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

struct Counters {
    current: AtomicUsize,
    peak: AtomicUsize,
    served: AtomicUsize,
}

fn start_counting_seeder(meta: Metainfo, content: Vec<u8>, counters: Arc<Counters>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let mut stream = match connection {
                Ok(s) => s,
                Err(_) => break,
            };
            let meta = meta.clone();
            let content = content.clone();
            let counters = counters.clone();
            std::thread::spawn(move || {
                let current = counters.current.fetch_add(1, Ordering::SeqCst) + 1;
                counters.peak.fetch_max(current, Ordering::SeqCst);
                let mut raw = [0u8; 68];
                if stream.read_exact(&mut raw).is_ok() {
                    if let Ok(theirs) = decode_handshake(&raw) {
                        let mine = Handshake {
                            info_hash: theirs.info_hash,
                            peer_id: *b"-SEED01-000000000000",
                            extensions: false,
                        };
                        if stream.write_all(&encode_handshake(&mine)).is_ok() {
                            let pieces = meta.pieces.len() as u32;
                            let mut bits = vec![0u8; pieces.div_ceil(8) as usize];
                            for i in 0..pieces {
                                bits[(i / 8) as usize] |= 0x80 >> (i % 8);
                            }
                            let _ = write_message(&mut stream, &Message::Bitfield(bits));
                            loop {
                                let message = match read_message(&mut stream) {
                                    Ok(m) => m,
                                    Err(_) => break,
                                };
                                match message {
                                    Message::Interested => {
                                        if write_message(&mut stream, &Message::Unchoke).is_err() {
                                            break;
                                        }
                                    }
                                    Message::Request { index, begin, length } => {
                                        let start = (index as u64 * meta.piece_length
                                            + begin as u64)
                                            as usize;
                                        let data =
                                            content[start..start + length as usize].to_vec();
                                        counters.served.fetch_add(1, Ordering::SeqCst);
                                        if write_message(
                                            &mut stream,
                                            &Message::Piece { index, begin, data },
                                        )
                                        .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                counters.current.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    addr
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

#[test]
fn peer_limit_respected_across_swarm() {
    let content = big_content();
    let mut meta = parse(&big_torrent(&content)).unwrap();
    let counters = Arc::new(Counters {
        current: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        served: AtomicUsize::new(0),
    });
    let seeders: Vec<SocketAddr> = (0..3)
        .map(|_| start_counting_seeder(meta.clone(), content.clone(), counters.clone()))
        .collect();
    let tracker = start_tracker(seeders);
    meta.trackers = vec![vec![format!("http://{}/announce", tracker)]];
    let dir = scratch("engine-swarm");
    let config = EngineConfig {
        output_dir: dir.clone(),
        peer_id: *b"-BG0001-abcdefghijkl",
        port: 6881,
        max_peers: 2,
    };
    let mut events = Vec::new();
    download(&meta, &config, &mut |e| events.push(e.clone())).unwrap();
    let written = std::fs::read(dir.join("big.bin")).unwrap();
    assert_eq!(Sha1::digest(&written), Sha1::digest(&content));
    assert!(events.contains(&Event::Complete));
    assert!(counters.peak.load(Ordering::SeqCst) <= 2, "peak {}", counters.peak.load(Ordering::SeqCst));
    assert!(counters.served.load(Ordering::SeqCst) >= 6);
}
