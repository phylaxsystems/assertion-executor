use super::Ext;

use crate::primitives::{
    AccountInfo,
    Address,
};
use revm::InMemoryDB;

impl Ext<InMemoryDB> for InMemoryDB {
    fn insert_account_info(&mut self, address: Address, account_info: AccountInfo) {
        self.insert_account_info(address, account_info);
    }
}
