use revm::{
    inspector_handle_register,
    primitives::{
        BlockEnv,
        CfgEnv,
        Env,
        EnvWithHandlerCfg,
        SpecId,
        TxEnv,
    },
    Context,
    Database,
    Evm,
    EvmContext,
    Handler,
    Inspector,
};

pub fn new_evm<'evm, 'i, 'db, DB: Database, I: Inspector<&'db mut DB>>(
    tx_env: TxEnv,
    block_env: BlockEnv,
    chain_id: u64,
    spec_id: SpecId,
    db: &'db mut DB,
    inspector: I,
) -> Evm<'evm, I, &'db mut DB> {
    let mut cfg_env = CfgEnv::default();
    cfg_env.chain_id = chain_id;

    let env = Env {
        block: block_env,
        tx: tx_env,
        cfg: cfg_env,
    };

    let EnvWithHandlerCfg { env, handler_cfg } =
        EnvWithHandlerCfg::new_with_spec_id(Box::new(env), spec_id);

    let mut handler = Handler::new(handler_cfg);
    handler.append_handler_register_plain(inspector_handle_register);

    let context = Context::new(EvmContext::new_with_env(db, env), inspector);

    Evm::new(context, handler)
}
