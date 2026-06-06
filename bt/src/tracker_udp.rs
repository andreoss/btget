use crate::tracker::{AnnounceRequest, AnnounceResponse, Error, Event};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::time::Duration;

const PROTOCOL_ID: u64 = 0x0417_2710_1980;
const ACTION_CONNECT: u32 = 0;
const ACTION_ANNOUNCE: u32 = 1;
const ACTION_ERROR: u32 = 3;

pub fn build_connect(transaction_id: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&PROTOCOL_ID.to_be_bytes());
    out[8..12].copy_from_slice(&ACTION_CONNECT.to_be_bytes());
    out[12..16].copy_from_slice(&transaction_id.to_be_bytes());
    out
}

pub fn parse_connect(raw: &[u8], transaction_id: u32) -> Result<u64, Error> {
    check_header(raw, ACTION_CONNECT, transaction_id)?;
    if raw.len() < 16 {
        return Err(Error::BadResponse);
    }
    Ok(u64::from_be_bytes(raw[8..16].try_into().unwrap()))
}

pub fn build_announce(
    connection_id: u64,
    transaction_id: u32,
    request: &AnnounceRequest,
) -> [u8; 98] {
    let mut out = [0u8; 98];
    out[0..8].copy_from_slice(&connection_id.to_be_bytes());
    out[8..12].copy_from_slice(&ACTION_ANNOUNCE.to_be_bytes());
    out[12..16].copy_from_slice(&transaction_id.to_be_bytes());
    out[16..36].copy_from_slice(&request.info_hash.0);
    out[36..56].copy_from_slice(&request.peer_id);
    out[56..64].copy_from_slice(&request.downloaded.to_be_bytes());
    out[64..72].copy_from_slice(&request.left.to_be_bytes());
    out[72..80].copy_from_slice(&request.uploaded.to_be_bytes());
    let event: u32 = match request.event {
        Event::None => 0,
        Event::Completed => 1,
        Event::Started => 2,
        Event::Stopped => 3,
    };
    out[80..84].copy_from_slice(&event.to_be_bytes());
    out[88..92].copy_from_slice(&0u32.to_be_bytes());
    out[92..96].copy_from_slice(&(-1i32).to_be_bytes());
    out[96..98].copy_from_slice(&request.port.to_be_bytes());
    out
}

pub fn parse_announce(raw: &[u8], transaction_id: u32) -> Result<AnnounceResponse, Error> {
    check_header(raw, ACTION_ANNOUNCE, transaction_id)?;
    if raw.len() < 20 {
        return Err(Error::BadResponse);
    }
    let interval = u32::from_be_bytes(raw[8..12].try_into().unwrap());
    let peers_raw = &raw[20..];
    if peers_raw.len() % 6 != 0 {
        return Err(Error::BadResponse);
    }
    let peers = peers_raw
        .chunks(6)
        .map(|c| {
            SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(c[0], c[1], c[2], c[3]),
                u16::from_be_bytes([c[4], c[5]]),
            ))
        })
        .filter(|a| a.port() != 0)
        .collect();
    Ok(AnnounceResponse { interval, peers })
}

fn check_header(raw: &[u8], expected_action: u32, transaction_id: u32) -> Result<(), Error> {
    if raw.len() < 8 {
        return Err(Error::BadResponse);
    }
    let action = u32::from_be_bytes(raw[0..4].try_into().unwrap());
    let txid = u32::from_be_bytes(raw[4..8].try_into().unwrap());
    if txid != transaction_id {
        return Err(Error::BadResponse);
    }
    if action == ACTION_ERROR {
        return Err(Error::Failure(
            String::from_utf8_lossy(&raw[8..]).to_string(),
        ));
    }
    if action != expected_action {
        return Err(Error::BadResponse);
    }
    Ok(())
}

pub fn udp_tracker_addr(url: &str) -> Result<(String, u16), Error> {
    let rest = url
        .strip_prefix("udp://")
        .ok_or_else(|| Error::UnsupportedUrl(url.to_string()))?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or_else(|| Error::UnsupportedUrl(url.to_string()))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| Error::UnsupportedUrl(url.to_string()))?;
    Ok((host.to_string(), port))
}

fn next_transaction_id() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(std::process::id() as u64);
    hasher.finish() as u32
}

pub fn udp_announce(
    url: &str,
    request: &AnnounceRequest,
    attempt_timeouts: &[Duration],
) -> Result<AnnounceResponse, Error> {
    let (host, port) = udp_tracker_addr(url)?;
    let target = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| Error::Io(e.to_string()))?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| Error::Io("no address".to_string()))?;
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| Error::Io(e.to_string()))?;
    socket
        .connect(target)
        .map_err(|e| Error::Io(e.to_string()))?;
    let mut last = Error::Io("no attempts".to_string());
    for timeout in attempt_timeouts {
        socket
            .set_read_timeout(Some(*timeout))
            .map_err(|e| Error::Io(e.to_string()))?;
        match udp_round(&socket, request) {
            Ok(response) => return Ok(response),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn udp_round(socket: &UdpSocket, request: &AnnounceRequest) -> Result<AnnounceResponse, Error> {
    let txid = next_transaction_id();
    socket
        .send(&build_connect(txid))
        .map_err(|e| Error::Io(e.to_string()))?;
    let mut buf = [0u8; 2048];
    let n = socket.recv(&mut buf).map_err(|e| Error::Io(e.to_string()))?;
    let connection_id = parse_connect(&buf[..n], txid)?;
    let txid = next_transaction_id();
    socket
        .send(&build_announce(connection_id, txid, request))
        .map_err(|e| Error::Io(e.to_string()))?;
    let n = socket.recv(&mut buf).map_err(|e| Error::Io(e.to_string()))?;
    parse_announce(&buf[..n], txid)
}
