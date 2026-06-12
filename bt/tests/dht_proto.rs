use bt::bencode::{decode, encode, Value};
use bt::dht::{
    build_query, parse_compact_nodes, parse_reply, DhtClient, Error, NodeEntry, NodeId,
    RoutingTable, K,
};
use std::collections::BTreeMap;
use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

fn own() -> NodeId {
    NodeId([7u8; 20])
}

#[test]
fn query_layout_is_krpc() {
    let packet = build_query(b"aa", &own(), "ping", vec![]);
    match decode(&packet).unwrap() {
        Value::Dict(top) => {
            assert_eq!(top.get(b"t".as_slice()), Some(&Value::Bytes(b"aa".to_vec())));
            assert_eq!(top.get(b"y".as_slice()), Some(&Value::Bytes(b"q".to_vec())));
            assert_eq!(top.get(b"q".as_slice()), Some(&Value::Bytes(b"ping".to_vec())));
            match top.get(b"a".as_slice()) {
                Some(Value::Dict(a)) => assert_eq!(
                    a.get(b"id".as_slice()),
                    Some(&Value::Bytes(vec![7u8; 20]))
                ),
                other => panic!("{:?}", other),
            }
        }
        other => panic!("{:?}", other),
    }
}

fn reply_bytes(txid: &[u8], r: BTreeMap<Vec<u8>, Value>) -> Vec<u8> {
    let mut top = BTreeMap::new();
    top.insert(b"t".to_vec(), Value::Bytes(txid.to_vec()));
    top.insert(b"y".to_vec(), Value::Bytes(b"r".to_vec()));
    top.insert(b"r".to_vec(), Value::Dict(r));
    encode(&Value::Dict(top))
}

#[test]
fn reply_parses_and_checks_txid() {
    let mut r = BTreeMap::new();
    r.insert(b"id".to_vec(), Value::Bytes(vec![1u8; 20]));
    let raw = reply_bytes(b"xy", r);
    assert!(parse_reply(&raw, b"xy").is_ok());
    assert_eq!(parse_reply(&raw, b"zz"), Err(Error::BadResponse));
}

#[test]
fn error_reply_surfaces() {
    let mut top = BTreeMap::new();
    top.insert(b"t".to_vec(), Value::Bytes(b"xy".to_vec()));
    top.insert(b"y".to_vec(), Value::Bytes(b"e".to_vec()));
    top.insert(
        b"e".to_vec(),
        Value::List(vec![Value::Int(201), Value::Bytes(b"Generic".to_vec())]),
    );
    let raw = encode(&Value::Dict(top));
    assert_eq!(
        parse_reply(&raw, b"xy"),
        Err(Error::Remote("201 Generic".to_string()))
    );
}

#[test]
fn compact_nodes_parse() {
    let mut raw = Vec::new();
    raw.extend_from_slice(&[1u8; 20]);
    raw.extend_from_slice(&[127, 0, 0, 1, 0x1a, 0xe1]);
    raw.extend_from_slice(&[2u8; 20]);
    raw.extend_from_slice(&[10, 0, 0, 1, 0, 0]);
    let nodes = parse_compact_nodes(&raw);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].id, NodeId([1u8; 20]));
    assert_eq!(nodes[0].addr, "127.0.0.1:6881".parse().unwrap());
}

#[test]
fn bucket_index_by_leading_zeroes() {
    let a = NodeId([0u8; 20]);
    let mut close = [0u8; 20];
    close[19] = 1;
    assert_eq!(a.bucket_index(&NodeId(close)), 159);
    let mut far = [0u8; 20];
    far[0] = 0x80;
    assert_eq!(a.bucket_index(&NodeId(far)), 0);
}

#[test]
fn routing_table_dedups_and_caps() {
    let mut table = RoutingTable::new(own());
    let mut inserted = 0;
    for i in 0..(K as u8 + 4) {
        let mut id = [0x80u8; 20];
        id[19] = i;
        let entry = NodeEntry {
            id: NodeId(id),
            addr: SocketAddrV4::new([10, 0, 0, i].into(), 6881),
        };
        if table.insert(entry) {
            inserted += 1;
        }
        table.insert(entry);
    }
    assert_eq!(inserted, K);
    assert_eq!(table.len(), K);
}

#[test]
fn closest_sorts_by_distance() {
    let mut table = RoutingTable::new(own());
    for i in 1..=4u8 {
        let mut id = [0u8; 20];
        id[0] = i;
        table.insert(NodeEntry {
            id: NodeId(id),
            addr: SocketAddrV4::new([10, 0, 0, i].into(), 6881),
        });
    }
    let mut target = [0u8; 20];
    target[0] = 3;
    let closest = table.closest(&NodeId(target), 2);
    assert_eq!(closest[0].id.0[0], 3);
    assert_eq!(closest[1].id.0[0], 2);
}

fn fake_dht_node() -> SocketAddrV4 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = match socket.local_addr().unwrap() {
        SocketAddr::V4(v4) => v4,
        _ => unreachable!(),
    };
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let (n, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => return,
            };
            let top = match decode(&buf[..n]) {
                Ok(Value::Dict(map)) => map,
                _ => continue,
            };
            let txid = match top.get(b"t".as_slice()) {
                Some(Value::Bytes(t)) => t.clone(),
                _ => continue,
            };
            let query = match top.get(b"q".as_slice()) {
                Some(Value::Bytes(q)) => q.clone(),
                _ => continue,
            };
            let mut r = BTreeMap::new();
            r.insert(b"id".to_vec(), Value::Bytes(vec![9u8; 20]));
            match query.as_slice() {
                b"ping" => {}
                b"find_node" => {
                    let mut nodes = Vec::new();
                    nodes.extend_from_slice(&[5u8; 20]);
                    nodes.extend_from_slice(&[10, 0, 0, 5, 0x1a, 0xe1]);
                    r.insert(b"nodes".to_vec(), Value::Bytes(nodes));
                }
                _ => continue,
            }
            let mut reply = BTreeMap::new();
            reply.insert(b"t".to_vec(), Value::Bytes(txid));
            reply.insert(b"y".to_vec(), Value::Bytes(b"r".to_vec()));
            reply.insert(b"r".to_vec(), Value::Dict(r));
            let _ = socket.send_to(&encode(&Value::Dict(reply)), from);
        }
    });
    addr
}

#[test]
fn ping_round_trip_against_local_node() {
    let node = fake_dht_node();
    let client = DhtClient::new(own(), Duration::from_secs(3)).unwrap();
    let id = client.ping(SocketAddr::V4(node)).unwrap();
    assert_eq!(id, NodeId([9u8; 20]));
}

#[test]
fn find_node_returns_compact_nodes() {
    let node = fake_dht_node();
    let client = DhtClient::new(own(), Duration::from_secs(3)).unwrap();
    let nodes = client
        .find_node(SocketAddr::V4(node), &NodeId([1u8; 20]))
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].addr, "10.0.0.5:6881".parse().unwrap());
}

#[test]
fn unreachable_node_times_out() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    let client = DhtClient::new(own(), Duration::from_millis(300)).unwrap();
    assert_eq!(client.ping(addr), Err(Error::Timeout));
}
