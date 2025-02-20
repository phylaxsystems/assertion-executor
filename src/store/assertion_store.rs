use crate::{
    inspectors::CallTracer,
    primitives::{
        Address,
        AssertionContract,
        Bytes,
        B256,
        U256,
    },
    store::{
        assertion_contract_extractor::extract_assertion_contract,
        FnSelectorExtractorError,
        PendingModification,
    },
    ExecutorConfig,
};

use std::sync::{
    Arc,
    Mutex,
};

use bincode::{
    deserialize as de,
    serialize as ser,
};

use serde::{
    Deserialize,
    Serialize,
};

use tracing::error;

#[derive(thiserror::Error, Debug)]
pub enum AssertionStoreError {
    #[error("Sled error")]
    SledError(#[from] std::io::Error),
    #[error("Bincode error")]
    BincodeError(#[from] bincode::Error),
    #[error("Block number exceeds u64")]
    BlockNumberExceedsU64,
}

/// Struct representing a pending assertion modification that has not passed the timelock.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct AssertionState {
    active_at_block: u64,
    inactive_at_block: Option<u64>,
    assertion_contract: AssertionContract,
}

impl AssertionState {
    /// Creates a new active assertion state.
    /// WIll be active across all blocks.
    pub fn new_active(
        bytecode: Bytes,
        executor_config: &ExecutorConfig,
    ) -> Result<Self, FnSelectorExtractorError> {
        let assertion_contract = extract_assertion_contract(bytecode.clone(), executor_config)?;
        Ok(Self {
            assertion_contract,
            active_at_block: 0,
            inactive_at_block: None,
        })
    }

    #[cfg(any(test, feature = "test"))]
    pub fn new_test(bytecode: Bytes) -> Self {
        Self::new_active(bytecode, &ExecutorConfig::default()).unwrap()
    }

    /// Getter for the assertion_contract_id
    pub fn assertion_contract_id(&self) -> B256 {
        self.assertion_contract.id
    }
}

#[derive(Debug, Clone)]
pub struct AssertionStore {
    db: Arc<Mutex<sled::Tree>>,
}

impl AssertionStore {
    /// Create a new assertion store
    pub fn new(active_assertions_tree: sled::Tree) -> Self {
        Self {
            db: Arc::new(Mutex::new(active_assertions_tree)),
        }
    }

    /// Creates a new assertion store without persistence.
    pub fn new_ephemeral() -> Result<Self, AssertionStoreError> {
        let db = sled::Config::tmp()?.open()?;
        Ok(Self::new(db.open_tree("active_assertions")?))
    }

    /// Inserts the given assertion into the store.
    /// If an assertion with the same assertion_contract_id already exists, it is replaced.
    /// Returns the previous assertion if it existed.
    pub fn insert(
        &self,
        assertion_adopter: Address,
        assertion: AssertionState,
    ) -> Result<Option<AssertionState>, AssertionStoreError> {
        let db_lock = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let mut assertions: Vec<AssertionState> = db_lock
            .get(assertion_adopter)?
            .map(|a| de(&a))
            .transpose()?
            .unwrap_or_default();

        let position = assertions
            .iter()
            .position(|a| a.assertion_contract_id() == assertion.assertion_contract_id());

        let previous = if let Some(pos) = position {
            Some(std::mem::replace(&mut assertions[pos], assertion))
        } else {
            assertions.push(assertion);
            None
        };

        db_lock.insert(assertion_adopter, ser(&assertions)?)?;

        Ok(previous)
    }

    /// Applies the given modifications to the store.
    pub fn apply_pending_modifications(
        &self,
        pending_modifications: Vec<PendingModification>,
    ) -> Result<(), AssertionStoreError> {
        let mut map = std::collections::HashMap::<Address, Vec<PendingModification>>::new();

        for modification in pending_modifications {
            map.entry(modification.assertion_adopter())
                .or_default()
                .push(modification);
        }

        for (aa, mods) in map {
            self.apply_pending_modification(aa, mods)?;
        }

        Ok(())
    }

    /// Reads the assertions for the given block from the store, given the traces.
    pub fn read(
        &self,
        traces: &CallTracer,
        block_num: U256,
    ) -> Result<Vec<AssertionContract>, AssertionStoreError> {
        let block_num = block_num
            .try_into()
            .map_err(|_| AssertionStoreError::BlockNumberExceedsU64)?;

        let mut assertions = Vec::new();

        for contract_address in traces.calls() {
            let contract_assertions = self.read_adopter(contract_address, block_num)?;
            assertions.extend(contract_assertions);
        }

        Ok(assertions)
    }

