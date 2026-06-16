mod fixtures;

use bt::engine::{download, EngineConfig, Error, Event};
use bt::metainfo::{parse, Metainfo};
use bt::peer::{
    decode_handshake, encode_handshake, read_message, write_message, Handshake, Message,
};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../scratch/tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
            let mut compact6 = Vec::new();
            for peer in &peers {
                match peer {
                    SocketAddr::V4(v4) => {
                        compact.extend_from_slice(&v4.ip().octets());
                        compact.extend_from_slice(&v4.port().to_be_bytes());
                    }
                    SocketAddr::V6(v6) => {
                        compact6.extend_from_slice(&v6.ip().octets());
                        compact6.extend_from_slice(&v6.port().to_be_bytes());
                    }
                }
            }
            let mut body = format!("d8:intervali1800e5:peers{}:", compact.len()).into_bytes();
            body.extend_from_slice(&compact);
            body.extend_from_slice(format!("6:peers6{}:", compact6.len()).as_bytes());
            body.extend_from_slice(&compact6);
            body.push(b'e');
            let mut response = format!(
                "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            response.extend_from_slice(&body);
            let _ = stream.write_all(&response);
        }
    });
    addr
}

fn start_seeder(meta: Metainfo, content: Vec<u8>, corrupt_first: bool) -> SocketAddr {
    start_seeder_on("127.0.0.1:0", meta, content, corrupt_first)
}

fn start_seeder_on(
    bind: &str,
    meta: Metainfo,
    content: Vec<u8>,
    corrupt_first: bool,
) -> SocketAddr {
    let listener = TcpListener::bind(bind).unwrap();
    let addr = listener.local_addr().unwrap();
    let corrupted = Arc::new(AtomicBool::new(false));
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
            let corrupted = corrupted.clone();
            let content = content.clone();
            let piece_length = meta.piece_length;
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
                        let start = (index as u64 * piece_length + begin as u64) as usize;
                        let mut data = content[start..start + length as usize].to_vec();
                        if corrupt_first && !corrupted.swap(true, Ordering::SeqCst) {
                            data[0] ^= 0xff;
                        }
                        if write_message(&mut stream, &Message::Piece { index, begin, data })
                            .is_err()
                        {
                            break;
                        }
                    }
                    _ => {}
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
        max_peers: 4,
        bootstrap_peers: vec![],
    }
}

fn with_tracker(mut meta: Metainfo, tracker: SocketAddr) -> Metainfo {
    meta.trackers = vec![vec![format!("http://{}/announce", tracker)]];
    meta
}

#[test]
fn single_file_downloads_end_to_end() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    let seeder = start_seeder(meta.clone(), b"hello world\n".to_vec(), false);
    let tracker = start_tracker(vec![seeder]);
    let meta = with_tracker(meta, tracker);
    let dir = scratch("engine-single");
    let mut events = Vec::new();
    download(&meta, &config(&dir), &mut |e| events.push(e.clone())).unwrap();
    assert_eq!(
        std::fs::read(dir.join("demo.bin")).unwrap(),
        b"hello world\n"
    );
    assert!(events.contains(&Event::Complete));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::PieceDone { have: 1, total: 1, .. })));
}

#[test]
fn corrupt_piece_is_refetched() {
    let meta = parse(&fixtures::multi_file_torrent()).unwrap();
    let seeder = start_seeder(meta.clone(), b"alphabeta".to_vec(), true);
    let tracker = start_tracker(vec![seeder]);
    let meta = with_tracker(meta, tracker);
    let dir = scratch("engine-corrupt");
    let mut events = Vec::new();
    download(&meta, &config(&dir), &mut |e| events.push(e.clone())).unwrap();
    assert_eq!(
        std::fs::read(dir.join("demo-dir/a.txt")).unwrap(),
        b"alpha"
    );
    assert_eq!(
        std::fs::read(dir.join("demo-dir/sub/b.txt")).unwrap(),
        b"beta"
    );
    assert!(events.contains(&Event::Complete));
}

#[test]
fn dead_tracker_failure_is_reported_with_url() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    let seeder = start_seeder(meta.clone(), b"hello world\n".to_vec(), false);
    let tracker = start_tracker(vec![seeder]);
    let mut meta = meta;
    let dead_url = "http://127.0.0.1:1/announce".to_string();
    meta.trackers = vec![
        vec![dead_url.clone()],
        vec![format!("http://{}/announce", tracker)],
    ];
    let dir = scratch("engine-dead-tracker");
    let mut events = Vec::new();
    download(&meta, &config(&dir), &mut |e| events.push(e.clone())).unwrap();
    assert!(events.contains(&Event::Complete));
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::AnnounceFailed { url, .. } if *url == dead_url
        )),
        "no failure event named the dead tracker: {:?}",
        events
            .iter()
            .filter(|e| !matches!(e, Event::PieceDone { .. }))
            .collect::<Vec<_>>()
    );
}

#[test]
fn v6_peer_downloads_end_to_end() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    let seeder = match std::panic::catch_unwind(|| {
        start_seeder_on("[::1]:0", meta.clone(), b"hello world\n".to_vec(), false)
    }) {
        Ok(addr) => addr,
        Err(_) => return,
    };
    assert!(seeder.is_ipv6());
    let tracker = start_tracker(vec![seeder]);
    let meta = with_tracker(meta, tracker);
    let dir = scratch("engine-v6");
    let mut events = Vec::new();
    download(&meta, &config(&dir), &mut |e| events.push(e.clone())).unwrap();
    assert_eq!(
        std::fs::read(dir.join("demo.bin")).unwrap(),
        b"hello world\n"
    );
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Connected { addr } if addr.is_ipv6())));
}

#[test]
fn unreachable_peer_failure_is_reported_with_addr() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    let seeder = start_seeder(meta.clone(), b"hello world\n".to_vec(), false);
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let tracker = start_tracker(vec![dead, seeder]);
    let meta = with_tracker(meta, tracker);
    let dir = scratch("engine-dead-peer");
    let mut events = Vec::new();
    download(&meta, &config(&dir), &mut |e| events.push(e.clone())).unwrap();
    assert!(events.contains(&Event::Complete));
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::PeerFailed { addr, .. } if *addr == dead
        )),
        "no failure event named the dead peer: {:?}",
        events
            .iter()
            .filter(|e| !matches!(e, Event::PieceDone { .. }))
            .collect::<Vec<_>>()
    );
}

#[test]
fn empty_swarm_errors_after_rounds() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    let tracker = start_tracker(vec![]);
    let meta = with_tracker(meta, tracker);
    let dir = scratch("engine-empty");
    let result = download(&meta, &config(&dir), &mut |_| {});
    assert_eq!(result, Err(Error::NoPeers));
}

#[test]
#[ignore]
fn live_download_completes() {
    let path = std::env::var("LIVE_TORRENT_FILE").unwrap();
    let meta = parse(&std::fs::read(path).unwrap()).unwrap();
    let dir = scratch("engine-live");
    let started = std::time::Instant::now();
    let mut last = 0u32;
    download(&meta, &config(&dir), &mut |e| {
        if let Event::PieceDone { have, total, .. } = e {
            if have % 50 == 0 || *have == *total {
                println!(
                    "pieces {}/{} elapsed {:?}",
                    have,
                    total,
                    started.elapsed()
                );
            }
            last = *have;
        }
    })
    .unwrap();
    println!("done: {} pieces in {:?}", last, started.elapsed());
}
