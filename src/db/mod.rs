mod in_mem_db;

use crate::primitives::{
    AccountInfo,
    Address,
};
use revm::Database;

pub trait Ext<D: Database> {
    /// Insert account info into database.
    fn insert_account_info(&mut self, address: Address, account_info: AccountInfo);
}
