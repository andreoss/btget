use bt::metainfo::InfoHash;
use bt::peer::{
    decode_handshake, encode_handshake, encode_message, exchange_handshake, parse_frame,
    read_message, write_message, Error, Handshake, Message, MAX_FRAME,
};
use std::io::Cursor;

fn ours() -> Handshake {
    Handshake {
        info_hash: InfoHash([7u8; 20]),
        peer_id: *b"-BG0001-abcdefghijkl",
        extensions: true,
    }
}

#[test]
fn handshake_round_trip() {
    let raw = encode_handshake(&ours());
    assert_eq!(raw[0], 19);
    assert_eq!(&raw[1..20], b"BitTorrent protocol");
    assert_eq!(decode_handshake(&raw).unwrap(), ours());
}

#[test]
fn handshake_without_extensions() {
    let mut hs = ours();
    hs.extensions = false;
    let raw = encode_handshake(&hs);
    assert_eq!(raw[25] & 0x10, 0);
    assert!(!decode_handshake(&raw).unwrap().extensions);
}

#[test]
fn bad_pstr_rejected() {
    let mut raw = encode_handshake(&ours());
    raw[3] = b'X';
    assert_eq!(decode_handshake(&raw), Err(Error::BadHandshake));
}

#[test]
fn exchange_rejects_wrong_hash() {
    let mut theirs = ours();
    theirs.info_hash = InfoHash([9u8; 20]);
    let their_raw = encode_handshake(&theirs);
    let mut stream = FakeStream {
        read: their_raw.to_vec(),
        written: Vec::new(),
        pos: 0,
    };
    assert_eq!(exchange_handshake(&mut stream, &ours()), Err(Error::WrongInfoHash));
    assert_eq!(stream.written, encode_handshake(&ours()).to_vec());
}

#[test]
fn exchange_accepts_matching_hash() {
    let mut theirs = ours();
    theirs.peer_id = *b"-XX0001-000000000000";
    let mut stream = FakeStream {
        read: encode_handshake(&theirs).to_vec(),
        written: Vec::new(),
        pos: 0,
    };
    assert_eq!(exchange_handshake(&mut stream, &ours()).unwrap(), theirs);
}

struct FakeStream {
    read: Vec<u8>,
    written: Vec<u8>,
    pos: usize,
}

impl std::io::Read for FakeStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = std::cmp::min(buf.len(), self.read.len() - self.pos);
        buf[..n].copy_from_slice(&self.read[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl std::io::Write for FakeStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn all_messages() -> Vec<Message> {
    vec![
        Message::KeepAlive,
        Message::Choke,
        Message::Unchoke,
        Message::Interested,
        Message::NotInterested,
        Message::Have(42),
        Message::Bitfield(vec![0b1010_0000, 0x01]),
        Message::Request { index: 1, begin: 16384, length: 16384 },
        Message::Piece { index: 2, begin: 32768, data: vec![9u8; 100] },
        Message::Cancel { index: 1, begin: 16384, length: 16384 },
        Message::Port(6881),
    ]
}

#[test]
fn message_round_trip() {
    for message in all_messages() {
        let framed = encode_message(&message);
        let mut cursor = Cursor::new(framed);
        assert_eq!(read_message(&mut cursor).unwrap(), message);
    }
}

#[test]
fn stream_of_messages_reads_in_order() {
    let mut wire = Vec::new();
    for message in all_messages() {
        write_message(&mut wire, &message).unwrap();
    }
    let mut cursor = Cursor::new(wire);
    for message in all_messages() {
        assert_eq!(read_message(&mut cursor).unwrap(), message);
    }
}

#[test]
fn keep_alive_is_zero_length() {
    assert_eq!(encode_message(&Message::KeepAlive), vec![0, 0, 0, 0]);
}

#[test]
fn unknown_id_rejected() {
    assert_eq!(parse_frame(&[42]), Err(Error::UnknownId(42)));
}

#[test]
fn short_payloads_rejected() {
    assert_eq!(parse_frame(&[4, 0, 0]), Err(Error::BadFrame));
    assert_eq!(parse_frame(&[6, 0, 0, 0, 1]), Err(Error::BadFrame));
    assert_eq!(parse_frame(&[7, 0, 0, 0, 1]), Err(Error::BadFrame));
    assert_eq!(parse_frame(&[9, 0]), Err(Error::BadFrame));
    assert_eq!(parse_frame(&[0, 1]), Err(Error::BadFrame));
}

#[test]
fn oversize_frame_rejected() {
    let mut wire = (MAX_FRAME + 1).to_be_bytes().to_vec();
    wire.extend_from_slice(&[0u8; 8]);
    let mut cursor = Cursor::new(wire);
    assert_eq!(read_message(&mut cursor), Err(Error::FrameTooLong(MAX_FRAME + 1)));
}

#[test]
fn truncated_stream_errors() {
    let framed = encode_message(&Message::Have(1));
    let mut cursor = Cursor::new(framed[..6].to_vec());
    assert!(matches!(read_message(&mut cursor), Err(Error::Io(_))));
}

#[test]
#[ignore]
fn live_handshake_completes() {
    use bt::tracker::{http_announce, AnnounceRequest, Event};
    use std::net::TcpStream;
    use std::time::Duration;

    let base = std::env::var("LIVE_HTTP_TRACKER").unwrap();
    let hash = InfoHash::from_hex(&std::env::var("LIVE_INFO_HASH").unwrap()).unwrap();
    let req = AnnounceRequest {
        info_hash: hash,
        peer_id: *b"-BG0001-abcdefghijkl",
        port: 6881,
        uploaded: 0,
        downloaded: 0,
        left: 1,
        event: Event::Started,
    };
    let response = http_announce(&base, &req, Duration::from_secs(15)).unwrap();
    assert!(!response.peers.is_empty());
    let mine = Handshake {
        info_hash: hash,
        peer_id: *b"-BG0001-abcdefghijkl",
        extensions: true,
    };
    let mut lasterr = String::new();
    for addr in response.peers.iter().take(10) {
        match TcpStream::connect_timeout(addr, Duration::from_secs(5)) {
            Ok(mut stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
                match exchange_handshake(&mut stream, &mine) {
                    Ok(theirs) => {
                        assert_eq!(theirs.info_hash, hash);
                        println!("handshake ok with {} extensions={}", addr, theirs.extensions);
                        return;
                    }
                    Err(e) => lasterr = format!("{:?}", e),
                }
            }
            Err(e) => lasterr = e.to_string(),
        }
    }
    panic!("no peer completed a handshake: {}", lasterr);
}
