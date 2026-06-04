#[test]
#[ignore]
fn live_torrent_hash_matches() {
    let path = std::env::var("LIVE_TORRENT_FILE").unwrap();
    let expected = std::env::var("LIVE_INFO_HASH").unwrap();
    let meta = bt::metainfo::parse(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(meta.info_hash.to_hex(), expected);
    assert!(meta.total_length > 0);
}
