use bt::engine::{download, EngineConfig, Error as EngineError, Event};
use btget::cli;
use std::io::Write;
use std::time::{Duration, Instant};

#[cfg(target_os = "openbsd")]
fn sandbox() {
    let promises = std::ffi::CString::new("stdio rpath wpath cpath inet dns").unwrap();
    if unsafe { libc::pledge(promises.as_ptr(), std::ptr::null()) } != 0 {
        eprintln!("pledge failed: {}", std::io::Error::last_os_error());
        std::process::exit(2);
    }
}

#[cfg(not(target_os = "openbsd"))]
fn sandbox() {}

fn main() {
    sandbox();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match cli::parse(&args) {
        Ok(cli::Cli::Help) => {
            println!("{}", cli::USAGE);
            0
        }
        Ok(cli::Cli::Run(config)) => run(&config),
        Err(e) => {
            eprintln!("{}", e);
            eprintln!("{}", cli::USAGE);
            2
        }
    };
    std::process::exit(code);
}

fn run(config: &cli::Config) -> i32 {
    let peer_id = generate_peer_id();
    let mut bootstrap_peers: Vec<std::net::SocketAddr> = Vec::new();
    let meta = match &config.input {
        cli::Input::Torrent(path) => match std::fs::read(path) {
            Ok(bytes) => match bt::metainfo::parse(&bytes) {
                Ok(meta) => meta,
                Err(e) => {
                    eprintln!("bad torrent file: {:?}", e);
                    return 3;
                }
            },
            Err(e) => {
                eprintln!("cannot read {}: {}", path.display(), e);
                return 3;
            }
        },
        cli::Input::Magnet(magnet) => match resolve_magnet(magnet, peer_id, config.port) {
            Ok(resolved) => {
                bootstrap_peers = resolved.peers;
                resolved.meta
            }
            Err(code) => return code,
        },
    };
    let engine_config = EngineConfig {
        output_dir: config.output_dir.clone(),
        peer_id,
        port: config.port,
        max_peers: config.max_peers,
        bootstrap_peers,
    };
    let piece_length = meta.piece_length;
    let total_bytes = meta.total_length;
    let piece_size =
        |index: u32| piece_length.min(total_bytes - (index as u64 * piece_length).min(total_bytes));
    let started = Instant::now();
    let mut bytes_done = 0u64;
    let mut peers = 0usize;
    let mut window_start = started;
    let mut window_bytes = 0u64;
    let mut rate = 0f64;
    let mut last_line = Instant::now() - Duration::from_secs(1);
    let result = download(&meta, &engine_config, &mut |event| {
        match event {
            Event::PieceDone { index, have, total } => {
                let size = piece_size(*index);
                bytes_done += size;
                window_bytes += size;
                let window = window_start.elapsed();
                if window >= Duration::from_secs(3) {
                    rate = window_bytes as f64 / window.as_secs_f64();
                    window_start = Instant::now();
                    window_bytes = 0;
                }
                if last_line.elapsed() >= Duration::from_secs(1) || *have == *total {
                    last_line = Instant::now();
                    let pct = 100.0 * bytes_done as f64 / total_bytes as f64;
                    print!(
                        "\r{:>6.2}%  {}/{} pieces  {}/s  peers {}    ",
                        pct,
                        have,
                        total,
                        human_bytes(rate as u64),
                        peers
                    );
                    let _ = std::io::stdout().flush();
                }
            }
            Event::Peers { count } => peers = *count,
            Event::AnnounceFailed { url, reason } => {
                eprintln!("announce failed at {}: {}", url, reason);
            }
            Event::PeerFailed { addr, reason } => {
                eprintln!("peer failed {}: {}", addr, reason);
            }
            Event::Resumed { have, total, bytes } => {
                bytes_done = *bytes;
                println!("resume: {}/{} pieces already verified", have, total);
            }
            Event::Rechecking { bad } => {
                println!();
                println!("recheck: {} pieces failed verification, refetching", bad);
            }
            _ => {}
        }
    });
    match result {
        Ok(()) => {
            println!();
            println!(
                "done: {} in {}s",
                human_bytes(total_bytes),
                started.elapsed().as_secs()
            );
            0
        }
        Err(EngineError::Incomplete) => {
            println!();
            eprintln!("verification failed after retries");
            5
        }
        Err(e) => {
            println!();
            eprintln!("download failed: {:?}", e);
            4
        }
    }
}

