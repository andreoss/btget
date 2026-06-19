use bt::metainfo::InfoHash;
use bt::tracker::{
    build_announce_url, escape_bytes, http_announce, parse_response, AnnounceRequest,
    AnnounceResponse, Error, Event,
};
use std::io::{Read, Write};
use std::net::{TcpListener, SocketAddr};
use std::time::Duration;

fn request() -> AnnounceRequest {
    AnnounceRequest {
        info_hash: InfoHash([0x01; 20]),
        peer_id: *b"-BG0001-abcdefghijkl",
        port: 6881,
        uploaded: 0,
        downloaded: 0,
        left: 1000,
        event: Event::Started,
    }
}

#[test]
fn escaping_covers_reserved_bytes() {
    assert_eq!(escape_bytes(b"Az09-_.~"), "Az09-_.~");
    assert_eq!(escape_bytes(&[0x00, 0x1f, 0xff, b' ', b'/']), "%00%1F%FF%20%2F");
}

#[test]
fn announce_url_contains_all_fields() {
    let url = build_announce_url("http://127.0.0.1:9999/announce", &request()).unwrap();
    assert!(url.starts_with("http://127.0.0.1:9999/announce?info_hash=%01%01"));
    assert!(url.contains("&peer_id=-BG0001-abcdefghijkl"));
    assert!(url.contains("&port=6881"));
    assert!(url.contains("&left=1000"));
    assert!(url.contains("&compact=1"));
    assert!(url.contains("&event=started"));
}

#[test]
fn existing_query_is_extended() {
    let url = build_announce_url("http://127.0.0.1/announce?key=1", &request()).unwrap();
    assert!(url.contains("announce?key=1&info_hash="));
}

#[test]
fn non_http_scheme_rejected() {
    assert!(matches!(
        build_announce_url("udp://127.0.0.1:1", &request()),
        Err(Error::UnsupportedUrl(_))
    ));
}

#[test]
fn compact_peers_parse() {
    let body = b"d8:intervali900e5:peers12:\x7f\x00\x00\x01\x1a\xe1\x0a\x00\x00\x02\x1a\xe2e";
    let response = parse_response(body).unwrap();
    assert_eq!(response.interval, 900);
    assert_eq!(
        response.peers,
        vec![
            "127.0.0.1:6881".parse::<SocketAddr>().unwrap(),
            "10.0.0.2:6882".parse::<SocketAddr>().unwrap(),
        ]
    );
}

#[test]
fn zero_port_peers_dropped() {
    let body = b"d8:intervali900e5:peers6:\x7f\x00\x00\x01\x00\x00e";
    assert_eq!(parse_response(body).unwrap().peers, vec![]);
}

#[test]
fn dict_peers_parse() {
    let body =
        b"d8:intervali60e5:peersld2:ip9:127.0.0.14:porti6881eed2:ip3:bad4:porti1eeee";
    let response = parse_response(body).unwrap();
    assert_eq!(response.peers, vec!["127.0.0.1:6881".parse::<SocketAddr>().unwrap()]);
}

#[test]
fn failure_reason_surfaces() {
    let body = b"d14:failure reason9:not founde";
    assert_eq!(
        parse_response(body),
        Err(Error::Failure("not found".to_string()))
    );
}

#[test]
fn truncated_compact_peers_rejected() {
    let body = b"d8:intervali900e5:peers5:\x7f\x00\x00\x01\x1ae";
    assert_eq!(parse_response(body), Err(Error::BadResponse));
}

#[test]
fn garbage_rejected() {
    assert_eq!(parse_response(b"hello"), Err(Error::BadResponse));
}

#[test]
fn local_announce_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap();
        let request_text = String::from_utf8_lossy(&buf[..n]).to_string();
        let body: &[u8] = b"d8:intervali1800e5:peers6:\x7f\x00\x00\x01\x1a\xe1e";
        let mut response = format!(
            "HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        stream.write_all(&response).unwrap();
        request_text
    });
    let base = format!("http://{}/announce", addr);
    let response = http_announce(&base, &request(), Duration::from_secs(5)).unwrap();
    assert_eq!(
        response,
        AnnounceResponse {
            interval: 1800,
            peers: vec!["127.0.0.1:6881".parse().unwrap()],
        }
    );
    let seen = server.join().unwrap();
    assert!(seen.starts_with("GET /announce?info_hash=%01%01"));
    assert!(seen.contains("HTTP/1.0"));
    assert!(seen.contains("Connection: close"));
}

