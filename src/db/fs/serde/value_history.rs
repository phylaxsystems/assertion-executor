use super::{
    deserialize_unsized_vec,
    serialize_unsized_vec,
    Deserialize,
    Serialize,
};
use crate::primitives::ValueHistory;
use sled::InlineArray;
use std::collections::BTreeMap;

///Format for 3 items is as follows:
/// [0..8] - n number of items
/// [8..8 + 8n] - n pointers to the start of each item, each 8 bytes
/// [8 + 8n..] - n items, each beginning at the pointer specified in the previous step
impl<T: Serialize> Serialize for ValueHistory<T> {
    fn serialize(&self) -> InlineArray {
        let serialized_items = self
            .value_history
            .iter()
            .map(|(block_num, val)| {
                let serialized_val = val.serialize();

                let mut bytes = Vec::new();
                bytes.extend_from_slice(&block_num.serialize());
                bytes.extend_from_slice(&serialized_val);

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
impl<T: Deserialize> Deserialize for ValueHistory<T> {
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let value_history =
            deserialize_unsized_vec(bytes)
                .into_iter()
                .fold(BTreeMap::new(), |mut map, items| {
                    for item in items {
                        let block_num = u64::from_be_bytes(item[0..8].try_into().unwrap());
                        let val = T::deserialize(item[8..].into()).unwrap();

                        map.insert(block_num, val);
                    }
                    map
                });

        Some(Self { value_history })
    }
}

#[test]
fn test_value_history_u64() {
    let mut value_history = ValueHistory::default();
    value_history.value_history.insert(1, 1);
    value_history.value_history.insert(2, 2);
    value_history.value_history.insert(3, 4);
    let serialized = value_history.serialize();
    let deserialized = ValueHistory::deserialize(serialized).unwrap();
    assert_eq!(value_history, deserialized);
}

#[test]
fn test_value_history_acct() {
    use crate::primitives::{
        AccountInfo,
        Bytecode,
        Bytes,
        B256,
        U256,
    };
    let mut value_history = ValueHistory::default();
    for (block_num, info) in [
        (
            1,
            AccountInfo {
                balance: U256::from(100),
                nonce: 1,
                code_hash: B256::from([0; 32]),
                code: None,
            },
        ),
        (
            2,
            AccountInfo {
                balance: U256::MAX,
                nonce: u64::MAX,
                code_hash: B256::from([u8::MAX; 32]),
                code: Some(Bytecode::new_raw(Bytes::from(vec![1, 2, 3]))),
            },
        ),
        (
            3,
            AccountInfo {
                balance: U256::from(100),
                nonce: 100000,
                code_hash: B256::from([0; 32]),
                code: None,
            },
        ),
    ] {
        value_history.value_history.insert(block_num, info);
    }
    let serialized = value_history.serialize();
    let deserialized = ValueHistory::deserialize(serialized).unwrap();
    assert_eq!(value_history, deserialized);
}
