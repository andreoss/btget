use bt::bencode::{encode, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;

pub fn bytes_value(s: &[u8]) -> Value {
    Value::Bytes(s.to_vec())
}

pub fn dict(entries: Vec<(&[u8], Value)>) -> Value {
    let mut map = BTreeMap::new();
    for (k, v) in entries {
        map.insert(k.to_vec(), v);
    }
    Value::Dict(map)
}

pub fn piece_hash(content: &[u8]) -> Vec<u8> {
    Sha1::digest(content).to_vec()
}

pub fn single_file_torrent() -> Vec<u8> {
    let info = dict(vec![
        (b"length", Value::Int(12)),
        (b"name", bytes_value(b"demo.bin")),
        (b"piece length", Value::Int(16384)),
        (b"pieces", Value::Bytes(piece_hash(b"hello world\n"))),
    ]);
    let top = dict(vec![
        (b"announce", bytes_value(b"http://127.0.0.1:8080/announce")),
        (b"info", info),
    ]);
    encode(&top)
}

pub fn multi_file_torrent() -> Vec<u8> {
    let files = Value::List(vec![
        dict(vec![
            (b"length", Value::Int(5)),
            (b"path", Value::List(vec![bytes_value(b"a.txt")])),
        ]),
        dict(vec![
            (b"length", Value::Int(4)),
            (b"path", Value::List(vec![bytes_value(b"sub"), bytes_value(b"b.txt")])),
        ]),
    ]);
    let info = dict(vec![
        (b"files", files),
        (b"name", bytes_value(b"demo-dir")),
        (b"piece length", Value::Int(16384)),
        (b"pieces", Value::Bytes(piece_hash(b"alphabeta"))),
    ]);
    let tiers = Value::List(vec![
        Value::List(vec![bytes_value(b"http://127.0.0.1:8080/announce")]),
        Value::List(vec![bytes_value(b"udp://127.0.0.1:8081")]),
    ]);
    let top = dict(vec![
        (b"announce", bytes_value(b"http://127.0.0.1:8080/announce")),
        (b"announce-list", tiers),
        (b"info", info),
    ]);
    encode(&top)
}

#[test]
#[ignore]
fn regen() {
    std::fs::create_dir_all("tests/data").unwrap();
    std::fs::write("tests/data/single.torrent", single_file_torrent()).unwrap();
    std::fs::write("tests/data/multi.torrent", multi_file_torrent()).unwrap();
}
