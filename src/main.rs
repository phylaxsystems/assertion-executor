mod config;

use assertion_executor::db::MemoryDb;

use sled::Db;

use clap::Parser;

use anyhow::Result;

/// The leaf fanout is a sled const that determines how fragmented
/// the data stored on disk is. Higher values mean less disk but slower
/// access times, and lower values mean more disk but faster access times.
/// The default is 1024 and cannot be changed dynamically.
pub const LEAF_FANOUT: usize = 1024;

#[tokio::main]
async fn main() -> Result<()> {
	// Parse CLI args
	let config = config::ExecutorConfig::parse();

	// Open the executor Db
	let db: Db<LEAF_FANOUT> = open_sled!(config);

	let memory_db: MemoryDb<64> = init_mem_db!(config, &db);

	Ok(())
}