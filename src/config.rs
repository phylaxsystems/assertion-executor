use clap_derive::Parser;

#[derive(Parser, Debug, Clone, Default)]
#[command(author, version, about, long_about = None)]
pub struct SharedDbConfig {
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
    /// The number of blocks to retain in memory.
    #[arg(long, default_value = "256")]
    pub blocks_to_retain: usize,
}

/// Initialize the `MemoryDb` struct. Will load an exported sled
/// database if the option is present.
#[macro_export]
macro_rules! init_mem_db {
    (
        $config:expr
    ) => {{
        use assertion_executor::db::MemoryDb;

        let mut mem_db: MemoryDb<64> = MemoryDb::default();

        if $config.reth_path.is_some() {
            println!("reth path present, importing reth data...");
            let config = sled::Config::new()
                .path($config.reth_path.clone().unwrap())
                .cache_capacity_bytes($config.cache_size)
                .zstd_compression_level($config.zstd_compression_level)
                .entry_cache_percent($config.entry_cache_percent);

            let exported_sled = sled::Db::open_with_config(&config)?;

            mem_db.load_from_exported_sled(&exported_sled)?;
            println!("Done~!");
        }

        mem_db
    }};
}

/// Tries to open the sled database given the configuration and panics if unable.
#[macro_export]
macro_rules! create_shared_db {
    (
        $mem_db:expr,
        $config:expr,
        $provider:expr
    ) => {{
        let sled_config = sled::Config::new()
            .path($config.db_path.clone())
            .cache_capacity_bytes($config.cache_size)
            .zstd_compression_level($config.zstd_compression_level)
            .entry_cache_percent($config.entry_cache_percent);

        let db = match assertion_executor::db::SharedDB::new_with_config(
            $mem_db,
            sled_config,
            $provider,
            Default::default(),
        ) {
            Ok(db) => db,
            Err(e) => {
                panic!("Failed to open sled database: {}", e);
            }
        };

        if $config.reth_path.is_some() {
            println!("Exporting memory db to sled db...");
            db.commit_mem_db_to_fs().await?;
            println!("Done~!");
            println!("Exiting...");
            return Ok(());
        }

        println!("Created shared db!");

        db
    }};
}