    /// Reads the assertions for the given assertion adopter at the given block.
    /// Returns the assertions that are active at the given block.
    /// An assertion is considered active at a block if the active_at_block is less than or equal
    /// to the given block, and the inactive_at_block is greater than the given block.
    /// `assertion_adopter` is the address of the contract leveraging assertions.
    fn read_adopter(
        &self,
        assertion_adopter: Address,
        block: u64,
    ) -> Result<Vec<AssertionContract>, AssertionStoreError> {
        let assertion_states = self
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(assertion_adopter)?
            .map(|a| de::<Vec<AssertionState>>(&a))
            .transpose()?
            .unwrap_or_default();

        let active_assertion_contracts = assertion_states
            .into_iter()
            .filter(|a| {
                let inactive_block = match a.inactive_at_block {
                    Some(inactive_block) => {
                        // If the inactive block is less than the active block, the end bound is
                        // ignored.
                        if inactive_block < a.active_at_block {
                            u64::MAX
                        } else {
                            inactive_block
                        }
                    }
                    None => u64::MAX,
                };

                let in_bound_start = a.active_at_block <= block;
                let in_bound_end = block < inactive_block;
                in_bound_start && in_bound_end
            })
            .map(|a| a.assertion_contract)
            .collect();

        Ok(active_assertion_contracts)
    }

    /// Applies the given assertion adopter modifications to the store.
    fn apply_pending_modification(
        &self,
        assertion_adopter: Address,
        modifications: Vec<PendingModification>,
    ) -> Result<(), AssertionStoreError> {
        loop {
            let assertions_serialized = self
                .db
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(assertion_adopter)?;

            let mut assertions: Vec<AssertionState> = assertions_serialized
                .clone()
                .map(|a| de(&a))
                .transpose()?
                .unwrap_or_default();

            for modification in modifications.clone().into_iter() {
                match modification {
                    PendingModification::Add {
                        assertion_contract,
                        active_at_block,
                        ..
                    } => {
                        let existing_state = assertions
                            .iter_mut()
                            .find(|a| a.assertion_contract_id() == assertion_contract.id);

                        match existing_state {
                            Some(state) => {
                                state.active_at_block = active_at_block;
                            }
                            None => {
                                assertions.push(AssertionState {
                                    assertion_contract,
                                    active_at_block,
                                    inactive_at_block: None,
                                });
                            }
                        }
                    }
                    PendingModification::Remove {
                        assertion_contract_id,
                        inactive_at_block,
                        ..
                    } => {
                        let existing_state = assertions
                            .iter_mut()
                            .find(|a| a.assertion_contract_id() == assertion_contract_id);

                        match existing_state {
                            Some(state) => {
                                state.inactive_at_block = Some(inactive_at_block);
                            }
                            None => {
                                // The assertion was not found, so we add it with the inactive_at_block set.
                                error!(
                                    target = "assertion-executor::ssertion_store",
                                    ?assertion_contract_id,
                                    "Apply pending modifications error: Assertion not found for removal",
                                );
                            }
                        }
                    }
                }
            }
            let result = self
                .db
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .compare_and_swap(
                    assertion_adopter,
                    assertions_serialized,
                    Some(ser(&assertions)?),
                );

            if let Ok(Ok(_)) = result {
                break;
            } else {
                tracing::debug!(
                    target: "assertion-executor::assertion_store",
                    ?result, "Assertion store CAS failed, retrying");
            }
        }

        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Address;

    fn create_test_assertion(
        active_at_block: u64,
        inactive_at_block: Option<u64>,
    ) -> AssertionState {
        let state = AssertionState {
            active_at_block,
            inactive_at_block,
            assertion_contract: AssertionContract {
                id: B256::random(),
                ..Default::default()
            },
        };
        state
    }

    fn create_test_modification(
        active_at: u64,
        aa: Address,
        log_index: u64,
    ) -> PendingModification {
        create_test_modification_with_id(active_at, aa, log_index, B256::random())
    }

    fn create_test_modification_with_id(
        active_at: u64,
        aa: Address,
        log_index: u64,
        id: B256,
    ) -> PendingModification {
        PendingModification::Add {
            log_index,
            assertion_adopter: aa,
            assertion_contract: AssertionContract {
                id,
                ..Default::default()
            },
            active_at_block: active_at,
        }
    }

    fn create_test_modification_remove(
        inactive_at: u64,
        aa: Address,
        log_index: u64,
        id: B256,
    ) -> PendingModification {
        PendingModification::Remove {
            log_index,
            assertion_adopter: aa,
            assertion_contract_id: id,
            inactive_at_block: inactive_at,
        }
    }

    #[test]
    fn test_insert_and_read() -> Result<(), AssertionStoreError> {
        let store = AssertionStore::new_ephemeral()?;
        let aa = Address::random();

        // Create a test assertion
        let assertion = create_test_assertion(100, None);
        let assertion_contract_id = assertion.assertion_contract_id();

        // Insert the assertion
        store.insert(aa, assertion.clone())?;

        // Create a call tracer that includes our AA
        let mut tracer = CallTracer::default();
        tracer.insert_trace(aa);

        // Read at block 150 (should be active)
        let assertions = store.read(&tracer, U256::from(150))?;
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].id, assertion_contract_id);

