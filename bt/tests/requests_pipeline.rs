use bt::requests::{piece_blocks, BlockRequest, RequestPipeline, BLOCK_SIZE};
use std::time::{Duration, Instant};

fn request(begin: u32) -> BlockRequest {
    BlockRequest {
        piece: 0,
        begin,
        length: BLOCK_SIZE,
    }
}

#[test]
fn blocks_cover_exact_multiple() {
    let blocks = piece_blocks(2 * BLOCK_SIZE as u64);
    assert_eq!(blocks, vec![(0, BLOCK_SIZE), (BLOCK_SIZE, BLOCK_SIZE)]);
}

#[test]
fn blocks_cover_remainder() {
    let blocks = piece_blocks(BLOCK_SIZE as u64 + 100);
    assert_eq!(blocks, vec![(0, BLOCK_SIZE), (BLOCK_SIZE, 100)]);
}

#[test]
fn small_piece_is_one_block() {
    assert_eq!(piece_blocks(12), vec![(0, 12)]);
}

#[test]
fn capacity_is_enforced() {
    let mut pipeline = RequestPipeline::new(2, Duration::from_secs(30));
    let now = Instant::now();
    assert!(pipeline.issue(request(0), now));
    assert!(pipeline.issue(request(BLOCK_SIZE), now));
    assert!(!pipeline.has_slot());
    assert!(!pipeline.issue(request(2 * BLOCK_SIZE), now));
    assert_eq!(pipeline.in_flight(), 2);
}

#[test]
fn duplicate_issue_refused() {
    let mut pipeline = RequestPipeline::new(4, Duration::from_secs(30));
    let now = Instant::now();
    assert!(pipeline.issue(request(0), now));
    assert!(!pipeline.issue(request(0), now));
}

#[test]
fn complete_removes_matching_request() {
    let mut pipeline = RequestPipeline::new(4, Duration::from_secs(30));
    let now = Instant::now();
    pipeline.issue(request(0), now);
    pipeline.issue(request(BLOCK_SIZE), now);
    assert_eq!(pipeline.complete(0, 0, BLOCK_SIZE), Some(request(0)));
    assert_eq!(pipeline.complete(0, 0, BLOCK_SIZE), None);
    assert_eq!(pipeline.in_flight(), 1);
}

#[test]
fn unrelated_complete_ignored() {
    let mut pipeline = RequestPipeline::new(4, Duration::from_secs(30));
    pipeline.issue(request(0), Instant::now());
    assert_eq!(pipeline.complete(9, 0, BLOCK_SIZE), None);
    assert_eq!(pipeline.in_flight(), 1);
}

#[test]
fn expiry_returns_and_removes_timed_out() {
    let mut pipeline = RequestPipeline::new(4, Duration::from_millis(10));
    let start = Instant::now();
    pipeline.issue(request(0), start);
    let later = start + Duration::from_millis(20);
    pipeline.issue(request(BLOCK_SIZE), later);
    assert_eq!(pipeline.expired(later), vec![request(0)]);
    assert_eq!(pipeline.in_flight(), 1);
    assert!(pipeline.contains(&request(BLOCK_SIZE)));
}

#[test]
fn drain_empties_pipeline() {
    let mut pipeline = RequestPipeline::new(4, Duration::from_secs(30));
    let now = Instant::now();
    pipeline.issue(request(0), now);
    pipeline.issue(request(BLOCK_SIZE), now);
    assert_eq!(pipeline.drain().len(), 2);
    assert_eq!(pipeline.in_flight(), 0);
}

