use bt::metainfo::InfoHash;
use bt::peer::{decode_handshake, encode_handshake, exchange_handshake, Error, Handshake};

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