        // Read at block 50 (should be inactive)
        let assertions = store.read(&tracer, U256::from(50))?;
        assert_eq!(assertions.len(), 0);

        Ok(())
    }

    #[test]
    fn test_apply_pending_modifications() -> Result<(), AssertionStoreError> {
        let store = AssertionStore::new_ephemeral()?;
        let aa = Address::random();

        // Create two modifications
        let mod1 = create_test_modification(100, aa, 0);
        let mod2 = create_test_modification(200, aa, 0);

        // Apply modifications
        store.apply_pending_modifications(vec![mod1.clone(), mod2])?;

        // Create a call tracer that includes our AA
        let mut tracer = CallTracer::default();
        tracer.insert_trace(aa);

        // Read at block 150 (should see first assertion only)
        let assertions = store.read(&tracer, U256::from(150))?;
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].id, mod1.assertion_contract_id());

        // Read at block 250 (should see both assertions)
        let assertions = store.read(&tracer, U256::from(250))?;
        assert_eq!(assertions.len(), 2);

        Ok(())
    }

    #[test]
    fn test_removal_modification() -> Result<(), AssertionStoreError> {
        let store = AssertionStore::new_ephemeral()?;
        let aa = Address::random();

        // Add an assertion
        let add_mod = create_test_modification(100, aa, 0);
        store.apply_pending_modifications(vec![add_mod.clone()])?;

        // Remove the assertion at block 200
        let remove_mod = PendingModification::Remove {
            log_index: 1,
            assertion_adopter: aa,
            assertion_contract_id: add_mod.assertion_contract_id(),
            inactive_at_block: 200,
        };
        store.apply_pending_modifications(vec![remove_mod])?;

        // Create a call tracer
        let mut tracer = CallTracer::default();
        tracer.insert_trace(aa);

        // Check at different blocks
        let assertions = store.read(&tracer, U256::from(150))?;
        assert_eq!(assertions.len(), 1); // Active

        let assertions = store.read(&tracer, U256::from(250))?;
        assert_eq!(assertions.len(), 0); // Inactive

        Ok(())
    }

    #[test]
    fn test_multiple_assertion_adopters() -> Result<(), AssertionStoreError> {
        let store = AssertionStore::new_ephemeral()?;
        let aa1 = Address::random();
        let aa2 = Address::random();

        // Create modifications for different AAs
        let mod1 = create_test_modification(100, aa1, 0);
        let mod2 = create_test_modification(100, aa2, 1);

        // Apply modifications
        store.apply_pending_modifications(vec![mod1.clone(), mod2])?;

        // Create a call tracer that includes both AAs
        let mut tracer = CallTracer::default();
        tracer.insert_trace(aa1);
        tracer.insert_trace(aa2);

        // Read at block 150 (should see both assertions)
        let assertions = store.read(&tracer, U256::from(150))?;
        assert_eq!(assertions.len(), 2);

        // Create a tracer with only aa1
        let mut tracer = CallTracer::default();
        tracer.insert_trace(aa1);

        // Should only see one assertion
        let assertions = store.read(&tracer, U256::from(150))?;
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].id, mod1.assertion_contract_id());

        Ok(())
    }

    #[test]
    fn test_update_same_assertion() -> Result<(), AssertionStoreError> {
        let store = AssertionStore::new_ephemeral()?;
        let aa = Address::random();

        let a_state = create_test_assertion(100, None);
        let id = a_state.assertion_contract_id();
        let _ = store.insert(aa, a_state);

        let mod1 = create_test_modification_remove(200, aa, 0, id);
        let mod2 = create_test_modification_with_id(300, aa, 1, id);

        // Apply modifications
        store.apply_pending_modifications(vec![mod1, mod2])?;

        // Create a call tracer that includes both AAs
        let mut tracer = CallTracer::default();
        tracer.insert_trace(aa);

        assert_eq!(store.db.lock().unwrap().len(), 1);
        let assertions: Vec<AssertionState> =
            de(&store.db.lock().unwrap().get(aa)?.unwrap()).unwrap();

        assert_eq!(assertions.len(), 1);

        // Read at block 250 (should see no assertion)
        let assertions = store.read(&tracer, U256::from(250))?;
        assert_eq!(assertions.len(), 0);

        let assertions = store.read(&tracer, U256::from(350))?;
        assert_eq!(assertions.len(), 1);

        Ok(())
    }

    #[test]
    fn test_block_number_exceeds_u64() {
        let store = AssertionStore::new_ephemeral().unwrap();
        let mut tracer = CallTracer::default();
        tracer.insert_trace(Address::random());

        let result = store.read(&tracer, U256::MAX);
        assert!(matches!(
            result,
            Err(AssertionStoreError::BlockNumberExceedsU64)
        ));
    }
}
