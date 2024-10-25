mod config;

use clap::Parser;

use anyhow::Result;

/// The leaf fanout is a sled const that determines how fragmented
/// the data stored on disk is. Higher values mean less disk but slower
/// access times, and lower values mean more disk but faster access times.
/// The default is 1024 and cannot be changed dynamically.
pub const LEAF_FANOUT: usize = 1024;

fn main() -> Result<()> {
    // Parse CLI args
    let config = config::ExecutorConfig::parse();

    // Initialize the memory db
    let memory_db = init_mem_db!(config);

    // Create the `SharedDb`
    let _shared_db = create_shared_db!(memory_db, config);

    Ok(())
}
