use crate::{
    inspectors::phevm::PhEvmContext,
    inspectors::sol_primitives::PhEvm,
    primitives::{
        Address,
        Bytes,
        JournaledState,
        U256,
    },
};

use alloy_sol_types::{
    SolCall,
    SolValue,
};

use revm::{
    interpreter::CallInputs,
    JournalEntry,
};

#[derive(Debug, thiserror::Error)]
pub enum GetStateChangesError {
    #[error("Error decoding call inputs")]
    CallDecodeError(#[from] alloy_sol_types::Error),
    #[error("Journaled State Missing from Context. For some reason it was not captured.")]
    JournaledStateMissing,
    #[error("Slot not found in journaled state, but differences were found.")]
    SlotNotFound,
    #[error("Account not found in journaled state, but differences were found.")]
    AccountNotFound,
}

/// Function for getting state changes for the PhEvm precompile.
/// This returns a result type, which can be used to determine the success of the precompile call and include error messaging.
pub fn get_state_changes(
    inputs: &CallInputs,
    context: &PhEvmContext,
) -> Result<Bytes, GetStateChangesError> {
    let event = PhEvm::getStateChangesCall::abi_decode(&inputs.input, true)?;
    let journaled_state = context
        .call_traces
        .journaled_state
        .as_ref()
        .ok_or(GetStateChangesError::JournaledStateMissing)?;

    let differences = get_differences(journaled_state, event.contractAddress, event.slot.into())?;

    Ok(Vec::<U256>::abi_encode(&differences).into())
}

/// Returns an array of different values for an account and slot, from the JournaledState passed.
fn get_differences(
    journaled_state: &JournaledState,
    contract_address: Address,
    slot: U256,
) -> Result<Vec<U256>, GetStateChangesError> {
    let mut differences = Vec::new();

    for entry in journaled_state.journal.iter().flatten() {
        if let JournalEntry::StorageChanged {
            address,
            had_value,
            key,
        } = entry
        {
            if *address == **contract_address && *key == slot {
                differences.push(*had_value);
            }
        }
    }

    // If any differences were found in the journal, check the state to get the current value.
    // The account should always exist in state if differences were found in the journal.
    if !differences.is_empty() {
        let journaled_state_account = journaled_state
            .state
            .get(&contract_address)
            .ok_or(GetStateChangesError::AccountNotFound)?;

        let current_slot_value = journaled_state_account
            .storage
            .get(&slot)
            .ok_or(GetStateChangesError::SlotNotFound)?
            .present_value;

        differences.push(current_slot_value);
    };

    Ok(differences)
}

#[cfg(test)]
mod test {
    use crate::test_utils::run_precompile_test;

    #[tokio::test]
    async fn test_get_statechanges() {
        let result = run_precompile_test("TestGetStateChanges").await;
        assert!(result.is_valid(), "{result:#?}");
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
