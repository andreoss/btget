use bt::extensions::{
    build_ext_handshake, build_metadata_data, build_metadata_reject, build_metadata_request,
    parse_ext_handshake, parse_metadata_message, Error, ExtHandshake, MetadataMessage,
};
use bt::peer::{encode_message, read_message, Message};
use std::io::Cursor;

#[test]
fn extended_message_round_trips() {
    let message = Message::Extended {
        ext: 3,
        payload: b"d8:msg_typei0e5:piecei0ee".to_vec(),
    };
    let framed = encode_message(&message);
    assert_eq!(framed[4], 20);
    assert_eq!(framed[5], 3);
    let mut cursor = Cursor::new(framed);
    assert_eq!(read_message(&mut cursor).unwrap(), message);
}

#[test]
fn ext_handshake_round_trips() {
    let payload = build_ext_handshake(Some(31235));
    let parsed = parse_ext_handshake(&payload).unwrap();
    assert_eq!(
        parsed,
        ExtHandshake {
            ut_metadata: Some(1),
            metadata_size: Some(31235),
        }
    );
}

#[test]
fn foreign_ext_handshake_parses() {
    let payload = b"d1:md9:other_exti9e11:ut_metadatai3ee13:metadata_sizei31235e1:v4:demoe";
    let parsed = parse_ext_handshake(payload).unwrap();
    assert_eq!(parsed.ut_metadata, Some(3));
    assert_eq!(parsed.metadata_size, Some(31235));
}

#[test]
fn handshake_without_metadata_support() {
    let payload = b"d1:mdee";
    let parsed = parse_ext_handshake(payload).unwrap();
    assert_eq!(parsed.ut_metadata, None);
    assert_eq!(parsed.metadata_size, None);
}

#[test]
fn garbage_handshake_rejected() {
    assert!(parse_ext_handshake(b"i5e").is_err());
    assert!(parse_ext_handshake(b"x").is_err());
}

#[test]
fn metadata_request_round_trips() {
    let payload = build_metadata_request(2);
    assert_eq!(
        parse_metadata_message(&payload).unwrap(),
        MetadataMessage::Request { piece: 2 }
    );
}

#[test]
fn metadata_data_carries_trailing_bytes() {
    let payload = build_metadata_data(1, 20000, b"raw-info-bytes");
    match parse_metadata_message(&payload).unwrap() {
        MetadataMessage::Data {
            piece,
            total_size,
            data,
        } => {
            assert_eq!(piece, 1);
            assert_eq!(total_size, Some(20000));
            assert_eq!(data, b"raw-info-bytes");
        }
        other => panic!("{:?}", other),
    }
}

#[test]
fn metadata_reject_round_trips() {
    let payload = build_metadata_reject(7);
    assert_eq!(
        parse_metadata_message(&payload).unwrap(),
        MetadataMessage::Reject { piece: 7 }
    );
}

#[test]
fn unknown_msg_type_rejected() {
    assert_eq!(
        parse_metadata_message(b"d8:msg_typei9e5:piecei0ee"),
        Err(Error::BadPayload)
    );
}

#[test]
fn missing_piece_rejected() {
    assert_eq!(
        parse_metadata_message(b"d8:msg_typei0ee"),
        Err(Error::BadPayload)
    );
}
