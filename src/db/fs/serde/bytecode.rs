use crate::primitives::Bytecode;

use super::{
    Deserialize,
    Serialize,
};
use sled::InlineArray;

impl Serialize for Bytecode {
    fn serialize(&self) -> InlineArray {
        self.bytes().0.as_ref().into()
    }
}

impl Deserialize for Bytecode {
    fn deserialize(bytes: InlineArray) -> Option<Self> {
        let bytes = bytes.as_ref().to_vec();
        let bytecode = Bytecode::new_raw(bytes.into());
        Some(bytecode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Bytecode;

    #[test]
    fn test_bytecode_serialize() {
        let bytecode = Bytecode::new_raw(vec![0x01, 0x02, 0x03].into());
        let serialized = bytecode.serialize();
        let deserialized = Bytecode::deserialize(serialized).unwrap();
        assert_eq!(bytecode, deserialized);
    }
}
