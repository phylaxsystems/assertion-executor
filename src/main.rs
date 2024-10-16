mod config;

use assertion_executor::db::MemoryDb;

use sled::Db;

use clap::Parser;

use anyhow::Result;

pub const LEAF_FANOUT: usize = 1024;

#[tokio::main]
async fn main() -> Result<()> {
	// Parse CLI args
	let config = config::ExecutorConfig::parse();

	// Open the executor Db
	let db: Db<LEAF_FANOUT> = open_sled!(config);

	let memory_db: MemoryDb<64> = init_mem_db!(config);

	Ok(())
}