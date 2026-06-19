use crate::bencode::{self, Value};
use crate::metainfo::InfoHash;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    None,
    Started,
    Stopped,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceRequest {
    pub info_hash: InfoHash,
    pub peer_id: [u8; 20],
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceResponse {
    pub interval: u32,
    pub peers: Vec<SocketAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnsupportedUrl(String),
    Io(String),
    HttpStatus(u16),
    BadResponse,
    Failure(String),
}

const MAX_RESPONSE: u64 = 1 << 20;

pub fn url_is_printable_ascii(url: &str) -> bool {
    url.bytes().all(|b| b > 0x20 && b < 0x7f)
}

pub fn build_announce_url(base: &str, req: &AnnounceRequest) -> Result<String, Error> {
    if !base.starts_with("http://") || !url_is_printable_ascii(base) {
        return Err(Error::UnsupportedUrl(base.to_string()));
    }
    let separator = if base.contains('?') { '&' } else { '?' };
    Ok(format!(
        "{}{}info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}&compact=1&numwant=50{}",
        base,
        separator,
        escape_bytes(&req.info_hash.0),
        escape_bytes(&req.peer_id),
        req.port,
        req.uploaded,
        req.downloaded,
        req.left,
        match req.event {
            Event::None => "",
            Event::Started => "&event=started",
            Event::Stopped => "&event=stopped",
            Event::Completed => "&event=completed",
        }
    ))
}

pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub fn parse_response(body: &[u8]) -> Result<AnnounceResponse, Error> {
    let top = match bencode::decode(body) {
        Ok(Value::Dict(map)) => map,
        _ => return Err(Error::BadResponse),
    };
    if let Some(Value::Bytes(reason)) = top.get(b"failure reason".as_slice()) {
        return Err(Error::Failure(String::from_utf8_lossy(reason).to_string()));
    }
    let interval = match top.get(b"interval".as_slice()) {
        Some(Value::Int(n)) if *n > 0 => *n as u32,
        Some(_) => return Err(Error::BadResponse),
        None => 1800,
    };
    let mut peers = match top.get(b"peers".as_slice()) {
        Some(Value::Bytes(raw)) => parse_compact_peers(raw)?,
        Some(Value::List(items)) => parse_dict_peers(items),
        Some(_) => return Err(Error::BadResponse),
        None => Vec::new(),
    };
    match top.get(b"peers6".as_slice()) {
        Some(Value::Bytes(raw)) => peers.extend(parse_compact_peers6(raw)?),
        Some(_) => return Err(Error::BadResponse),
        None => {
            if !top.contains_key(b"peers".as_slice()) {
                return Err(Error::BadResponse);
            }
        }
    }
    Ok(AnnounceResponse { interval, peers })
}

fn parse_compact_peers(raw: &[u8]) -> Result<Vec<SocketAddr>, Error> {
    if raw.len() % 6 != 0 {
        return Err(Error::BadResponse);
    }
    Ok(raw
        .chunks(6)
        .map(|c| {
            let ip = Ipv4Addr::new(c[0], c[1], c[2], c[3]);
            let port = u16::from_be_bytes([c[4], c[5]]);
            SocketAddr::V4(SocketAddrV4::new(ip, port))
        })
        .filter(|addr| addr.port() != 0)
        .collect())
}

fn parse_compact_peers6(raw: &[u8]) -> Result<Vec<SocketAddr>, Error> {
    if raw.len() % 18 != 0 {
        return Err(Error::BadResponse);
    }
    Ok(raw
        .chunks(18)
        .map(|c| {
            let mut ip = [0u8; 16];
            ip.copy_from_slice(&c[0..16]);
            let port = u16::from_be_bytes([c[16], c[17]]);
            SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::from(ip)), port)
        })
        .filter(|addr| addr.port() != 0)
        .collect())
}

pub fn split_authority(authority: &str) -> Option<(&str, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        match tail.strip_prefix(':') {
            Some(port) => Some((host, port.parse().ok()?)),
            None if tail.is_empty() => Some((host, 80)),
            None => None,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => Some((host, port.parse().ok()?)),
            None => Some((authority, 80)),
        }
    }
}

fn parse_dict_peers(items: &[Value]) -> Vec<SocketAddr> {
    items
        .iter()
        .filter_map(|item| match item {
            Value::Dict(map) => {
                let ip = match map.get(b"ip".as_slice()) {
                    Some(Value::Bytes(b)) => std::str::from_utf8(b).ok()?.parse().ok()?,
                    _ => return None,
                };
                let port = match map.get(b"port".as_slice()) {
                    Some(Value::Int(n)) if *n > 0 && *n <= 65535 => *n as u16,
                    _ => return None,
                };
                Some(SocketAddr::new(ip, port))
            }
            _ => None,
        })
        .collect()
}

pub fn http_announce(
    base: &str,
    req: &AnnounceRequest,
    timeout: Duration,
) -> Result<AnnounceResponse, Error> {
    let url = build_announce_url(base, req)?;
    let without_scheme = &url["http://".len()..];
    let (authority, path) = match without_scheme.find('/') {
        Some(i) => (&without_scheme[..i], &without_scheme[i..]),
        None => (without_scheme, "/"),
    };
    let (host, port) =
        split_authority(authority).ok_or_else(|| Error::UnsupportedUrl(base.to_string()))?;
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::Io(e.to_string()))?
        .next()
        .ok_or_else(|| Error::Io("no address".to_string()))?;
    let mut stream =
        TcpStream::connect_timeout(&addr, timeout).map_err(|e| Error::Io(e.to_string()))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| Error::Io(e.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| Error::Io(e.to_string()))?;
    let request = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        path, authority
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| Error::Io(e.to_string()))?;
    let mut raw = Vec::new();
    (&mut stream)
        .take(MAX_RESPONSE)
        .read_to_end(&mut raw)
        .map_err(|e| Error::Io(e.to_string()))?;
    let (status, body) = split_http_response(&raw)?;
    if !(200..300).contains(&status) {
        return Err(Error::HttpStatus(status));
    }
    parse_response(body)
}

fn split_http_response(raw: &[u8]) -> Result<(u16, &[u8]), Error> {
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(Error::BadResponse)?;
    let head = std::str::from_utf8(&raw[..header_end]).map_err(|_| Error::BadResponse)?;
    let status_line = head.lines().next().ok_or(Error::BadResponse)?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or(Error::BadResponse)?;
    Ok((status, &raw[header_end + 4..]))
}
