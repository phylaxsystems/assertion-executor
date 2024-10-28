use super::{
    deserialize_unsized_vec,
    serialize_unsized_vec,
    Deserialize,
    Serialize,
};
use crate::primitives::{
    EvmStorage,
    EvmStorageSlot,
    U256,
};
use sled::InlineArray;

///Format for 3 items is as follows:
/// [0..8] - n number of items
/// [8..8 + 8n] - n pointers to the start of each item, each 8 bytes
/// [8 + 8n..] - n items, each beginning at the pointer specified in the previous step
impl Serialize for EvmStorage {
    fn serialize(&self) -> InlineArray {
        let serialized_items = self
            .iter()
            .map(|(block_num, val)| {
                let mut bytes = Vec::new();
                bytes.extend_from_slice(&block_num.to_be_bytes::<32>());
                bytes.extend_from_slice(&val.serialize());

                bytes.into()
            })
            .collect::<Vec<_>>();

        serialize_unsized_vec(serialized_items)
    }
}

///Format for 3 items is as follows:
/// [0..8] - n number of items
/// [8..8 + 8n] - n pointers to the start of each item, each 8 bytes
/// [8 + 8n..] - n items, each beginning at the pointer specified in the previous step
impl Deserialize for EvmStorage {
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        Some(deserialize_unsized_vec(bytes).into_iter().fold(
            EvmStorage::default(),
            |mut map, items| {
                for item in items {
                    let block_num = U256::from_be_slice(&item[0..32]);
                    let val = EvmStorageSlot::deserialize(item[32..].into()).unwrap();

                    map.insert(block_num, val);
                }
                map
            },
        ))
    }
}

impl Serialize for EvmStorageSlot {
    fn serialize(&self) -> InlineArray {
        let mut bytes = Vec::with_capacity(65);
        bytes.extend_from_slice(&self.original_value.to_be_bytes::<32>());
        bytes.extend_from_slice(&self.present_value.to_be_bytes::<32>());
        if self.is_cold {
            bytes.push(1);
        } else {
            bytes.push(0);
        }

        bytes.into()
    }
}
impl Deserialize for EvmStorageSlot {
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let original_value = U256::deserialize(InlineArray::from(&bytes[0..32]))?;
        let present_value = U256::deserialize(InlineArray::from(&bytes[32..64]))?;
        let is_cold = bytes[64] == 1;

        Some(Self {
            original_value,
            present_value,
            is_cold,
        })
    }
}

#[test]
fn test_evm_storage() {
    let mut storage = EvmStorage::default();
    storage.insert(
        U256::from(1),
        EvmStorageSlot {
            original_value: U256::from(100),
            present_value: U256::from(200),
            is_cold: true,
        },
    );
    storage.insert(
        U256::from(2),
        EvmStorageSlot {
            original_value: U256::from(200),
            present_value: U256::from(300),
            is_cold: false,
        },
    );
    let serialized = storage.serialize();
    let deserialized = EvmStorage::deserialize(serialized).unwrap();
    assert_eq!(storage, deserialized);
}

#[test]
fn test_evm_storage_slot() {
    let mut slot = EvmStorageSlot {
        original_value: U256::from(100),
        present_value: U256::from(200),
        is_cold: true,
    };
    let serialized = slot.serialize();
    let deserialized = EvmStorageSlot::deserialize(serialized).unwrap();
    assert_eq!(slot, deserialized);

    slot.is_cold = false;
    let serialized = slot.serialize();
    let deserialized = EvmStorageSlot::deserialize(serialized).unwrap();
    assert_eq!(slot, deserialized);
}
