use clap::{App, Arg};
use std::env::home_dir;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use stderrlog;

use daemon::Network;

use errors::*;

fn read_cookie(daemon_dir: &Path) -> Result<String> {
    let mut path = daemon_dir.to_path_buf();
    path.push(".cookie");
    let contents = String::from_utf8(
        fs::read(&path).chain_err(|| format!("failed to read cookie from {:?}", path))?
    ).chain_err(|| "invalid cookie string")?;
    Ok(contents.trim().to_owned())
}

#[derive(Debug)]
pub struct Config {
    pub log: stderrlog::StdErrLog,
    pub network_type: Network,         // bitcoind JSONRPC endpoint
    pub db_path: PathBuf,              // RocksDB directory path
    pub daemon_dir: PathBuf,           // Bitcoind data directory
    pub daemon_rpc_addr: SocketAddr,   // for connecting Bitcoind JSONRPC
    pub cookie: String,                // for bitcoind JSONRPC authentication ("USER:PASSWORD")
    pub electrum_rpc_addr: SocketAddr, // for serving Electrum clients
    pub monitoring_addr: SocketAddr,   // for Prometheus monitoring
}

impl Config {
    pub fn from_args() -> Config {
        let m = App::new("Electrum Rust Server")
            .version(crate_version!())
            .arg(
                Arg::with_name("verbosity")
                    .short("v")
                    .multiple(true)
                    .help("Increase logging verbosity"),
            )
            .arg(
                Arg::with_name("timestamp")
                    .long("timestamp")
                    .help("Prepend log lines with a timestamp"),
            )
            .arg(
                Arg::with_name("db_dir")
                    .long("db-dir")
                    .help("Directory to store index database (deafult: ./db/)")
                    .takes_value(true),
            )
            .arg(
                Arg::with_name("daemon_dir")
                    .long("daemon-dir")
                    .help("Data directory of Bitcoind (default: ~/.bitcoin/)")
                    .takes_value(true),
            )
            .arg(
                Arg::with_name("cookie")
                    .long("cookie")
                    .help("JSONRPC authentication cookie ('USER:PASSWORD', default: read from ~/.bitcoin/.cookie)")
                    .takes_value(true),
            )
            .arg(
                Arg::with_name("network")
                    .help("Select Bitcoin network type ('mainnet', 'testnet' or 'regtest')")
                    .takes_value(true),
            )
            .arg(
                Arg::with_name("electrum_rpc_addr")
                    .long("electrum-rpc-addr")
                    .help("Electrum server JSONRPC 'addr:port' to listen on (default: '127.0.0.1:50001' for mainnet, '127.0.0.1:60001' for testnet and '127.0.0.1:60401' for regtest)")
                    .takes_value(true),
            )
            .arg(
                Arg::with_name("daemon_rpc_addr")
                    .long("daemon-rpc-addr")
                    .help("Bitcoin daemon JSONRPC 'addr:port' to connect (default: 127.0.0.1:8332 for mainnet, 127.0.0.1:18332 for testnet and 127.0.0.1:18443 for regtest)")
                    .takes_value(true),
            )
            .arg(
                Arg::with_name("monitoring_addr")
                    .long("monitoring-addr")
                    .help("Prometheus monitoring 'addr:port' to listen on (default: 127.0.0.1:42024)")
                    .takes_value(true),
            )
            .get_matches();

        let network_name = m.value_of("network").unwrap_or("mainnet");
        let network_type = match network_name {
            "mainnet" => Network::Mainnet,
            "testnet" => Network::Testnet,
            "regtest" => Network::Regtest,
            _ => panic!("unsupported Bitcoin network: {:?}", network_name),
        };
        let db_dir = Path::new(m.value_of("db_dir").unwrap_or("./db"));
        let db_path = db_dir.join(network_name);

        let default_daemon_port = match network_type {
            Network::Mainnet => 8332,
            Network::Testnet => 18332,
            Network::Regtest => 18443,
        };
        let default_electrum_port = match network_type {
            Network::Mainnet => 50001,
            Network::Testnet => 60001,
            Network::Regtest => 60401,
        };

        let daemon_rpc_addr: SocketAddr = m.value_of("daemon_rpc_addr")
            .unwrap_or(&format!("127.0.0.1:{}", default_daemon_port))
            .parse()
            .expect("invalid Bitcoind RPC address");
        let electrum_rpc_addr: SocketAddr = m.value_of("electrum_rpc_addr")
            .unwrap_or(&format!("127.0.0.1:{}", default_electrum_port))
            .parse()
            .expect("invalid Electrum RPC address");
        let monitoring_addr: SocketAddr = m.value_of("monitoring_addr")
            .unwrap_or("127.0.0.1:42024")
            .parse()
            .expect("invalid Prometheus monitoring address");

        let mut daemon_dir = m.value_of("daemon_dir")
            .map(|p| PathBuf::from(p))
            .unwrap_or_else(|| {
                let mut default_dir = home_dir().expect("no homedir");
                default_dir.push(".bitcoin");
                default_dir
            });
        match network_type {
            Network::Mainnet => (),
            Network::Testnet => daemon_dir.push("testnet3"),
            Network::Regtest => daemon_dir.push("regtest"),
        }
        let cookie = m.value_of("cookie")
            .map(|s| s.to_owned())
            .unwrap_or_else(|| read_cookie(&daemon_dir).unwrap());

        let mut log = stderrlog::new();
        log.verbosity(m.occurrences_of("verbosity") as usize);
        log.timestamp(if m.is_present("timestamp") {
            stderrlog::Timestamp::Millisecond
        } else {
            stderrlog::Timestamp::Off
        });
        log.init().expect("logging initialization failed");
        let config = Config {
            log,
            network_type,
            db_path,
            daemon_dir,
            daemon_rpc_addr,
            cookie,
            electrum_rpc_addr,
            monitoring_addr,
        };
        eprintln!("{:?}", config);
        config
    }
}
