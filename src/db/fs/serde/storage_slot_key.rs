use crate::primitives::{
    Address,
    U256,
};

use super::{
    Deserialize,
    Serialize,
};
use sled::InlineArray;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct StorageSlotKey {
    pub address: Address,
    pub slot: U256,
}

impl Serialize for StorageSlotKey {
    fn serialize(&self) -> InlineArray {
        let mut bytes = Vec::with_capacity(52);
        bytes.extend_from_slice(&self.address.serialize());
        bytes.extend_from_slice(&self.slot.to_be_bytes::<32>());
        bytes.into()
    }
}

impl Deserialize for StorageSlotKey {
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let address = Address::from_slice(&bytes[0..20]);

        let fixed_bytes: [u8; 32] = bytes.as_ref()[20..52].try_into().ok()?;
        let slot = U256::from_be_bytes(fixed_bytes);
        Some(StorageSlotKey { address, slot })
    }
}

#[test]
fn test_basic_key() {
    use super::{
        Deserialize,
        Serialize,
    };

    for key_tuple in [
        (Address::from([0; 20]), U256::from(1)),
        (Address::from([u8::MAX; 20]), U256::MAX),
    ] {
        let basic_key = StorageSlotKey {
            address: key_tuple.0,
            slot: key_tuple.1,
        };

        let serialized = basic_key.serialize();
        assert_eq!(serialized.len(), 52);

        let basic_key2 = StorageSlotKey::deserialize(serialized).unwrap();

        assert_eq!(basic_key, basic_key2);
    }
}
