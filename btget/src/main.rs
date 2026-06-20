use bt::engine::{download, EngineConfig, Error as EngineError, Event};
use btget::cli;
use std::io::Write;
use std::time::{Duration, Instant};

#[cfg(target_os = "openbsd")]
fn pledge_or_die(promises: &str) {
    let promises = std::ffi::CString::new(promises).unwrap();
    if unsafe { libc::pledge(promises.as_ptr(), std::ptr::null()) } != 0 {
        eprintln!("pledge failed: {}", std::io::Error::last_os_error());
        std::process::exit(2);
    }
}

#[cfg(target_os = "openbsd")]
fn sandbox() {
    pledge_or_die("stdio rpath wpath cpath inet dns unveil");
}

#[cfg(target_os = "openbsd")]
fn confine(output_dir: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    let dir = std::ffi::CString::new(output_dir.as_os_str().as_bytes()).unwrap();
    let rwc = std::ffi::CString::new("rwc").unwrap();
    if unsafe { libc::unveil(dir.as_ptr(), rwc.as_ptr()) } != 0 {
        eprintln!(
            "unveil {} failed: {}",
            output_dir.display(),
            std::io::Error::last_os_error()
        );
        std::process::exit(2);
    }
    let read = std::ffi::CString::new("r").unwrap();
    for path in ["/etc/resolv.conf", "/etc/hosts"] {
        let path = std::ffi::CString::new(path).unwrap();
        unsafe { libc::unveil(path.as_ptr(), read.as_ptr()) };
    }
    if unsafe { libc::unveil(std::ptr::null(), std::ptr::null()) } != 0 {
        eprintln!("unveil lock failed: {}", std::io::Error::last_os_error());
        std::process::exit(2);
    }
    pledge_or_die("stdio rpath wpath cpath inet dns");
}

#[cfg(not(target_os = "openbsd"))]
fn sandbox() {}

#[cfg(not(target_os = "openbsd"))]
fn confine(_output_dir: &std::path::Path) {}

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

struct Logger {
    started: Instant,
    verbose: bool,
    quiet: bool,
}

impl Logger {
    fn stamp(&self) -> String {
        format!("[{:>7.1}s]", self.started.elapsed().as_secs_f64())
    }

    fn info(&self, message: &str) {
        if !self.quiet {
            eprintln!("{} {}", self.stamp(), message);
        }
    }

    fn debug(&self, message: &str) {
        if self.verbose && !self.quiet {
            eprintln!("{} {}", self.stamp(), message);
        }
    }
}

fn run(config: &cli::Config) -> i32 {
    let peer_id = generate_peer_id();
    let log = Logger {
        started: Instant::now(),
        verbose: config.verbose,
        quiet: config.quiet,
    };
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
        cli::Input::Magnet(magnet) => match resolve_magnet(magnet, peer_id, config.port, &log) {
            Ok(resolved) => {
                bootstrap_peers = resolved.peers;
                resolved.meta
            }
            Err(code) => return code,
        },
    };
    log.info(&format!(
        "torrent: {} ({}, {} pieces, {} trackers)",
        meta.name,
        human_bytes(meta.total_length),
        meta.pieces.len(),
        meta.trackers.iter().map(|t| t.len()).sum::<usize>()
    ));
    if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
        eprintln!("cannot create {}: {}", config.output_dir.display(), e);
        return 3;
    }
    confine(&config.output_dir);
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
    let started = log.started;
    let quiet = config.quiet;
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
                if !quiet && (last_line.elapsed() >= Duration::from_secs(1) || *have == *total) {
                    last_line = Instant::now();
                    let pct = 100.0 * bytes_done as f64 / total_bytes as f64;
                    print!(
                        "\r{:>6.2}%  {}/{} pieces  {}/{}  {}/s  eta {}  peers {}    ",
                        pct,
                        have,
                        total,
                        human_bytes(bytes_done),
                        human_bytes(total_bytes),
                        human_bytes(rate as u64),
                        human_eta(total_bytes.saturating_sub(bytes_done), rate),
                        peers
                    );
                    let _ = std::io::stdout().flush();
                }
            }
            Event::Peers { count } => {
                if *count != peers {
                    log.debug(&format!("peers: {} connected", count));
                }
                peers = *count;
            }
            Event::Announced { peers: found } => {
                log.info(&format!("announce: {} peers", found));
            }
            Event::AnnounceFailed { url, reason } => {
                log.info(&format!("announce failed at {}: {}", url, reason));
            }
            Event::Connected { addr } => {
                log.debug(&format!("peer connected {}", addr));
            }
            Event::PeerFailed { addr, reason } => {
                log.debug(&format!("peer failed {}: {}", addr, reason));
            }
            Event::Resumed { have, total, bytes } => {
                bytes_done = *bytes;
                log.info(&format!(
                    "resume: {}/{} pieces already verified ({})",
                    have,
                    total,
                    human_bytes(*bytes)
                ));
            }
            Event::Rechecking { bad } => {
                if !quiet {
                    println!();
                }
                log.info(&format!(
                    "recheck: {} pieces failed verification, refetching",
                    bad
                ));
            }
            Event::Complete => {}
        }
    });
    match result {
        Ok(()) => {
            if !quiet {
                println!();
            }
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            println!(
                "done: {} in {}s ({}/s)",
                human_bytes(total_bytes),
                elapsed as u64,
                human_bytes((total_bytes as f64 / elapsed) as u64)
            );
            0
        }
        Err(EngineError::Incomplete) => {
            if !quiet {
                println!();
            }
            eprintln!("verification failed after retries");
            5
        }
        Err(e) => {
            if !quiet {
                println!();
            }
            eprintln!("download failed: {:?}", e);
            4
        }
    }
}

