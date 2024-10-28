use super::{
    deserialize_unsized_vec,
    serialize_unsized_vec,
    Deserialize,
    Serialize,
};
use crate::primitives::{
    Account,
    Address,
    BlockChanges,
    EvmState,
    B256,
};
use sled::InlineArray;

impl Serialize for BlockChanges {
    fn serialize(&self) -> InlineArray {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.block_num.to_be_bytes());
        bytes.extend_from_slice(&self.block_hash.serialize());
        bytes.extend_from_slice(&self.state_changes.serialize());
        bytes.into()
    }
}
impl Deserialize for BlockChanges {
    // Deserialization function
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let mut bytes = bytes.as_ref();
        let block_num = u64::from_be_bytes(bytes.get(..8)?.try_into().ok()?);
        bytes = &bytes[8..];
        let block_hash = B256::from_slice(bytes.get(..32)?);
        bytes = &bytes[32..];
        let state_changes = EvmState::deserialize(InlineArray::from(bytes))?;

        Some(Self {
            block_num,
            block_hash,
            state_changes,
        })
    }
}

impl Serialize for EvmState {
    fn serialize(&self) -> InlineArray {
        let ser_items = self
            .iter()
            .map(|(address, account)| {
                let mut bytes = Vec::new();
                bytes.extend_from_slice(&address.serialize());
                bytes.extend_from_slice(&account.serialize());
                bytes.into()
            })
            .collect();

        serialize_unsized_vec(ser_items)
    }
}

impl Deserialize for EvmState {
    // Deserialization function
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let mut state_changes = EvmState::default();
        for item in deserialize_unsized_vec(bytes)? {
            let address = Address::from_slice(&item[..20]);
            let account = Account::deserialize(InlineArray::from(item[20..].to_vec()))?;
            state_changes.insert(address, account);
        }
        Some(state_changes)
    }
}

#[test]
fn test_block_changes() {
    use crate::primitives::{
        AccountInfo,
        AccountStatus,
        Bytecode,
        EvmStorageSlot,
        U256,
    };

    for block_changes in [
        BlockChanges {
            block_num: 1,
            block_hash: B256::from([0; 32]),
            state_changes: EvmState::default(),
        },
        BlockChanges {
            block_num: u64::MAX,
            block_hash: B256::from([u8::MAX; 32]),
            state_changes: [
                (
                    Address::from([0; 20]),
                    Account {
                        info: AccountInfo {
                            nonce: 1,
                            balance: U256::from(2),
                            code_hash: B256::from([3; 32]),
                            code: Some(Bytecode::new_raw([8; 32].into())),
                        },
                        status: AccountStatus::Touched,
                        storage: [(
                            U256::from_be_bytes([9; 32]),
                            EvmStorageSlot {
                                original_value: U256::from(10),
                                present_value: U256::from(11),
                                is_cold: true,
                            },
                        )]
                        .iter()
                        .cloned()
                        .collect(),
                    },
                ),
                (
                    Address::from([1; 20]),
                    Account {
                        info: AccountInfo {
                            nonce: 4,
                            balance: U256::from(5),
                            code_hash: B256::from([6; 32]),
                            code: None,
                        },
                        status: AccountStatus::Created,
                        storage: [(
                            U256::from_be_bytes([12; 32]),
                            EvmStorageSlot {
                                original_value: U256::from(13),
                                present_value: U256::from(14),
                                is_cold: false,
                            },
                        )]
                        .iter()
                        .cloned()
                        .collect(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        },
    ] {
        let serialized = block_changes.serialize();
        let block_changes2 = BlockChanges::deserialize(serialized).unwrap();
        assert_eq!(block_changes, block_changes2);
    }
}
