use bt::bencode::{decode, encode, Value};
use bt::dht::{
    build_query, lookup_peers, parse_compact_nodes, parse_get_peers_reply, parse_reply, DhtClient,
    Error, NodeEntry, NodeId, RoutingTable, K,
};
use bt::metainfo::InfoHash;
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
fn get_peers_reply_parses_values_and_nodes() {
    let mut r = BTreeMap::new();
    r.insert(b"token".to_vec(), Value::Bytes(b"tok".to_vec()));
    r.insert(
        b"values".to_vec(),
        Value::List(vec![Value::Bytes(vec![127, 0, 0, 1, 0x1a, 0xe1])]),
    );
    let mut nodes = Vec::new();
    nodes.extend_from_slice(&[3u8; 20]);
    nodes.extend_from_slice(&[10, 1, 1, 1, 0x1a, 0xe2]);
    r.insert(b"nodes".to_vec(), Value::Bytes(nodes));
    let reply = parse_get_peers_reply(&r);
    assert_eq!(reply.token, Some(b"tok".to_vec()));
    assert_eq!(reply.peers, vec!["127.0.0.1:6881".parse::<SocketAddr>().unwrap()]);
    assert_eq!(reply.nodes.len(), 1);
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

fn fake_dht_node(peers: Vec<[u8; 6]>, next_node: Option<SocketAddrV4>) -> SocketAddrV4 {
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
                b"get_peers" => {
                    r.insert(b"token".to_vec(), Value::Bytes(b"tok".to_vec()));
                    if peers.is_empty() {
                        if let Some(next) = next_node {
                            let mut nodes = Vec::new();
                            nodes.extend_from_slice(&[5u8; 20]);
                            nodes.extend_from_slice(&next.ip().octets());
                            nodes.extend_from_slice(&next.port().to_be_bytes());
                            r.insert(b"nodes".to_vec(), Value::Bytes(nodes));
                        }
                    } else {
                        r.insert(
                            b"values".to_vec(),
                            Value::List(
                                peers
                                    .iter()
                                    .map(|p| Value::Bytes(p.to_vec()))
                                    .collect(),
                            ),
                        );
                    }
                }
                b"find_node" => {
                    r.insert(b"nodes".to_vec(), Value::Bytes(Vec::new()));
                }
                b"announce_peer" => {
                    match top.get(b"a".as_slice()) {
                        Some(Value::Dict(a)) => {
                            if a.get(b"token".as_slice()) != Some(&Value::Bytes(b"tok".to_vec()))
                            {
                                continue;
                            }
                        }
                        _ => continue,
                    }
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
    let node = fake_dht_node(vec![], None);
    let client = DhtClient::new(own(), Duration::from_secs(3)).unwrap();
    let id = client.ping(SocketAddr::V4(node)).unwrap();
    assert_eq!(id, NodeId([9u8; 20]));
}

#[test]
fn lookup_walks_nodes_to_peers() {
    let leaf = fake_dht_node(vec![[127, 0, 0, 1, 0x1a, 0xe1]], None);
    let root = fake_dht_node(vec![], Some(leaf));
    let client = DhtClient::new(own(), Duration::from_secs(3)).unwrap();
    let (peers, table, tokens) = lookup_peers(
        &client,
        &[&root.to_string()],
        &InfoHash([0xaa; 20]),
        1,
    )
    .unwrap();
    assert_eq!(peers, vec!["127.0.0.1:6881".parse::<SocketAddr>().unwrap()]);
    assert!(table.len() >= 1);
    assert!(!tokens.is_empty());
}

#[test]
fn announce_peer_uses_token() {
    let node = fake_dht_node(vec![[127, 0, 0, 1, 0x1a, 0xe1]], None);
    let client = DhtClient::new(own(), Duration::from_secs(3)).unwrap();
    let reply = client
        .get_peers(SocketAddr::V4(node), &InfoHash([0xaa; 20]))
        .unwrap();
    client
        .announce_peer(
            SocketAddr::V4(node),
            &InfoHash([0xaa; 20]),
            6881,
            &reply.token.unwrap(),
        )
        .unwrap();
}

#[test]
fn unreachable_node_times_out() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    let client = DhtClient::new(own(), Duration::from_millis(300)).unwrap();
    assert_eq!(client.ping(addr), Err(Error::Timeout));
}

#[test]
#[ignore]
fn live_bootstrap_answers_and_finds_peers() {
    let client = DhtClient::new(bt::dht::random_node_id(), Duration::from_secs(5)).unwrap();
    let bootstrap = std::env::var("LIVE_DHT_BOOTSTRAP").unwrap();
    let hash = InfoHash::from_hex(&std::env::var("LIVE_INFO_HASH").unwrap()).unwrap();
    let hosts: Vec<&str> = bootstrap.split(',').collect();
    let mut answered = false;
    for host in &hosts {
        if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(host) {
            for addr in addrs {
                if addr.is_ipv4() {
                    if let Ok(id) = client.ping(addr) {
                        println!("ping ok from {} id {:02x?}", addr, &id.0[..4]);
                        answered = true;
                        break;
                    }
                }
            }
        }
        if answered {
            break;
        }
    }
    assert!(answered, "no bootstrap node answered ping");
    let (peers, table, _tokens) = lookup_peers(&client, &hosts, &hash, 5).unwrap();
    println!("dht lookup: {} peers, {} routing entries", peers.len(), table.len());
    assert!(!peers.is_empty());
}

#[test]
fn transaction_ids_are_not_sequential() {
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = server.local_addr().unwrap();
    let client = DhtClient::new(own(), Duration::from_millis(150)).unwrap();
    let queries = std::thread::spawn(move || {
        let mut seen: Vec<u16> = Vec::new();
        let mut buf = [0u8; 1024];
        for _ in 0..4 {
            let (n, _) = server.recv_from(&mut buf).unwrap();
            match decode(&buf[..n]).unwrap() {
                Value::Dict(top) => match top.get(b"t".as_slice()) {
                    Some(Value::Bytes(t)) if t.len() == 2 => {
                        seen.push(u16::from_be_bytes([t[0], t[1]]))
                    }
                    other => panic!("{:?}", other),
                },
                other => panic!("{:?}", other),
            }
        }
        seen
    });
    for _ in 0..4 {
        let _ = client.ping(addr);
    }
    let seen = queries.join().unwrap();
    assert!(
        seen.windows(2).any(|w| w[1] != w[0].wrapping_add(1)),
        "transaction ids walk a counter: {:?}",
        seen
    );
}