#[test]
#[ignore]
fn live_piece_downloads() {
    use bt::metainfo::parse;
    use bt::peer::{exchange_handshake, read_message, write_message, Handshake, Message};
    use bt::peer_state::PeerState;
    use bt::tracker::{http_announce, AnnounceRequest, Event};
    use sha1::{Digest, Sha1};
    use std::net::TcpStream;

    let torrent = std::fs::read(std::env::var("LIVE_TORRENT_FILE").unwrap()).unwrap();
    let meta = parse(&torrent).unwrap();
    let announce_url = meta.trackers[0][0].clone();
    let req = AnnounceRequest {
        info_hash: meta.info_hash,
        peer_id: *b"-BG0001-abcdefghijkl",
        port: 6881,
        uploaded: 0,
        downloaded: 0,
        left: meta.total_length,
        event: Event::Started,
    };
    let response = http_announce(&announce_url, &req, Duration::from_secs(15)).unwrap();
    let piece_len = std::cmp::min(meta.piece_length, meta.total_length);
    let blocks = piece_blocks(piece_len);
    let mine = Handshake {
        info_hash: meta.info_hash,
        peer_id: *b"-BG0001-abcdefghijkl",
        extensions: false,
    };
    let mut lasterr = String::new();
    for addr in response.peers.iter().take(12) {
        match try_piece_zero(addr, &mine, &blocks) {
            Ok(data) => {
                assert_eq!(data.len() as u64, piece_len);
                let digest: [u8; 20] = Sha1::digest(&data).into();
                assert_eq!(digest, meta.pieces[0], "piece hash mismatch from {}", addr);
                println!("piece 0 ok from {} ({} bytes)", addr, data.len());
                return;
            }
            Err(e) => lasterr = format!("{}: {}", addr, e),
        }
    }
    panic!("no peer served piece 0: {}", lasterr);

    fn try_piece_zero(
        addr: &std::net::SocketAddr,
        mine: &Handshake,
        blocks: &[(u32, u32)],
    ) -> Result<Vec<u8>, String> {
        use bt::requests::{BlockRequest, RequestPipeline};
        let mut stream =
            TcpStream::connect_timeout(addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        exchange_handshake(&mut stream, mine).map_err(|e| format!("{:?}", e))?;
        let mut state = PeerState::new();
        let mut pipeline = RequestPipeline::new(8, Duration::from_secs(20));
        let mut has_piece0 = false;
        let mut pending: Vec<(u32, u32)> = blocks.to_vec();
        let total: usize = blocks.iter().map(|(_, l)| *l as usize).sum();
        let mut data = vec![0u8; total];
        let mut received = 0usize;
        let deadline = Instant::now() + Duration::from_secs(60);
        while received < total {
            if Instant::now() > deadline {
                return Err("deadline".to_string());
            }
            if state.can_request() && has_piece0 {
                while pipeline.has_slot() && !pending.is_empty() {
                    let (begin, length) = pending.remove(0);
                    let block = BlockRequest { piece: 0, begin, length };
                    pipeline.issue(block, Instant::now());
                    write_message(
                        &mut stream,
                        &Message::Request { index: 0, begin, length },
                    )
                    .map_err(|e| format!("{:?}", e))?;
                }
            }
            let message = read_message(&mut stream).map_err(|e| format!("{:?}", e))?;
            state.on_message(&message);
            match message {
                Message::Bitfield(bits) => {
                    if bits.first().map(|b| b & 0x80 != 0).unwrap_or(false) {
                        has_piece0 = true;
                        write_message(&mut stream, &Message::Interested)
                            .map_err(|e| format!("{:?}", e))?;
                        state.set_interested(true);
                    } else {
                        return Err("peer lacks piece 0".to_string());
                    }
                }
                Message::Have(0) => {
                    has_piece0 = true;
                    write_message(&mut stream, &Message::Interested)
                        .map_err(|e| format!("{:?}", e))?;
                    state.set_interested(true);
                }
                Message::Choke => {
                    for block in pipeline.drain() {
                        pending.push((block.begin, block.length));
                    }
                }
                Message::Piece { index: 0, begin, data: chunk } => {
                    if pipeline.complete(0, begin, chunk.len() as u32).is_some() {
                        data[begin as usize..begin as usize + chunk.len()]
                            .copy_from_slice(&chunk);
                        received += chunk.len();
                    }
                }
                _ => {}
            }
            for block in pipeline.expired(Instant::now()) {
                pending.push((block.begin, block.length));
            }
        }
        Ok(data)
    }
}
