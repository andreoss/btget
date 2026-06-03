use bt::magnet::{parse, Error};

const HEX: &str = "0123456789abcdef0123456789abcdef01234567";
const BASE32: &str = "AERUKZ4JVPG66AJDIVTYTK6N54ASGRLH";

#[test]
fn hex_magnet_parses() {
    let uri = format!(
        "magnet:?xt=urn:btih:{}&dn=My+File%20Name&tr=http%3A%2F%2F127.0.0.1%3A8080%2Fannounce&tr=udp%3A%2F%2F127.0.0.1%3A8081",
        HEX
    );
    let magnet = parse(&uri).unwrap();
    assert_eq!(magnet.info_hash.to_hex(), HEX);
    assert_eq!(magnet.display_name.as_deref(), Some("My File Name"));
    assert_eq!(
        magnet.trackers,
        vec![
            "http://127.0.0.1:8080/announce".to_string(),
            "udp://127.0.0.1:8081".to_string(),
        ]
    );
}

#[test]
fn uppercase_hex_parses() {
    let uri = format!("magnet:?xt=urn:btih:{}", HEX.to_uppercase());
    assert_eq!(parse(&uri).unwrap().info_hash.to_hex(), HEX);
}

#[test]
fn base32_equals_hex() {
    let uri = format!("magnet:?xt=urn:btih:{}", BASE32);
    assert_eq!(parse(&uri).unwrap().info_hash.to_hex(), HEX);
}

#[test]
fn bare_magnet_parses() {
    let magnet = parse(&format!("magnet:?xt=urn:btih:{}", HEX)).unwrap();
    assert_eq!(magnet.display_name, None);
    assert!(magnet.trackers.is_empty());
}

#[test]
fn non_magnet_rejected() {
    assert_eq!(parse("http://example.invalid/x.torrent"), Err(Error::NotMagnet));
}

#[test]
fn missing_topic_rejected() {
    assert_eq!(parse("magnet:?dn=x"), Err(Error::MissingTopic));
}

#[test]
fn v2_topic_rejected() {
    let uri = "magnet:?xt=urn:btmh:1220aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert_eq!(parse(uri), Err(Error::UnsupportedTopic));
}

#[test]
fn wrong_length_hash_rejected() {
    assert_eq!(parse("magnet:?xt=urn:btih:abcdef"), Err(Error::BadHash));
}

#[test]
fn non_hex_hash_rejected() {
    let bad = "z".repeat(40);
    assert_eq!(parse(&format!("magnet:?xt=urn:btih:{}", bad)), Err(Error::BadHash));
}

#[test]
fn bad_base32_rejected() {
    let bad = "1".repeat(32);
    assert_eq!(parse(&format!("magnet:?xt=urn:btih:{}", bad)), Err(Error::BadHash));
}

#[test]
fn bad_percent_encoding_rejected() {
    let uri = format!("magnet:?xt=urn:btih:{}&dn=%zz", HEX);
    assert_eq!(parse(&uri), Err(Error::BadEncoding));
}

#[test]
fn truncated_percent_encoding_rejected() {
    let uri = format!("magnet:?xt=urn:btih:{}&dn=%4", HEX);
    assert_eq!(parse(&uri), Err(Error::BadEncoding));
}
