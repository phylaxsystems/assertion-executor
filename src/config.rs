use clap_derive::Parser;

#[derive(Parser, Debug, Clone, Default)]
#[command(author, version, about, long_about = None)]
pub struct ExecutorConfig {
    /// Path of the database, defaults to `./executor`.
    #[arg(long, default_value = "./executor")]
    pub db_path: String,
    /// Cache size in bytes, defaults to 512mb.
    #[arg(long, default_value = "536870912")]
    pub cache_size: usize,
    /// The percentage of the cache that is dedicated to the
    /// scan-resistant entry cache.
    #[arg(long, default_value = "20")]
    pub entry_cache_percent: u8,
    /// The zstd compression level to use when writing data to disk.
    /// Defaults to 3.
    #[arg(long, default_value = "3")]
    pub zstd_compression_level: i32,
    /// If set, the executor will open an exported reth
    /// database, and load it into the MemoryDb, store
    /// MemoryDb in executor Db and exit.
    #[arg(long, default_value = "None")]
    pub reth_path: Option<String>,
}

/// Initialize the `MemoryDb` from either the executor database or from
/// an exported sled database.
#[macro_export]
macro_rules! init_mem_db {
    (
        $config:expr
    ) => {{
        let mut mem_db = assertion_executor::db::MemoryDb::default();

        if $config.reth_path.is_some() {
            let config = sled::Config::new()
                .path($config.reth_path.clone().unwrap())
                .cache_capacity_bytes($config.cache_size)
                .zstd_compression_level($config.zstd_compression_level)
                .entry_cache_percent($config.entry_cache_percent);

            let exported_sled = sled::Db::open_with_config(&config)?;

            mem_db.load_from_exported_sled(&exported_sled)?;
        }

        mem_db
    }};
}

/// Tries to open the sled database given the configuration and panics if unable.
#[macro_export]
macro_rules! create_shared_db {
    (
        $mem_db:expr,
        $config:expr
    ) => {{
        let sled_config = sled::Config::new()
            .path($config.db_path.clone())
            .cache_capacity_bytes($config.cache_size)
            .zstd_compression_level($config.zstd_compression_level)
            .entry_cache_percent($config.entry_cache_percent);

        match assertion_executor::db::SharedDB::new_with_config($mem_db, sled_config) {
            Ok(db) => db,
            Err(e) => {
                panic!("Failed to open sled database: {}", e);
            }
        }
    }};
}
