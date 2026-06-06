mod fixtures;

use bt::metainfo::parse;
use bt::verify::{piece_digest, verify_piece};

#[test]
fn digest_matches_known_vector() {
    let digest = piece_digest(b"hello world\n");
    let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    assert_eq!(hex, "22596363b3de40b06f981fb85d82312e8c0ed511");
}

#[test]
fn fixture_piece_verifies() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    assert!(verify_piece(b"hello world\n", &meta.pieces[0]));
}

#[test]
fn corrupt_piece_rejected() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    let mut data = b"hello world\n".to_vec();
    data[0] ^= 0x01;
    assert!(!verify_piece(&data, &meta.pieces[0]));
}

#[test]
fn truncated_piece_rejected() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    assert!(!verify_piece(b"hello world", &meta.pieces[0]));
}

#[test]
fn padded_piece_rejected() {
    let meta = parse(&fixtures::single_file_torrent()).unwrap();
    assert!(!verify_piece(b"hello world\n\0", &meta.pieces[0]));
}
