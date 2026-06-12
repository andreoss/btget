use crate::bencode::{self, Value};
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

pub const K: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub [u8; 20]);

impl NodeId {
    pub fn distance(&self, other: &NodeId) -> [u8; 20] {
        let mut out = [0u8; 20];
        for i in 0..20 {
            out[i] = self.0[i] ^ other.0[i];
        }
        out
    }

    pub fn bucket_index(&self, other: &NodeId) -> usize {
        let distance = self.distance(other);
        for (byte_index, byte) in distance.iter().enumerate() {
            if *byte != 0 {
                return byte_index * 8 + byte.leading_zeros() as usize;
            }
        }
        159
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeEntry {
    pub id: NodeId,
    pub addr: SocketAddrV4,
}

#[derive(Debug, Clone)]
pub struct RoutingTable {
    own: NodeId,
    buckets: Vec<Vec<NodeEntry>>,
}

impl RoutingTable {
    pub fn new(own: NodeId) -> Self {
        RoutingTable {
            own,
            buckets: vec![Vec::new(); 160],
        }
    }

    pub fn insert(&mut self, entry: NodeEntry) -> bool {
        if entry.id == self.own {
            return false;
        }
        let bucket = &mut self.buckets[self.own.bucket_index(&entry.id)];
        if bucket.iter().any(|n| n.id == entry.id) {
            return false;
        }
        if bucket.len() >= K {
            return false;
        }
        bucket.push(entry);
        true
    }

    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn closest(&self, target: &NodeId, count: usize) -> Vec<NodeEntry> {
        let mut all: Vec<NodeEntry> = self.buckets.iter().flatten().copied().collect();
        all.sort_by_key(|entry| entry.id.distance(target));
        all.truncate(count);
        all
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Io(String),
    Timeout,
    BadResponse,
    Remote(String),
}


pub fn build_query(txid: &[u8], own: &NodeId, name: &str, extra: Vec<(&[u8], Value)>) -> Vec<u8> {
    let mut a = BTreeMap::new();
    a.insert(b"id".to_vec(), Value::Bytes(own.0.to_vec()));
    for (key, value) in extra {
        a.insert(key.to_vec(), value);
    }
    let mut top = BTreeMap::new();
    top.insert(b"t".to_vec(), Value::Bytes(txid.to_vec()));
    top.insert(b"y".to_vec(), Value::Bytes(b"q".to_vec()));
    top.insert(b"q".to_vec(), Value::Bytes(name.as_bytes().to_vec()));
    top.insert(b"a".to_vec(), Value::Dict(a));
    bencode::encode(&Value::Dict(top))
}

pub fn parse_reply(raw: &[u8], txid: &[u8]) -> Result<BTreeMap<Vec<u8>, Value>, Error> {
    let top = match bencode::decode(raw) {
        Ok(Value::Dict(map)) => map,
        _ => return Err(Error::BadResponse),
    };
    match top.get(b"t".as_slice()) {
        Some(Value::Bytes(t)) if t == txid => {}
        _ => return Err(Error::BadResponse),
    }
    match top.get(b"y".as_slice()) {
        Some(Value::Bytes(y)) if y == b"r" => {}
        Some(Value::Bytes(y)) if y == b"e" => {
            let message = match top.get(b"e".as_slice()) {
                Some(Value::List(items)) => items
                    .iter()
                    .filter_map(|i| match i {
                        Value::Bytes(b) => Some(String::from_utf8_lossy(b).to_string()),
                        Value::Int(n) => Some(n.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => "unknown".to_string(),
            };
            return Err(Error::Remote(message));
        }
        _ => return Err(Error::BadResponse),
    }
    match top.get(b"r".as_slice()) {
        Some(Value::Dict(r)) => Ok(r.clone()),
        _ => Err(Error::BadResponse),
    }
}

pub fn parse_compact_nodes(raw: &[u8]) -> Vec<NodeEntry> {
    raw.chunks(26)
        .filter(|c| c.len() == 26)
        .map(|c| {
            let mut id = [0u8; 20];
            id.copy_from_slice(&c[0..20]);
            NodeEntry {
                id: NodeId(id),
                addr: SocketAddrV4::new(
                    Ipv4Addr::new(c[20], c[21], c[22], c[23]),
                    u16::from_be_bytes([c[24], c[25]]),
                ),
            }
        })
        .filter(|n| n.addr.port() != 0)
        .collect()
}


pub struct DhtClient {
    socket: UdpSocket,
    own: NodeId,
    txid_counter: std::cell::Cell<u16>,
}

impl DhtClient {
    pub fn new(own: NodeId, timeout: Duration) -> Result<Self, Error> {
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| Error::Io(e.to_string()))?;
        socket
            .set_read_timeout(Some(timeout))
            .map_err(|e| Error::Io(e.to_string()))?;
        Ok(DhtClient {
            socket,
            own,
            txid_counter: std::cell::Cell::new(seed_txid()),
        })
    }

    pub fn own_id(&self) -> NodeId {
        self.own
    }

    fn round(
        &self,
        addr: SocketAddr,
        name: &str,
        extra: Vec<(&[u8], Value)>,
    ) -> Result<BTreeMap<Vec<u8>, Value>, Error> {
        let counter = self.txid_counter.get().wrapping_add(1);
        self.txid_counter.set(counter);
        let txid = counter.to_be_bytes();
        let packet = build_query(&txid, &self.own, name, extra);
        self.socket
            .send_to(&packet, addr)
            .map_err(|e| Error::Io(e.to_string()))?;
        let mut buf = [0u8; 4096];
        let deadline_attempts = 3;
        for _ in 0..deadline_attempts {
            let (n, from) = match self.socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut
                    {
                        return Err(Error::Timeout);
                    }
                    return Err(Error::Io(e.to_string()));
                }
            };
            if from != addr {
                continue;
            }
            return parse_reply(&buf[..n], &txid);
        }
        Err(Error::Timeout)
    }

    pub fn ping(&self, addr: SocketAddr) -> Result<NodeId, Error> {
        let r = self.round(addr, "ping", vec![])?;
        reply_id(&r)
    }

    pub fn find_node(&self, addr: SocketAddr, target: &NodeId) -> Result<Vec<NodeEntry>, Error> {
        let r = self.round(
            addr,
            "find_node",
            vec![(b"target".as_slice(), Value::Bytes(target.0.to_vec()))],
        )?;
        match r.get(b"nodes".as_slice()) {
            Some(Value::Bytes(raw)) => Ok(parse_compact_nodes(raw)),
            _ => Err(Error::BadResponse),
        }
    }


}

fn reply_id(r: &BTreeMap<Vec<u8>, Value>) -> Result<NodeId, Error> {
    match r.get(b"id".as_slice()) {
        Some(Value::Bytes(id)) if id.len() == 20 => {
            let mut out = [0u8; 20];
            out.copy_from_slice(id);
            Ok(NodeId(out))
        }
        _ => Err(Error::BadResponse),
    }
}

fn seed_txid() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.finish() as u16
}

pub fn random_node_id() -> NodeId {
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; 20];
    for chunk in out.chunks_mut(8) {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u8(chunk.len() as u8);
        let bytes = hasher.finish().to_be_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    out[0] &= 0x7f;
    NodeId(out)
}

