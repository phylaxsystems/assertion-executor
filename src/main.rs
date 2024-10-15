mod config;
use clap::Parser;

use assertion_executor::*;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
	// Parse CLI args
	let config = config::ExecutorConfig::parse();

	Ok(())
}