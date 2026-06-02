use bt::bencode::{decode, Value};
use std::collections::BTreeMap;

fn bytes(s: &str) -> Value {
    Value::Bytes(s.as_bytes().to_vec())
}

fn dict(entries: Vec<(&str, Value)>) -> Value {
    let mut map = BTreeMap::new();
    for (k, v) in entries {
        map.insert(k.as_bytes().to_vec(), v);
    }
    Value::Dict(map)
}

const VALID: &[&[u8]] = &[
    b"i0e",
    b"i42e",
    b"i-3e",
    b"i9223372036854775807e",
    b"i-9223372036854775808e",
    b"0:",
    b"4:spam",
    b"le",
    b"li1ei2ei3ee",
    b"l4:spami7ee",
    b"de",
    b"d3:cow3:moo4:spam4:eggse",
    b"d4:spaml1:a1:bee",
    b"d1:ad1:bd1:cleeee",
];

const INVALID: &[&[u8]] = &[
    b"",
    b"x",
    b"i03e",
    b"i-0e",
    b"i-03e",
    b"ie",
    b"i-e",
    b"i2",
    b"i1x2e",
    b"i9223372036854775808e",
    b"2:a",
    b"-1:a",
    b"01:a",
    b"4spam",
    b"l",
    b"li1e",
    b"d",
    b"d3:key",
    b"d3:keyi1e",
    b"di1ei2ee",
    b"d1:bi1e1:ai2ee",
    b"d1:ai1e1:ai2ee",
    b"i1ee",
    b"4:spamx",
    b"lee",
];

#[test]
fn valid_corpus_decodes() {
    for sample in VALID {
        assert!(decode(sample).is_ok(), "{:?}", sample);
    }
}

#[test]
fn invalid_corpus_rejected() {
    for sample in INVALID {
        assert!(decode(sample).is_err(), "{:?}", sample);
    }
}

#[test]
fn decoded_shapes() {
    assert_eq!(decode(b"i-3e").unwrap(), Value::Int(-3));
    assert_eq!(decode(b"4:spam").unwrap(), bytes("spam"));
    assert_eq!(
        decode(b"li1e4:spame").unwrap(),
        Value::List(vec![Value::Int(1), bytes("spam")])
    );
    assert_eq!(
        decode(b"d3:cow3:moo4:spam4:eggse").unwrap(),
        dict(vec![("cow", bytes("moo")), ("spam", bytes("eggs"))])
    );
}

#[test]
fn deep_nesting_rejected() {
    let mut sample = Vec::new();
    sample.extend(std::iter::repeat(b'l').take(10_000));
    sample.extend(std::iter::repeat(b'e').take(10_000));
    assert!(decode(&sample).is_err());
}

#[test]
fn binary_strings_survive() {
    let raw = [0u8, 255, 10, 58];
    let mut sample = b"4:".to_vec();
    sample.extend_from_slice(&raw);
    assert_eq!(decode(&sample).unwrap(), Value::Bytes(raw.to_vec()));
}