fn human_eta(remaining: u64, rate: f64) -> String {
    if rate < 1.0 {
        return "-".to_string();
    }
    let seconds = (remaining as f64 / rate) as u64;
    if seconds >= 3600 {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}s", seconds)
    }
}

struct ResolvedMagnet {
    meta: bt::metainfo::Metainfo,
    peers: Vec<std::net::SocketAddr>,
}

const DEFAULT_DHT_BOOTSTRAP: &str = "router.bittorrent.com:6881,dht.transmissionbt.com:6881";
const METADATA_ROUNDS: u32 = 3;
const METADATA_RETRY_PAUSE: Duration = Duration::from_secs(5);

fn announce_peers(
    magnet: &bt::magnet::Magnet,
    peer_id: [u8; 20],
    port: u16,
    log: &Logger,
) -> Vec<std::net::SocketAddr> {
    use bt::tracker::{AnnounceRequest, Event as TrackerEvent};
    use bt::tracker_set::{announce_url, TrackerSet};

    if magnet.trackers.is_empty() {
        return Vec::new();
    }
    let tiers: Vec<Vec<String>> = magnet.trackers.iter().map(|t| vec![t.clone()]).collect();
    let mut trackers = TrackerSet::new(tiers);
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
        Ok(response) => {
            log.info(&format!("magnet announce: {} peers", response.peers.len()));
            response.peers
        }
        Err(_) => Vec::new(),
    }
}

fn dht_peers(magnet: &bt::magnet::Magnet, log: &Logger) -> Vec<std::net::SocketAddr> {
    let bootstrap =
        std::env::var("BTGET_DHT_BOOTSTRAP").unwrap_or_else(|_| DEFAULT_DHT_BOOTSTRAP.to_string());
    let hosts: Vec<&str> = bootstrap.split(',').collect();
    log.info("looking up peers in the dht");
    match bt::dht::DhtClient::new(bt::dht::random_node_id(), Duration::from_secs(4)) {
        Ok(client) => match bt::dht::lookup_peers(&client, &hosts, &magnet.info_hash, 60) {
            Ok((found, _, _)) => {
                log.info(&format!("dht lookup: {} peers", found.len()));
                found
            }
            Err(e) => {
                log.info(&format!("dht lookup failed: {:?}", e));
                Vec::new()
            }
        },
        Err(e) => {
            log.info(&format!("dht start failed: {:?}", e));
            Vec::new()
        }
    }
}

fn resolve_magnet(
    magnet: &bt::magnet::Magnet,
    peer_id: [u8; 20],
    port: u16,
    log: &Logger,
) -> Result<ResolvedMagnet, i32> {
    let tiers: Vec<Vec<String>> = magnet.trackers.iter().map(|t| vec![t.clone()]).collect();
    let mut known: Vec<std::net::SocketAddr> = Vec::new();
    let mut tried: Vec<std::net::SocketAddr> = Vec::new();
    let mut failure = String::new();
    for round in 0..METADATA_ROUNDS {
        if round > 0 {
            std::thread::sleep(METADATA_RETRY_PAUSE);
        }
        for addr in announce_peers(magnet, peer_id, port, log) {
            if !known.contains(&addr) {
                known.push(addr);
            }
        }
        if known.iter().all(|addr| tried.contains(addr)) {
            for addr in dht_peers(magnet, log) {
                if !known.contains(&addr) {
                    known.push(addr);
                }
            }
        }
        let mut batch: Vec<std::net::SocketAddr> = known
            .iter()
            .copied()
            .filter(|addr| !tried.contains(addr))
            .collect();
        if batch.is_empty() {
            batch = known.clone();
        }
        if batch.is_empty() {
            break;
        }
        for addr in &batch {
            if !tried.contains(addr) {
                tried.push(*addr);
            }
        }
        log.info(&format!("fetching metadata from up to {} peers", batch.len()));
        match bt::metadata::fetch_from_peers(
            magnet.info_hash,
            peer_id,
            &batch,
            Duration::from_secs(20),
        ) {
            Ok(metadata) => match bt::metainfo::parse_info_dict(&metadata, tiers.clone()) {
                Ok(meta) => {
                    log.info(&format!("metadata: {} ({} bytes)", meta.name, metadata.len()));
                    return Ok(ResolvedMagnet { meta, peers: known });
                }
                Err(e) => {
                    eprintln!("fetched metadata does not parse: {:?}", e);
                    return Err(5);
                }
            },
            Err(e) => {
                failure = e;
                log.info(&format!("metadata fetch failed: {}", failure));
            }
        }
    }
    if tried.is_empty() {
        eprintln!("no peers found for magnet link");
    } else {
        eprintln!("metadata fetch failed on every peer: {}", failure);
    }
    Err(4)
}

fn generate_peer_id() -> [u8; 20] {
    let mut id = *b"-BG0001-000000000000";
    let mut salt = [0u8; 12];
    bt::random::Source::new().fill(&mut salt);
    for (byte, value) in id[8..].iter_mut().zip(salt) {
        *byte = b'a' + (value % 26);
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