#[test]
fn http_error_status_surfaces() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        stream
            .write_all(b"HTTP/1.0 404 Not Found\r\n\r\n")
            .unwrap();
    });
    let base = format!("http://{}/announce", addr);
    assert_eq!(
        http_announce(&base, &request(), Duration::from_secs(5)),
        Err(Error::HttpStatus(404))
    );
}

#[test]
#[ignore]
fn live_announce_returns_peers() {
    let base = std::env::var("LIVE_HTTP_TRACKER").unwrap();
    let hash = std::env::var("LIVE_INFO_HASH").unwrap();
    let mut req = request();
    req.info_hash = InfoHash::from_hex(&hash).unwrap();
    let response = http_announce(&base, &req, Duration::from_secs(15)).unwrap();
    assert!(response.interval > 0);
    assert!(!response.peers.is_empty());
}

#[test]
fn compact_v6_peers_parse() {
    let mut body = b"d8:intervali900e5:peers0:6:peers618:".to_vec();
    let mut ip = [0u8; 16];
    ip[15] = 1;
    body.extend_from_slice(&ip);
    body.extend_from_slice(&0x1ae1u16.to_be_bytes());
    body.push(b'e');
    let response = parse_response(&body).unwrap();
    assert_eq!(response.peers, vec!["[::1]:6881".parse::<SocketAddr>().unwrap()]);
}

#[test]
fn v6_only_response_parses() {
    let mut body = b"d8:intervali900e6:peers618:".to_vec();
    let mut ip = [0u8; 16];
    ip[15] = 1;
    body.extend_from_slice(&ip);
    body.extend_from_slice(&0x1ae1u16.to_be_bytes());
    body.push(b'e');
    assert_eq!(parse_response(&body).unwrap().peers.len(), 1);
}

#[test]
fn mixed_families_combine() {
    let mut body = b"d8:intervali900e5:peers6:\x7f\x00\x00\x01\x1a\xe16:peers618:".to_vec();
    let mut ip = [0u8; 16];
    ip[15] = 1;
    body.extend_from_slice(&ip);
    body.extend_from_slice(&0x1ae1u16.to_be_bytes());
    body.push(b'e');
    let response = parse_response(&body).unwrap();
    assert_eq!(response.peers.len(), 2);
}

#[test]
fn truncated_v6_peers_rejected() {
    let body = b"d8:intervali900e6:peers610:0123456789e";
    assert_eq!(parse_response(body), Err(Error::BadResponse));
}

#[test]
fn no_peer_key_rejected() {
    assert_eq!(parse_response(b"d8:intervali900ee"), Err(Error::BadResponse));
}

#[test]
fn bracketed_v6_host_announces_locally() {
    let listener = match TcpListener::bind("[::1]:0") {
        Ok(l) => l,
        Err(_) => return,
    };
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap();
        let seen = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(seen.contains(&format!("Host: [::1]:{}", addr.port())), "{}", seen);
        let body: &[u8] = b"d8:intervali1800e5:peers6:\x7f\x00\x00\x01\x1a\xe1e";
        let mut response = format!(
            "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        stream.write_all(&response).unwrap();
    });
    let base = format!("http://[::1]:{}/announce", addr.port());
    let response = http_announce(&base, &request(), Duration::from_secs(5)).unwrap();
    assert_eq!(response.interval, 1800);
}

#[test]
fn control_bytes_in_the_url_are_refused() {
    for base in [
        "http://127.0.0.1:9999/announce\r\nX-Injected: yes",
        "http://127.0.0.1:9999/announce\nX-Injected: yes",
        "http://127.0.0.1:9999/announce HTTP/1.0",
        "http://127.0.0.1:9999/announce\0",
    ] {
        assert_eq!(
            build_announce_url(base, &request()),
            Err(Error::UnsupportedUrl(base.to_string())),
            "accepted {:?}",
            base
        );
        assert_eq!(
            http_announce(base, &request(), Duration::from_secs(5)),
            Err(Error::UnsupportedUrl(base.to_string())),
            "announced to {:?}",
            base
        );
    }
}

#[test]
fn oversized_response_does_not_grow_without_bound() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.0 200 OK\r\n\r\nd8:intervali1800e5:peers");
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..64 {
            if stream.write_all(&chunk).is_err() {
                return;
            }
        }
    });
    let base = format!("http://{}/announce", addr);
    assert_eq!(
        http_announce(&base, &request(), Duration::from_secs(5)),
        Err(Error::BadResponse)
    );
}
