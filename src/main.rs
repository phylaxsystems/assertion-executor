mod config;

use assertion_executor::db::MemoryDb;

use sled::Db;

use clap::Parser;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
	// Parse CLI args
	let config = config::ExecutorConfig::parse();

	// Open the executor Db
	let db: Db<1024> = open_sled!(config);

	let memory_db: MemoryDb<64> = init_mem_db!(config);

	Ok(())
}