use bt::bencode::{encode, Value};
use bt::peer::{
    decode_handshake, encode_handshake, read_message, write_message, Handshake, Message,
};
use bt::verify::piece_digest;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::Command;

const CONTENT: &[u8] = b"hello world\n";

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../scratch/tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn torrent_bytes(announce: &str) -> Vec<u8> {
    let mut info = BTreeMap::new();
    info.insert(b"length".to_vec(), Value::Int(CONTENT.len() as i64));
    info.insert(b"name".to_vec(), Value::Bytes(b"demo.bin".to_vec()));
    info.insert(b"piece length".to_vec(), Value::Int(16384));
    info.insert(
        b"pieces".to_vec(),
        Value::Bytes(piece_digest(CONTENT).to_vec()),
    );
    let mut top = BTreeMap::new();
    top.insert(b"announce".to_vec(), Value::Bytes(announce.as_bytes().to_vec()));
    top.insert(b"info".to_vec(), Value::Dict(info));
    encode(&Value::Dict(top))
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

fn start_seeder(metadata: Vec<u8>) -> SocketAddr {
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
            if write_message(&mut stream, &Message::Bitfield(vec![0x80])).is_err() {
                continue;
            }
            let metadata = metadata.clone();
            loop {
                match read_message(&mut stream) {
                    Ok(Message::Interested) => {
                        if write_message(&mut stream, &Message::Unchoke).is_err() {
                            break;
                        }
                    }
                    Ok(Message::Request { index, begin, length }) => {
                        let start = begin as usize;
                        let data = CONTENT[start..start + length as usize].to_vec();
                        if write_message(&mut stream, &Message::Piece { index, begin, data })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Message::Extended { ext, payload }) => {
                        use bt::extensions::{
                            build_metadata_data, parse_metadata_message, MetadataMessage,
                        };
                        if ext == 0 {
                            let mut m = BTreeMap::new();
                            m.insert(b"ut_metadata".to_vec(), Value::Int(3));
                            let mut top = BTreeMap::new();
                            top.insert(b"m".to_vec(), Value::Dict(m));
                            top.insert(
                                b"metadata_size".to_vec(),
                                Value::Int(metadata.len() as i64),
                            );
                            let reply = Message::Extended {
                                ext: 0,
                                payload: encode(&Value::Dict(top)),
                            };
                            if write_message(&mut stream, &reply).is_err() {
                                break;
                            }
                        } else if let Ok(MetadataMessage::Request { piece }) =
                            parse_metadata_message(&payload)
                        {
                            let reply = Message::Extended {
                                ext: 1,
                                payload: build_metadata_data(
                                    piece,
                                    metadata.len() as u64,
                                    &metadata,
                                ),
                            };
                            if write_message(&mut stream, &reply).is_err() {
                                break;
                            }
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

fn info_bytes(torrent: &[u8]) -> Vec<u8> {
    match bt::bencode::decode(torrent).unwrap() {
        Value::Dict(top) => encode(top.get(b"info".as_slice()).unwrap()),
        _ => unreachable!(),
    }
}

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_btget"))
}

#[test]
fn no_arguments_is_usage_error() {
    let output = binary().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage"));
}

#[test]
fn missing_torrent_file_is_input_error() {
    let dir = scratch("cli-missing");
    let output = binary()
        .arg(dir.join("absent.torrent"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn garbage_torrent_file_is_input_error() {
    let dir = scratch("cli-garbage");
    let path = dir.join("junk.torrent");
    std::fs::write(&path, b"not bencode at all").unwrap();
    let output = binary().arg(&path).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("bad torrent file"));
}

#[test]
fn trackerless_magnet_with_dead_dht_is_network_error() {
    let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let output = binary()
        .arg("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567")
        .env("BTGET_DHT_BOOTSTRAP", dead.local_addr().unwrap().to_string())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no peers"));
}

#[test]
fn magnet_download_succeeds() {
    let dir = scratch("cli-magnet");
    let torrent = torrent_bytes("http://127.0.0.1:1/announce");
    let meta = bt::metainfo::parse(&torrent).unwrap();
    let seeder = start_seeder(info_bytes(&torrent));
    let tracker = start_tracker(vec![seeder]);
    let uri = format!(
        "magnet:?xt=urn:btih:{}&tr=http%3A%2F%2F{}%2Fannounce",
        meta.info_hash.to_hex(),
        tracker.to_string().replace(':', "%3A")
    );
    let output = binary().arg(&uri).arg("-o").arg(&dir).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("metadata: demo.bin"), "stdout: {}", stdout);
    assert_eq!(std::fs::read(dir.join("demo.bin")).unwrap(), CONTENT);
}

#[test]
fn empty_swarm_is_network_error() {
    let dir = scratch("cli-empty");
    let tracker = start_tracker(vec![]);
    let path = dir.join("demo.torrent");
    std::fs::write(&path, torrent_bytes(&format!("http://{}/announce", tracker))).unwrap();
    let output = binary()
        .arg(&path)
        .arg("-o")
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stderr).contains("download failed"));
}

#[test]
fn full_download_succeeds_with_progress() {
    let dir = scratch("cli-full");
    let seeder = start_seeder(info_bytes(&torrent_bytes("http://127.0.0.1:1/announce")));
    let tracker = start_tracker(vec![seeder]);
    let path = dir.join("demo.torrent");
    std::fs::write(&path, torrent_bytes(&format!("http://{}/announce", tracker))).unwrap();
    let output = binary()
        .arg(&path)
        .arg("-o")
        .arg(&dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("done:"), "stdout: {}", stdout);
    assert!(stdout.contains("pieces"), "stdout: {}", stdout);
    assert_eq!(std::fs::read(dir.join("demo.bin")).unwrap(), CONTENT);
}
