use super::{
    Deserialize,
    Serialize,
};
use crate::primitives::{
    Account,
    AccountInfo,
    AccountStatus,
    EvmStorage,
};
use sled::InlineArray;

impl Serialize for Account {
    fn serialize(&self) -> InlineArray {
        let mut bytes = Vec::new();
        let status_bytes = self.status.bits().to_be_bytes();

        let info_bytes = self.info.serialize();
        let storage_ptr = 8 + status_bytes.len() + info_bytes.len();

        bytes.extend_from_slice(&storage_ptr.to_be_bytes());
        bytes.extend_from_slice(&status_bytes);
        bytes.extend_from_slice(&info_bytes);
        bytes.extend_from_slice(&self.storage.serialize());
        bytes.into()
    }
}
impl Deserialize for Account {
    // Deserialization function
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let bytes = bytes.as_ref();

        let storage_ptr = u64::from_be_bytes(bytes[..8].try_into().ok()?);

        let status = AccountStatus::from_bits(bytes[8])?;
        let info = AccountInfo::deserialize(bytes[9..storage_ptr as usize].into())?;
        let storage = EvmStorage::deserialize(bytes[storage_ptr as usize..].into())?;

        Some(Self {
            info,
            status,
            storage,
        })
    }
}

#[test]
fn account_serialize_deserialize() {
    use crate::primitives::{
        Bytecode,
        EvmStorageSlot,
        B256,
        U256,
    };
    for account in [
        Account::default(),
        Account {
            info: AccountInfo {
                nonce: 1,
                balance: U256::from(2),
                code: Some(Bytecode::new_raw([1; 65].into())),
                code_hash: B256::from([1; 32]),
            },
            status: AccountStatus::Touched,
            storage: [(
                U256::from(1),
                EvmStorageSlot {
                    original_value: U256::from(100),
                    present_value: U256::from(200),
                    is_cold: true,
                },
            )]
            .into_iter()
            .collect(),
        },
        Account {
            info: AccountInfo {
                nonce: 9,
                balance: U256::from(12),
                code: None,
                code_hash: B256::from([11; 32]),
            },
            status: AccountStatus::Touched,
            storage: [(
                U256::from(1),
                EvmStorageSlot {
                    original_value: U256::from(200),
                    present_value: U256::from(000),
                    is_cold: false,
                },
            )]
            .into_iter()
            .collect(),
        },
        Account {
            info: AccountInfo {
                nonce: 9,
                balance: U256::from(12),
                code: None,
                code_hash: B256::from([11; 32]),
            },
            status: AccountStatus::SelfDestructed,
            storage: EvmStorage::default(),
        },
        Account {
            info: AccountInfo {
                nonce: 9,
                balance: U256::from(12),
                code: Some(Bytecode::new_raw([1; 65].into())),
                code_hash: B256::from([11; 32]),
            },
            status: AccountStatus::SelfDestructed,
            storage: EvmStorage::default(),
        },
    ] {
        let serialized = account.serialize();
        let deserialized = Account::deserialize(serialized).unwrap();

        assert_eq!(account, deserialized);
    }
}
