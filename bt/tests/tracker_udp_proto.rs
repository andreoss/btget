use bt::metainfo::InfoHash;
use bt::tracker::{AnnounceRequest, Error, Event};
use bt::tracker_udp::{
    build_announce, build_connect, parse_announce, parse_connect, udp_announce, udp_tracker_addr,
};
use std::net::UdpSocket;
use std::time::Duration;

fn request() -> AnnounceRequest {
    AnnounceRequest {
        info_hash: InfoHash([0xab; 20]),
        peer_id: *b"-BG0001-abcdefghijkl",
        port: 6881,
        uploaded: 1,
        downloaded: 2,
        left: 3,
        event: Event::Started,
    }
}

#[test]
fn connect_packet_layout() {
    let packet = build_connect(0xdead_beef);
    assert_eq!(&packet[0..8], &0x0417_2710_1980u64.to_be_bytes());
    assert_eq!(&packet[8..12], &[0, 0, 0, 0]);
    assert_eq!(&packet[12..16], &0xdead_beefu32.to_be_bytes());
}

#[test]
fn connect_response_parses() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&0u32.to_be_bytes());
    raw.extend_from_slice(&7u32.to_be_bytes());
    raw.extend_from_slice(&99u64.to_be_bytes());
    assert_eq!(parse_connect(&raw, 7).unwrap(), 99);
}

#[test]
fn connect_response_txid_mismatch_rejected() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&0u32.to_be_bytes());
    raw.extend_from_slice(&8u32.to_be_bytes());
    raw.extend_from_slice(&99u64.to_be_bytes());
    assert_eq!(parse_connect(&raw, 7), Err(Error::BadResponse));
}

#[test]
fn announce_packet_layout() {
    let packet = build_announce(0x1122_3344_5566_7788, 5, &request());
    assert_eq!(&packet[0..8], &0x1122_3344_5566_7788u64.to_be_bytes());
    assert_eq!(&packet[8..12], &1u32.to_be_bytes());
    assert_eq!(&packet[12..16], &5u32.to_be_bytes());
    assert_eq!(&packet[16..36], &[0xab; 20]);
    assert_eq!(&packet[36..56], b"-BG0001-abcdefghijkl");
    assert_eq!(&packet[56..64], &2u64.to_be_bytes());
    assert_eq!(&packet[64..72], &3u64.to_be_bytes());
    assert_eq!(&packet[72..80], &1u64.to_be_bytes());
    assert_eq!(&packet[80..84], &2u32.to_be_bytes());
    assert_eq!(&packet[92..96], &(-1i32).to_be_bytes());
    assert_eq!(&packet[96..98], &6881u16.to_be_bytes());
}

#[test]
fn announce_response_parses_peers() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&1u32.to_be_bytes());
    raw.extend_from_slice(&5u32.to_be_bytes());
    raw.extend_from_slice(&900u32.to_be_bytes());
    raw.extend_from_slice(&2u32.to_be_bytes());
    raw.extend_from_slice(&10u32.to_be_bytes());
    raw.extend_from_slice(&[127, 0, 0, 1, 0x1a, 0xe1]);
    raw.extend_from_slice(&[10, 0, 0, 9, 0, 0]);
    let response = parse_announce(&raw, 5, false).unwrap();
    assert_eq!(response.interval, 900);
    assert_eq!(response.peers, vec!["127.0.0.1:6881".parse().unwrap()]);
}

#[test]
fn error_action_surfaces_message() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&3u32.to_be_bytes());
    raw.extend_from_slice(&5u32.to_be_bytes());
    raw.extend_from_slice(b"denied");
    assert_eq!(
        parse_announce(&raw, 5, false),
        Err(Error::Failure("denied".to_string()))
    );
    assert_eq!(
        parse_connect(&{
            let mut c = raw.clone();
            c.extend_from_slice(&[0, 0]);
            c
        }, 5),
        Err(Error::Failure("denied\0\0".to_string()))
    );
}

#[test]
fn short_responses_rejected() {
    assert_eq!(parse_connect(&[0; 15], 0), Err(Error::BadResponse));
    assert_eq!(parse_announce(&[0; 19], 0, false), Err(Error::BadResponse));
}

#[test]
fn url_parsing() {
    assert_eq!(
        udp_tracker_addr("udp://127.0.0.1:8081").unwrap(),
        ("127.0.0.1".to_string(), 8081)
    );
    assert_eq!(
        udp_tracker_addr("udp://t.example:80/announce").unwrap(),
        ("t.example".to_string(), 80)
    );
    assert!(udp_tracker_addr("http://x:1").is_err());
    assert!(udp_tracker_addr("udp://noport").is_err());
}

