mod config;

use assertion_executor::db::MemoryDb;

use clap::Parser;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
	// Parse CLI args
	let config = config::ExecutorConfig::parse();

	let memory_db: MemoryDb<64> = init_mem_db!(config);

	Ok(())
}