struct ResolvedMagnet {
    meta: bt::metainfo::Metainfo,
    peers: Vec<std::net::SocketAddr>,
}

const DEFAULT_DHT_BOOTSTRAP: &str = "router.bittorrent.com:6881,dht.transmissionbt.com:6881";

fn resolve_magnet(
    magnet: &bt::magnet::Magnet,
    peer_id: [u8; 20],
    port: u16,
) -> Result<ResolvedMagnet, i32> {
    use bt::tracker::{AnnounceRequest, Event as TrackerEvent};
    use bt::tracker_set::{announce_url, TrackerSet};

    let tiers: Vec<Vec<String>> = magnet.trackers.iter().map(|t| vec![t.clone()]).collect();
    let mut peers: Vec<std::net::SocketAddr> = Vec::new();
    if !magnet.trackers.is_empty() {
        let mut trackers = TrackerSet::new(tiers.clone());
        let request = AnnounceRequest {
            info_hash: magnet.info_hash,
            peer_id,
            port,
            uploaded: 0,
            downloaded: 0,
            left: 1,
            event: TrackerEvent::Started,
        };
        match trackers.announce(&request, &mut |url, req| {
            announce_url(url, req, Duration::from_secs(20)).map_err(|e| {
                eprintln!("magnet announce failed at {}: {:?}", url, e);
                e
            })
        }) {
            Ok(response) => peers = response.peers,
            Err(_) => {}
        }
    }
    if peers.is_empty() {
        let bootstrap = std::env::var("BTGET_DHT_BOOTSTRAP")
            .unwrap_or_else(|_| DEFAULT_DHT_BOOTSTRAP.to_string());
        let hosts: Vec<&str> = bootstrap.split(',').collect();
        println!("looking up peers in the dht");
        match bt::dht::DhtClient::new(bt::dht::random_node_id(), Duration::from_secs(4)) {
            Ok(client) => match bt::dht::lookup_peers(&client, &hosts, &magnet.info_hash, 60) {
                Ok((found, _, _)) => peers = found,
                Err(e) => {
                    eprintln!("dht lookup failed: {:?}", e);
                    return Err(4);
                }
            },
            Err(e) => {
                eprintln!("dht start failed: {:?}", e);
                return Err(4);
            }
        }
    }
    if peers.is_empty() {
        eprintln!("no peers found for magnet link");
        return Err(4);
    }
    println!("fetching metadata from up to {} peers", peers.len());
    match bt::metadata::fetch_from_peers(magnet.info_hash, peer_id, &peers, Duration::from_secs(20))
    {
        Ok(metadata) => match bt::metainfo::parse_info_dict(&metadata, tiers) {
            Ok(meta) => {
                println!("metadata: {} ({} bytes)", meta.name, metadata.len());
                Ok(ResolvedMagnet { meta, peers })
            }
            Err(e) => {
                eprintln!("fetched metadata does not parse: {:?}", e);
                Err(5)
            }
        },
        Err(e) => {
            eprintln!("metadata fetch failed: {}", e);
            Err(4)
        }
    }
}

fn generate_peer_id() -> [u8; 20] {
    use std::hash::{BuildHasher, Hasher};
    let mut id = *b"-BG0001-000000000000";
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    let salt = hasher.finish();
    for (i, byte) in id[8..].iter_mut().enumerate() {
        let n = (salt >> ((i % 8) * 8)) as u8;
        *byte = b'a' + (n % 26);
    }
    id
}

fn human_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", value, UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit])
    }
}
