use super::{
    Deserialize,
    Serialize,
};
use crate::primitives::{
    AccountInfo,
    Bytecode,
    Bytes,
    B256,
    U256,
};
use sled::InlineArray;

impl Serialize for AccountInfo {
    fn serialize(&self) -> InlineArray {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.balance.to_be_bytes::<32>());
        bytes.extend_from_slice(&self.nonce.to_be_bytes());
        bytes.extend_from_slice(&self.code_hash.serialize());

        if let Some(code) = &self.code {
            bytes.extend_from_slice(code.bytes_slice());
        }
        bytes.into()
    }
}
impl Deserialize for AccountInfo {
    // Deserialization function
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let bytes = bytes.as_ref();
        let balance = U256::from_be_bytes::<32>(bytes[0..32].try_into().ok()?);
        let bytes = &bytes[32..];
        let nonce = u64::from_be_bytes(bytes[..8].try_into().ok()?);
        let bytes = &bytes[8..];
        let code_hash = B256::from_slice(&bytes[..32]);
        let bytes = &bytes[32..];

        let code = if !bytes.is_empty() {
            let bytecode = Bytecode::new_raw(Bytes::from(bytes.to_vec()));
            Some(bytecode)
        } else {
            None
        };

        Some(Self {
            balance,
            nonce,
            code_hash,
            code,
        })
    }
}
#[test]
fn test_account_info_bytes() {
    use crate::primitives::{
        B256,
        U256,
    };

    for account_info in [
        AccountInfo {
            balance: U256::from(100),
            nonce: 1,
            code_hash: B256::from([0; 32]),
            code: None,
        },
        AccountInfo {
            balance: U256::MAX,
            nonce: u64::MAX,
            code_hash: B256::from([u8::MAX; 32]),
            code: Some(Bytecode::new_raw(Bytes::from(vec![1, 2, 3]))),
        },
    ] {
        let bytes = account_info.serialize();
        let deserialized_account_info = AccountInfo::deserialize(bytes).unwrap();
        assert_eq!(account_info, deserialized_account_info);
    }
}
