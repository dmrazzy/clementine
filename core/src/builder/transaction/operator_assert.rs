use self::input::get_kickoff_utxo_vout;
use self::output::UnspentTxOut;
use crate::builder;
pub use crate::builder::transaction::txhandler::TxHandler;
pub use crate::builder::transaction::*;
use crate::config::protocol::ProtocolParamset;
use crate::errors::BridgeError;
use crate::rpc::clementine::NormalSignatureKind;
use bitcoin::Sequence;

/// Creates a [`TxHandler`] for the `disprove_timeout_tx`. This transaction will be sent by the operator
/// to be able to send `reimburse_tx` later.
pub fn create_disprove_timeout_txhandler(
    kickoff_txhandler: &TxHandler,
    paramset: &'static ProtocolParamset,
) -> Result<TxHandler<Unsigned>, BridgeError> {
    Ok(TxHandlerBuilder::new(TransactionType::DisproveTimeout)
        .with_version(Version::non_standard(3))
        .add_input(
            NormalSignatureKind::OperatorSighashDefault,
            kickoff_txhandler.get_spendable_output(3)?,
            SpendPath::ScriptSpend(0),
            Sequence::from_height(paramset.disprove_timeout_timelock),
        )
        .add_input(
            NormalSignatureKind::DisproveTimeout2,
            kickoff_txhandler.get_spendable_output(1)?,
            SpendPath::ScriptSpend(0),
            DEFAULT_SEQUENCE,
        )
        .add_output(UnspentTxOut::from_partial(anchor_output()))
        .finalize())
}

pub fn create_mini_asserts(
    kickoff_txhandler: &TxHandler,
    num_asserts: usize,
) -> Result<Vec<TxHandler>, BridgeError> {
    let mut txhandlers = Vec::new();
    for idx in 0..num_asserts {
        txhandlers.push(
            TxHandlerBuilder::new(TransactionType::MiniAssert(idx))
                .with_version(Version::non_standard(3))
                .add_input(
                    NormalSignatureKind::MiniAssert1,
                    kickoff_txhandler.get_spendable_output(get_kickoff_utxo_vout(idx))?,
                    SpendPath::ScriptSpend(1),
                    DEFAULT_SEQUENCE,
                )
                .add_output(UnspentTxOut::from_partial(
                    builder::transaction::anchor_output(),
                ))
                .finalize(),
        );
    }
    Ok(txhandlers)
}
