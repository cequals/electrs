use rocksdb;

use crate::config::Config;
use crate::util::{bincode, Bytes};
use derivative::Derivative;
use std::path::{Path, PathBuf};
use std::sync::Arc;

static DB_VERSION: u32 = 1;

#[derive(Debug, Eq, PartialEq)]
pub struct DBRow {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

pub struct ScanIterator<'a> {
    prefix: Vec<u8>,
    iter: rocksdb::DBIterator<'a>,
    done: bool,
}

impl<'a> Iterator for ScanIterator<'a> {
    type Item = DBRow;

    fn next(&mut self) -> Option<DBRow> {
        if self.done {
            return None;
        }
        let (key, value) = self.iter.next()?.expect("valid iterator");
        if !key.starts_with(&self.prefix) {
            self.done = true;
            return None;
        }
        Some(DBRow {
            key: key.to_vec(),
            value: value.to_vec(),
        })
    }
}

pub struct ReverseScanIterator<'a> {
    prefix: Vec<u8>,
    iter: rocksdb::DBRawIterator<'a>,
    done: bool,
}

impl<'a> Iterator for ReverseScanIterator<'a> {
    type Item = DBRow;

    fn next(&mut self) -> Option<DBRow> {
        if self.done || !self.iter.valid() {
            return None;
        }

        let key = self.iter.key().unwrap();
        if !key.starts_with(&self.prefix) {
            self.done = true;
            return None;
        }

        let row = DBRow {
            key: key.into(),
            value: self.iter.value().unwrap().into(),
        };

        self.iter.prev();

        Some(row)
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct DB {
    db: rocksdb::DB,
    read_only_mode: bool,
    path: PathBuf,
    #[derivative(Debug = "ignore")]
    db_opts: Arc<rocksdb::Options>,
    config: Config,
}

#[derive(Copy, Clone, Debug)]
pub enum DBFlush {
    Disable,
    Enable,
}

impl DB {
    pub fn open(path: &Path, config: &Config) -> DB {
        debug!("opening DB at {:?}", path);
        let db_opts = Arc::new(build_db_options(config));

        let db = if config.read_only {
            rocksdb::DB::open_for_read_only(&db_opts, path, false)
                .expect("failed to open RocksDB in read-only mode")
        } else {
            rocksdb::DB::open(&db_opts, path).expect("failed to open RocksDB")
        };

        let db = DB {
            db,
            read_only_mode: config.read_only,
            path: path.to_path_buf(),
            db_opts: db_opts.clone(),
            config: config.clone(), // Ensure Config implements Clone
        };
        db.verify_compatibility(config);
        db
    }

    pub fn reopen(&mut self) {
        // Re-open the database to reload the latest data from disk and drop any cached data.
        self.db = if self.read_only_mode {
            rocksdb::DB::open_for_read_only(&self.db_opts, &self.path, false)
                .expect("failed to open RocksDB in read-only mode")
        } else {
            rocksdb::DB::open(&self.db_opts, &self.path).expect("failed to open RocksDB")
        };

        // Just to be safe, verify that the database is still compatible
        self.verify_compatibility(&self.config);
    }

    pub fn full_compaction(&self) {
        if self.read_only_mode {
            return;
        }

        // TODO: make sure this doesn't fail silently
        debug!("starting full compaction on {:?}", self.db);
        self.db.compact_range(None::<&[u8]>, None::<&[u8]>);
        debug!("finished full compaction on {:?}", self.db);
    }

    pub fn enable_auto_compaction(&self) {
        if self.read_only_mode {
            return;
        }

        let opts = [("disable_auto_compactions", "false")];
        self.db.set_options(&opts).unwrap();
    }

    pub fn raw_iterator(&self) -> rocksdb::DBRawIterator {
        self.db.raw_iterator()
    }

    pub fn iter_scan(&self, prefix: &[u8]) -> ScanIterator {
        ScanIterator {
            prefix: prefix.to_vec(),
            iter: self.db.prefix_iterator(prefix),
            done: false,
        }
    }

    pub fn iter_scan_from(&self, prefix: &[u8], start_at: &[u8]) -> ScanIterator {
        let iter = self.db.iterator(rocksdb::IteratorMode::From(
            start_at,
            rocksdb::Direction::Forward,
        ));
        ScanIterator {
            prefix: prefix.to_vec(),
            iter,
            done: false,
        }
    }

    pub fn iter_scan_reverse(&self, prefix: &[u8], prefix_max: &[u8]) -> ReverseScanIterator {
        let mut iter = self.db.raw_iterator();
        iter.seek_for_prev(prefix_max);

        ReverseScanIterator {
            prefix: prefix.to_vec(),
            iter,
            done: false,
        }
    }

    pub fn write(&self, mut rows: Vec<DBRow>, flush: DBFlush) {
        if self.read_only_mode {
            return;
        }

        debug!(
            "writing {} rows to {:?}, flush={:?}",
            rows.len(),
            self.db,
            flush
        );
        rows.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        let mut batch = rocksdb::WriteBatch::default();
        for row in rows {
            #[cfg(not(feature = "oldcpu"))]
            batch.put(&row.key, &row.value);
            #[cfg(feature = "oldcpu")]
            batch.put(&row.key, &row.value).unwrap();
        }
        let do_flush = match flush {
            DBFlush::Enable => true,
            DBFlush::Disable => false,
        };
        let mut opts = rocksdb::WriteOptions::new();
        opts.set_sync(do_flush);
        opts.disable_wal(!do_flush);
        self.db.write_opt(batch, &opts).unwrap();
    }

    pub fn flush(&self) {
        if self.read_only_mode {
            return;
        }

        self.db.flush().unwrap();
    }

    pub fn put(&self, key: &[u8], value: &[u8]) {
        if self.read_only_mode {
            return;
        }

        self.db.put(key, value).unwrap();
    }

    pub fn put_sync(&self, key: &[u8], value: &[u8]) {
        if self.read_only_mode {
            return;
        }

        let mut opts = rocksdb::WriteOptions::new();
        opts.set_sync(true);
        self.db.put_opt(key, value, &opts).unwrap();
    }

    pub fn get(&self, key: &[u8]) -> Option<Bytes> {
        self.db.get(key).unwrap().map(|v| v.to_vec())
    }

    fn verify_compatibility(&self, config: &Config) {
        let mut compatibility_bytes = bincode::serialize_little(&DB_VERSION).unwrap();

        if config.light_mode {
            // append a byte to indicate light_mode is enabled.
            // we're not letting bincode serialize this so that the compatiblity bytes won't change
            // (and require a reindex) when light_mode is disabled. this should be chagned the next
            // time we bump DB_VERSION and require a re-index anyway.
            compatibility_bytes.push(1);
        }

        match self.get(b"V") {
            None => self.put(b"V", &compatibility_bytes),
            Some(ref x) if x != &compatibility_bytes => {
                panic!("Incompatible database found. Please reindex.")
            }
            Some(_) => (),
        }
    }
}

fn build_db_options(config: &Config) -> rocksdb::Options {
    let mut db_opts = rocksdb::Options::default();
    db_opts.create_if_missing(!config.read_only);
    db_opts.set_max_open_files(100_000);
    db_opts.set_compaction_style(rocksdb::DBCompactionStyle::Level);
    db_opts.set_compression_type(rocksdb::DBCompressionType::Snappy);
    db_opts.set_target_file_size_base(1_073_741_824);
    db_opts.set_write_buffer_size(256 << 20);
    db_opts.set_disable_auto_compactions(true);
    db_opts.set_compaction_readahead_size(1 << 20);
    db_opts.increase_parallelism(2);
    db_opts
}
