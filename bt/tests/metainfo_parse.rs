mod fixtures;

use bt::bencode::{decode, encode, Value};
use bt::metainfo::{parse, Error, FileEntry};

fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/data/{}", name)).unwrap()
}

fn with_info<F: FnOnce(&mut std::collections::BTreeMap<Vec<u8>, Value>)>(
    torrent: &[u8],
    mutate: F,
) -> Vec<u8> {
    let mut top = match decode(torrent).unwrap() {
        Value::Dict(map) => map,
        _ => unreachable!(),
    };
    let mut info = match top.remove(b"info".as_slice()).unwrap() {
        Value::Dict(map) => map,
        _ => unreachable!(),
    };
    mutate(&mut info);
    top.insert(b"info".to_vec(), Value::Dict(info));
    encode(&Value::Dict(top))
}

#[test]
fn fixture_matches_generator() {
    assert_eq!(read_fixture("single.torrent"), fixtures::single_file_torrent());
    assert_eq!(read_fixture("multi.torrent"), fixtures::multi_file_torrent());
}

#[test]
fn single_file_parses() {
    let meta = parse(&read_fixture("single.torrent")).unwrap();
    assert_eq!(meta.info_hash.to_hex(), "71af21365358f65e33ed4da954118a5fa643a503");
    assert_eq!(meta.name, "demo.bin");
    assert_eq!(meta.piece_length, 16384);
    assert_eq!(meta.total_length, 12);
    assert_eq!(meta.pieces.len(), 1);
    assert_eq!(meta.files, vec![FileEntry { path: vec![], length: 12 }]);
    assert_eq!(meta.trackers, vec![vec!["http://127.0.0.1:8080/announce".to_string()]]);
}

#[test]
fn multi_file_parses() {
    let meta = parse(&read_fixture("multi.torrent")).unwrap();
    assert_eq!(meta.info_hash.to_hex(), "a08e4714b1bf517d465c9efacff4ee40dc9d2436");
    assert_eq!(meta.name, "demo-dir");
    assert_eq!(meta.total_length, 9);
    assert_eq!(
        meta.files,
        vec![
            FileEntry { path: vec!["a.txt".into()], length: 5 },
            FileEntry { path: vec!["sub".into(), "b.txt".into()], length: 4 },
        ]
    );
    assert_eq!(
        meta.trackers,
        vec![
            vec!["http://127.0.0.1:8080/announce".to_string()],
            vec!["udp://127.0.0.1:8081".to_string()],
        ]
    );
}

#[test]
fn bad_pieces_length_rejected() {
    let torrent = with_info(&fixtures::single_file_torrent(), |info| {
        info.insert(b"pieces".to_vec(), Value::Bytes(vec![0u8; 19]));
    });
    assert_eq!(parse(&torrent), Err(Error::BadPieces));
}

#[test]
fn piece_count_mismatch_rejected() {
    let torrent = with_info(&fixtures::single_file_torrent(), |info| {
        info.insert(b"pieces".to_vec(), Value::Bytes(vec![0u8; 40]));
    });
    assert_eq!(parse(&torrent), Err(Error::PieceCountMismatch));
}

#[test]
fn both_length_and_files_rejected() {
    let torrent = with_info(&fixtures::multi_file_torrent(), |info| {
        info.insert(b"length".to_vec(), Value::Int(9));
    });
    assert_eq!(parse(&torrent), Err(Error::BothLengthAndFiles));
}

#[test]
fn traversal_path_rejected() {
    let torrent = with_info(&fixtures::multi_file_torrent(), |info| {
        let files = Value::List(vec![fixtures::dict(vec![
            (b"length".as_slice(), Value::Int(9)),
            (b"path".as_slice(), Value::List(vec![fixtures::bytes_value(b"..")])),
        ])]);
        info.insert(b"files".to_vec(), files);
    });
    assert_eq!(parse(&torrent), Err(Error::BadPath));
}

#[test]
fn separator_in_name_rejected() {
    let torrent = with_info(&fixtures::single_file_torrent(), |info| {
        info.insert(b"name".to_vec(), fixtures::bytes_value(b"a/b"));
    });
    assert_eq!(parse(&torrent), Err(Error::BadPath));
}

#[test]
fn zero_piece_length_rejected() {
    let torrent = with_info(&fixtures::single_file_torrent(), |info| {
        info.insert(b"piece length".to_vec(), Value::Int(0));
    });
    assert_eq!(parse(&torrent), Err(Error::BadLength));
}

#[test]
fn missing_info_rejected() {
    assert_eq!(parse(b"de"), Err(Error::MissingKey("info")));
}

#[test]
fn top_level_not_dict_rejected() {
    assert_eq!(parse(b"le"), Err(Error::NotADict));
}

#[test]
fn empty_files_rejected() {
    let torrent = with_info(&fixtures::multi_file_torrent(), |info| {
        info.insert(b"files".to_vec(), Value::List(vec![]));
    });
    assert_eq!(parse(&torrent), Err(Error::NoFiles));
}