fn fake_udp_tracker(drop_first_connect: bool) -> std::net::SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut dropped = !drop_first_connect;
        let mut buf = [0u8; 2048];
        loop {
            let (n, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => return,
            };
            if n >= 16 && buf[8..12] == 0u32.to_be_bytes() {
                if !dropped {
                    dropped = true;
                    continue;
                }
                let txid = &buf[12..16];
                let mut reply = Vec::new();
                reply.extend_from_slice(&0u32.to_be_bytes());
                reply.extend_from_slice(txid);
                reply.extend_from_slice(&0x4242u64.to_be_bytes());
                let _ = socket.send_to(&reply, from);
            } else if n >= 98 {
                let txid = &buf[12..16];
                let mut reply = Vec::new();
                reply.extend_from_slice(&1u32.to_be_bytes());
                reply.extend_from_slice(txid);
                reply.extend_from_slice(&1800u32.to_be_bytes());
                reply.extend_from_slice(&0u32.to_be_bytes());
                reply.extend_from_slice(&1u32.to_be_bytes());
                reply.extend_from_slice(&[127, 0, 0, 1, 0x1a, 0xe1]);
                let _ = socket.send_to(&reply, from);
            }
        }
    });
    addr
}

#[test]
fn local_udp_announce_round_trip() {
    let addr = fake_udp_tracker(false);
    let url = format!("udp://{}", addr);
    let response = udp_announce(&url, &request(), &[Duration::from_secs(5)]).unwrap();
    assert_eq!(response.interval, 1800);
    assert_eq!(response.peers, vec!["127.0.0.1:6881".parse().unwrap()]);
}

#[test]
fn retransmit_after_dropped_packet() {
    let addr = fake_udp_tracker(true);
    let url = format!("udp://{}", addr);
    let response = udp_announce(
        &url,
        &request(),
        &[Duration::from_millis(300), Duration::from_secs(5)],
    )
    .unwrap();
    assert_eq!(response.interval, 1800);
}

#[test]
fn no_reply_times_out() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let url = format!("udp://{}", socket.local_addr().unwrap());
    let result = udp_announce(&url, &request(), &[Duration::from_millis(200)]);
    assert!(matches!(result, Err(Error::Io(_))));
}

#[test]
#[ignore]
fn live_udp_announce_returns_peers() {
    let url = std::env::var("LIVE_UDP_TRACKER").unwrap();
    let hash = std::env::var("LIVE_INFO_HASH").unwrap();
    let mut req = request();
    req.info_hash = InfoHash::from_hex(&hash).unwrap();
    req.left = 1;
    let response = udp_announce(
        &url,
        &req,
        &[Duration::from_secs(5), Duration::from_secs(10)],
    )
    .unwrap();
    println!("interval {} peers {}", response.interval, response.peers.len());
    assert!(response.interval > 0);
}

#[test]
fn announce_response_parses_v6_peers() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&1u32.to_be_bytes());
    raw.extend_from_slice(&5u32.to_be_bytes());
    raw.extend_from_slice(&900u32.to_be_bytes());
    raw.extend_from_slice(&2u32.to_be_bytes());
    raw.extend_from_slice(&10u32.to_be_bytes());
    let mut ip = [0u8; 16];
    ip[15] = 1;
    raw.extend_from_slice(&ip);
    raw.extend_from_slice(&0x1ae1u16.to_be_bytes());
    let response = parse_announce(&raw, 5, true).unwrap();
    assert_eq!(response.peers, vec!["[::1]:6881".parse().unwrap()]);
}

#[test]
fn bracketed_v6_url_parses() {
    assert_eq!(
        udp_tracker_addr("udp://[2001:db8::7]:6969/announce").unwrap(),
        ("2001:db8::7".to_string(), 6969)
    );
}

fn fake_udp_tracker_v6() -> std::net::SocketAddr {
    let socket = UdpSocket::bind("[::1]:0").unwrap();
    let addr = socket.local_addr().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 2048];
        loop {
            let (n, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => return,
            };
            if n >= 16 && buf[8..12] == 0u32.to_be_bytes() {
                let txid = &buf[12..16];
                let mut reply = Vec::new();
                reply.extend_from_slice(&0u32.to_be_bytes());
                reply.extend_from_slice(txid);
                reply.extend_from_slice(&0x4242u64.to_be_bytes());
                let _ = socket.send_to(&reply, from);
            } else if n >= 98 {
                let txid = &buf[12..16];
                let mut reply = Vec::new();
                reply.extend_from_slice(&1u32.to_be_bytes());
                reply.extend_from_slice(txid);
                reply.extend_from_slice(&1800u32.to_be_bytes());
                reply.extend_from_slice(&0u32.to_be_bytes());
                reply.extend_from_slice(&1u32.to_be_bytes());
                let mut ip = [0u8; 16];
                ip[15] = 1;
                reply.extend_from_slice(&ip);
                reply.extend_from_slice(&0x1ae1u16.to_be_bytes());
                let _ = socket.send_to(&reply, from);
            }
        }
    });
    addr
}

#[test]
fn local_v6_udp_announce_round_trip() {
    let addr = fake_udp_tracker_v6();
    let url = format!("udp://[::1]:{}", addr.port());
    let response = udp_announce(&url, &request(), &[Duration::from_secs(5)]).unwrap();
    assert_eq!(response.interval, 1800);
    assert_eq!(response.peers, vec!["[::1]:6881".parse().unwrap()]);
}
