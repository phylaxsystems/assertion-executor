use crate::{
    inspectors::{
        CallTracer,
        TriggerRecorder,
        TriggerType,
    },
    primitives::{
        Address,
        AssertionContract,
        Bytes,
        FixedBytes,
        B256,
        U256,
    },
    store::{
        assertion_contract_extractor::{
            extract_assertion_contract,
            FnSelectorExtractorError,
        },
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

use tracing::{
    debug,
    error,
};

use std::collections::HashSet;

#[derive(thiserror::Error, Debug)]
pub enum AssertionStoreError {
    #[error("Sled error")]
    SledError(#[from] std::io::Error),
    #[error("Bincode error")]
    BincodeError(#[from] bincode::Error),
    #[error("Block number exceeds u64")]
    BlockNumberExceedsU64,
}

/// Struct representing an assertion contract, matched fn selectors, and the adopter.
/// This is necessary context when running assertions.
#[derive(Debug, Clone)]
pub struct AssertionsForExecution {
    pub assertion_contract: AssertionContract,
    pub selectors: Vec<FixedBytes<4>>,
    pub adopter: Address,
}

/// Struct representing a pending assertion modification that has not passed the timelock.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct AssertionState {
    pub active_at_block: u64,
    pub inactive_at_block: Option<u64>,
    pub assertion_contract: AssertionContract,
    pub trigger_recorder: TriggerRecorder,
}

impl AssertionState {
    /// Creates a new active assertion state.
    /// WIll be active across all blocks.
    #[allow(clippy::result_large_err)]
    pub fn new_active(
        bytecode: Bytes,
        executor_config: &ExecutorConfig,
    ) -> Result<Self, FnSelectorExtractorError> {
        let (contract, trigger_recorder) =
            extract_assertion_contract(bytecode.clone(), executor_config)?;
        Ok(Self {
            active_at_block: 0,
            inactive_at_block: None,
            assertion_contract: contract,
            trigger_recorder,
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
    db: Arc<Mutex<sled::Db>>,
}

impl AssertionStore {
    /// Create a new assertion store
    pub fn new(active_assertions_tree: sled::Db) -> Self {
        Self {
            db: Arc::new(Mutex::new(active_assertions_tree)),
        }
    }

    /// Creates a new assertion store without persistence.
    pub fn new_ephemeral() -> Result<Self, AssertionStoreError> {
        let db = sled::Config::tmp()?.open()?;
        Ok(Self::new(db))
    }

    /// Inserts the given assertion into the store.
    /// If an assertion with the same assertion_contract_id already exists, it is replaced.
    /// Returns the previous assertion if it existed.
    pub fn insert(
        &self,
        assertion_adopter: Address,
        assertion: AssertionState,
    ) -> Result<Option<AssertionState>, AssertionStoreError> {
        debug!(
            target: "assertion-executor::assertion_store",
            assertion_adopter=?assertion_adopter,
            active_at_block=?assertion.active_at_block,
            triggers=?assertion.trigger_recorder.triggers,
            "Inserting assertion into store"
        );

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
    ) -> Result<Vec<AssertionsForExecution>, AssertionStoreError> {
        let block_num = block_num
            .try_into()
            .map_err(|_| AssertionStoreError::BlockNumberExceedsU64)?;

        let mut assertions = Vec::new();
        for (contract_address, triggers) in traces.triggers() {
            let contract_assertions = self.read_adopter(contract_address, triggers, block_num)?;
            let assertions_for_execution: Vec<AssertionsForExecution> = contract_assertions
                .into_iter()
                .map(|(assertion_contract, selectors)| {
                    AssertionsForExecution {
                        assertion_contract,
                        selectors,
                        adopter: contract_address,
                    }
                })
                .collect();

            assertions.extend(assertions_for_execution);
        }
        debug!(target: "assertion-executor::assertion_store", assertions=?assertions, triggers=?traces.triggers(), "Assertions found based on triggers");
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
        triggers: HashSet<TriggerType>,
        block: u64,
    ) -> Result<Vec<(AssertionContract, Vec<FixedBytes<4>>)>, AssertionStoreError> {
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
            .map(|a| {
                // Get all function selectors from matching triggers
                let mut all_selectors = HashSet::new();
                let mut has_call_trigger = false;
                let mut has_storage_trigger = false;

                // Process specific triggers and detect trigger types
                for trigger in &triggers {
                    if let Some(selectors) = a.trigger_recorder.triggers.get(trigger) {
                        all_selectors.extend(selectors.iter().cloned());
                    }

                    // Check trigger type while we're iterating
                    match trigger {
                        TriggerType::Call { .. } => has_call_trigger = true,
                        TriggerType::StorageChange { .. } => has_storage_trigger = true,
                        _ => {}
                    }
                }

                // Add AllCalls selectors if needed
                if has_call_trigger {
                    if let Some(selectors) = a.trigger_recorder.triggers.get(&TriggerType::AllCalls)
                    {
                        all_selectors.extend(selectors.iter().cloned());
                    }
                }

                // Add AllStorageChanges selectors if needed
                if has_storage_trigger {
                    if let Some(selectors) = a
                        .trigger_recorder
                        .triggers
                        .get(&TriggerType::AllStorageChanges)
                    {
                        all_selectors.extend(selectors.iter().cloned());
                    }
                }
                // Convert HashSet to Vec to match the expected return type
                (a.assertion_contract, all_selectors.into_iter().collect())
            })
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

            debug!(
                target: "assertion-executor::assertion_store",
                pending_modifations_len = assertions.len(),
                "Applying pending modifications"
            );

            for modification in modifications.clone().into_iter() {
                match modification {
                    PendingModification::Add {
                        assertion_contract,
                        trigger_recorder,
                        active_at_block,
                        ..
                    } => {
                        debug!(
                            target: "assertion-executor::assertion_store",
                            ?assertion_contract,
                            ?trigger_recorder,
                            active_at_block,
                            "Applying pending assertion addition"
                        );
                        let existing_state = assertions
                            .iter_mut()
                            .find(|a| a.assertion_contract_id() == assertion_contract.id);

                        match existing_state {
                            Some(state) => {
                                state.active_at_block = active_at_block;
                            }
                            None => {
                                assertions.push(AssertionState {
                                    active_at_block,
                                    inactive_at_block: None,
                                    assertion_contract,
                                    trigger_recorder,
                                });
                            }
                        }
                    }
                    PendingModification::Remove {
                        assertion_contract_id,
                        inactive_at_block,
                        ..
                    } => {
                        debug!(
                            target: "assertion-executor::assertion_store",
                            ?assertion_contract_id,
                            inactive_at_block,
                            "Applying pending assertion removal"
                        );
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
                                    target: "assertion-executor::assertion_store",
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
                    ?result,
                    "Assertion store Compare and Swap failed, retrying"
                );
            }
        }

        Ok(())
    }

    #[cfg(any(test, feature = "test"))]
    pub fn assertion_contract_count(&self, assertion_adopter: Address) -> usize {
        let assertions = self.get_assertions_for_contract(assertion_adopter);
        assertions.len()
    }

    #[cfg(any(test, feature = "test"))]
    pub fn get_assertions_for_contract(&self, assertion_adopter: Address) -> Vec<AssertionState> {
        let assertions_serialized = self
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(assertion_adopter)
            .unwrap_or(None);

        if let Some(assertions_serialized) = assertions_serialized {
            let assertions: Vec<AssertionState> = de(&assertions_serialized).unwrap_or_default();
            assertions
        } else {
            vec![]
        }
    }
}
#[cfg(test)]
mod tests {

    use super::*;
    use crate::primitives::{
        Address,
        JournalEntry,
        JournaledState,
        SpecId,
    };

    use revm::primitives::HashSet as RevmHashSet;
    use std::collections::HashSet;

    fn create_test_assertion(
        active_at_block: u64,
        inactive_at_block: Option<u64>,
    ) -> AssertionState {
        AssertionState {
            active_at_block,
            inactive_at_block,
            assertion_contract: AssertionContract {
                id: B256::random(),
                ..Default::default()
            },
            trigger_recorder: TriggerRecorder::default(),
        }
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
            assertion_adopter: aa,
            assertion_contract: AssertionContract {
                id,
                ..Default::default()
            },
            trigger_recorder: TriggerRecorder::default(),
            active_at_block: active_at,
            log_index,
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
        assert_eq!(assertions[0].assertion_contract.id, assertion_contract_id);

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
        assert_eq!(
            assertions[0].assertion_contract.id,
            mod1.assertion_contract_id()
        );

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
        assert_eq!(
            assertions[0].assertion_contract.id,
            mod1.assertion_contract_id()
        );

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

    fn setup_and_match(
        recorded_triggers: Vec<(TriggerType, HashSet<FixedBytes<4>>)>,
        journal_entries: Vec<JournalEntry>,
        assertion_adopter: Address,
    ) -> Result<Vec<AssertionsForExecution>, AssertionStoreError> {
        let store = AssertionStore::new_ephemeral()?;
        let mut trigger_recorder = TriggerRecorder::default();

        recorded_triggers.iter().for_each(|(trigger, selectors)| {
            trigger_recorder
                .triggers
                .insert(trigger.clone(), selectors.clone());
        });

        let mut assertion = create_test_assertion(100, None);
        assertion.trigger_recorder = trigger_recorder;
        store.insert(assertion_adopter, assertion)?;

        let mut tracer = CallTracer::default();
        // insert_trace inserts (address, 0x00000000) in call_inputs to pretend a call
        tracer.insert_trace(assertion_adopter);

        tracer.journaled_state = Some(JournaledState::new(
            SpecId::LONDON,
            RevmHashSet::<Address>::default(),
        ));

        tracer
            .journaled_state
            .as_mut()
            .unwrap()
            .journal
            .push(journal_entries);

        store.read(&tracer, U256::from(100))
    }

    #[test]
    fn test_read_adopter_with_all_triggers() -> Result<(), AssertionStoreError> {
        let aa = Address::random();

        let assertion_selector_call = FixedBytes::<4>::random();
        let assertion_selector_storage = FixedBytes::<4>::random();
        let assertion_selector_both = FixedBytes::<4>::random();
        let mut expected_selectors = vec![
            assertion_selector_call,
            assertion_selector_storage,
            assertion_selector_both,
        ];
        expected_selectors.sort();

        // Create recorded triggers for all calls and storage changes
        let recorded_triggers = vec![
            (
                TriggerType::AllCalls,
                vec![assertion_selector_call, assertion_selector_both]
                    .into_iter()
                    .collect::<HashSet<_>>(),
            ),
            (
                TriggerType::AllStorageChanges,
                vec![assertion_selector_storage, assertion_selector_both]
                    .into_iter()
                    .collect::<HashSet<_>>(),
            ),
        ];

        let journal_entries = vec![JournalEntry::StorageChanged {
            address: aa,
            key: U256::from(1),
            had_value: U256::from(0),
        }];

        let assertions = setup_and_match(recorded_triggers, journal_entries, aa)?;
        assert_eq!(assertions.len(), 1);
        let mut matched_selectors = assertions[0].selectors.clone();
        matched_selectors.sort();
        assert_eq!(matched_selectors, expected_selectors);

        Ok(())
    }

    #[test]
    fn test_read_adopter_with_specific_triggers() -> Result<(), AssertionStoreError> {
        let aa = Address::random();
        let assertion_selector_call = FixedBytes::<4>::random();
        let assertion_selector_storage = FixedBytes::<4>::random();
        let assertion_selector_balance = FixedBytes::<4>::random();

        let mut expected_selectors = vec![
            assertion_selector_call,
            assertion_selector_storage,
            assertion_selector_balance,
        ];
        expected_selectors.sort();

        let trigger_selector = FixedBytes::<4>::default();

        let recorded_triggers = vec![
            (
                TriggerType::Call { trigger_selector },
                vec![assertion_selector_call]
                    .into_iter()
                    .collect::<HashSet<_>>(),
            ),
            (
                TriggerType::StorageChange {
                    trigger_slot: U256::from(1).into(),
                },
                vec![assertion_selector_storage]
                    .into_iter()
                    .collect::<HashSet<_>>(),
            ),
            (
                TriggerType::BalanceChange,
                vec![assertion_selector_balance]
                    .into_iter()
                    .collect::<HashSet<_>>(),
            ),
        ];

        let journal_entries = vec![
            JournalEntry::StorageChanged {
                address: aa,
                key: U256::from(1),
                had_value: U256::from(0),
            },
            JournalEntry::BalanceTransfer {
                from: aa,
                to: Address::random(),
                balance: U256::from(1),
            },
        ];

        let assertions = setup_and_match(recorded_triggers, journal_entries, aa)?;
        assert_eq!(assertions.len(), 1);
        let mut matched_selectors = assertions[0].selectors.clone();
        matched_selectors.sort();
        assert_eq!(matched_selectors, expected_selectors);

        Ok(())
    }

    #[test]
    fn test_read_adopter_only_match_call_trigger() -> Result<(), AssertionStoreError> {
        let aa = Address::random();

        let assertion_selector_call = FixedBytes::<4>::random();
        let assertion_selector_all_storage = FixedBytes::<4>::random();
        let assertion_selector_balance = FixedBytes::<4>::random();
        let mut expected_selectors = vec![assertion_selector_call];
        expected_selectors.sort();

        let trigger_selector_call = FixedBytes::<4>::default();

        let recorded_triggers = vec![
            (
                TriggerType::Call {
                    trigger_selector: trigger_selector_call,
                },
                vec![assertion_selector_call]
                    .into_iter()
                    .collect::<HashSet<_>>(),
            ),
            (
                TriggerType::AllStorageChanges,
                vec![assertion_selector_all_storage]
                    .into_iter()
                    .collect::<HashSet<_>>(),
            ),
            (
                TriggerType::BalanceChange,
                vec![assertion_selector_balance]
                    .into_iter()
                    .collect::<HashSet<_>>(),
            ),
        ];

        let assertions = setup_and_match(recorded_triggers, vec![], aa)?;
        assert_eq!(assertions.len(), 1);
        let mut matched_selectors = assertions[0].selectors.clone();
        matched_selectors.sort();
        assert_eq!(matched_selectors, expected_selectors);

        Ok(())
    }
}
