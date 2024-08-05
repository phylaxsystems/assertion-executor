mod in_mem_db;

use crate::primitives::{AccountInfo, Address};
use revm::Database;

pub trait Ext<D: Database> {
    /// Insert account info but not override storage
    fn insert_account_info(&mut self, address: Address, account_info: AccountInfo);

    /// Inserts the account's code into the cache.
    ///
    /// Accounts objects and code are stored separately in the cache, this will take the code from the account and instead map it to the code hash.
    ///
    /// Note: This will not insert into the underlying external database.
    fn insert_contract(&mut self, account: &mut AccountInfo);
}
