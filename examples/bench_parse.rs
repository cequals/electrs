extern crate bitcoin;
extern crate electrs;

#[macro_use]
extern crate log;
#[macro_use]
extern crate error_chain;

use electrs::{bulk::Parser,
              config::Config,
              daemon::Daemon,
              errors::*,
              metrics::Metrics,
              signal::Waiter,
              store::{DBStore, StoreOptions, WriteStore}};

use error_chain::ChainedError;

fn run(config: Config) -> Result<()> {
    let signal = Waiter::new();
    let metrics = Metrics::new(config.monitoring_addr);
    metrics.start();

    let daemon = Daemon::new(config.network_type, &metrics)?;
    let store = DBStore::open("./test-db", StoreOptions { bulk_import: true });

    let chan = Parser::new(&daemon, &store, &metrics)?.start();
    store.load(chan, &signal)
}

fn main() {
    if let Err(e) = run(Config::from_args()) {
        error!("{}", e.display_chain());
    }
}