mod account;
mod account_info;
mod block_changes;
mod bytecode;
mod evm_storage;

mod storage_slot_key;
pub use storage_slot_key::StorageSlotKey;

mod value_history;

use crate::primitives::{
    Address,
    B256,
    U256,
};
use sled::InlineArray;
use zerocopy::{
    BigEndian,
    FromBytes,
    U64,
};

pub trait Serialize {
    fn serialize(&self) -> InlineArray;
}

pub trait Deserialize {
    fn deserialize(inline_array: InlineArray) -> Option<Self>
    where
        Self: Sized;
}

fn serialize_unsized_vec(items: Vec<InlineArray>) -> InlineArray {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&items.len().to_be_bytes());

    let mut position_ptr = 8 * (items.len() as u64 + 1);

    for item in &items {
        {
            buffer.extend_from_slice(&position_ptr.to_be_bytes());
        }
        position_ptr += item.len() as u64;
    }

    for item in items {
        buffer.extend_from_slice(item.to_vec().as_slice());
    }

    buffer.into()
}

fn deserialize_unsized_vec(bytes: InlineArray) -> Option<Vec<InlineArray>> {
    let bytes = bytes.as_ref();
    let len: U64<BigEndian> = U64::read_from_bytes(&bytes[0..8]).ok()?;

    let mut ptrs = Vec::new();
    let mut start = 8;
    for _ in 0..len.get() {
        let end = 8 + start;
        let ptr: U64<BigEndian> =
            U64::read_from_bytes(&bytes[start as usize..end as usize]).ok()?;
        ptrs.push(ptr.get());
        start = end;
    }

    let mut items = Vec::new();
    let mut iterator = ptrs.iter().peekable();
    while let Some(ptr) = iterator.next() {
        let ser_item = {
            if let Some(next_ptr) = iterator.peek() {
                &bytes[*ptr as usize..**next_ptr as usize]
            } else {
                &bytes[*ptr as usize..]
            }
        };
        items.push(InlineArray::from(ser_item));
    }
    Some(items)
}

macro_rules! impl_serialize_fixed_bytes {
    ($($t:ty),*) => {
        $(impl Serialize for $t {
            fn serialize(&self) -> InlineArray {
                InlineArray::from(self.0.as_slice())
            }
        })*
    };
}
impl_serialize_fixed_bytes!(B256, Address);

impl Serialize for Vec<u8> {
    fn serialize(&self) -> InlineArray {
        self.as_slice().into()
    }
}

impl Serialize for u64 {
    fn serialize(&self) -> InlineArray {
        self.to_be_bytes().as_slice().into()
    }
}

impl Deserialize for u64 {
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        Some(u64::from_be_bytes(bytes.as_ref().try_into().ok()?))
    }
}

impl Serialize for U256 {
    fn serialize(&self) -> InlineArray {
        (&self.to_be_bytes::<32>()).into()
    }
}

impl Deserialize for U256 {
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let fixed_bytes: [u8; 32] = bytes.as_ref().try_into().ok()?;
        Some(Self::from_be_bytes(fixed_bytes))
    }
}

impl Deserialize for B256 {
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let fixed_bytes: [u8; 32] = bytes.as_ref().try_into().ok()?;
        Some(Self::from(fixed_bytes))
    }
}

#[test]
fn test_unsized_vec() {
    let items = vec![InlineArray::from(b"hello"), InlineArray::from(b"world")];
    let serialized = serialize_unsized_vec(items.clone());
    let itemsd = deserialize_unsized_vec(serialized).unwrap();
    assert_eq!(items, itemsd);
}

#[test]
fn test_u256() {
    let u256 = U256::from(1);
    let serialized = u256.serialize();
    let u256d = U256::deserialize(serialized).unwrap();
    assert_eq!(u256, u256d);
}
