use crate::{
    db::multi_fork_db::{
        ForkError,
        ForkId,
        MultiForkDb,
    },
    primitives::{
        Bytes,
        JournaledState,
    },
};

use revm::{
    DatabaseRef,
    EvmContext,
    InnerEvmContext,
};

pub fn fork_pre_state(
    init_journaled_state: &JournaledState,
    context: &mut EvmContext<&mut MultiForkDb<impl DatabaseRef>>,
) -> Result<Bytes, ForkError> {
    let InnerEvmContext {
        ref mut db,
        ref mut journaled_state,
        ..
    } = context.inner;

    db.switch_fork(ForkId::PreTx, journaled_state, init_journaled_state)?;
    Ok(Bytes::default())
}

pub fn fork_post_state(
    init_journaled_state: &JournaledState,
    context: &mut EvmContext<&mut MultiForkDb<impl DatabaseRef>>,
) -> Result<Bytes, ForkError> {
    let InnerEvmContext {
        ref mut db,
        ref mut journaled_state,
        ..
    } = context.inner;

    db.switch_fork(ForkId::PostTx, journaled_state, init_journaled_state)?;
    Ok(Bytes::default())
}

#[cfg(test)]
mod test {
    use crate::test_utils::run_precompile_test;

    #[tokio::test]
    async fn test_fork_switching() {
        let result = run_precompile_test("TestFork").await;
        assert!(result.is_valid());
        let result_and_state = result.result_and_state;
        assert!(result_and_state.result.is_success());
    }
}
