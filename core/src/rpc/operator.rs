use super::clementine::{
    clementine_operator_server::ClementineOperator, DepositSignSession, Empty,
    NewWithdrawalSigParams, NewWithdrawalSigResponse, OperatorBurnSig, OperatorParams,
    WithdrawalFinalizedParams,
};
use crate::traits::rpc::OperatorRpcServer;
use crate::{errors::BridgeError, operator::Operator};
use bitcoin::OutPoint;
use bitcoin_mock_rpc::RpcApiWrapper;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{async_trait, Request, Response, Status};

#[async_trait]
impl<T> ClementineOperator for Operator<T>
where
    T: RpcApiWrapper,
{
    type DepositSignStream = ReceiverStream<Result<OperatorBurnSig, Status>>;

    #[tracing::instrument(skip(self), err(level = tracing::Level::ERROR), ret(level = tracing::Level::TRACE))]
    async fn get_params(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<OperatorParams>, Status> {
        todo!()
    }

    #[tracing::instrument(skip(self), err(level = tracing::Level::ERROR), ret(level = tracing::Level::TRACE))]
    async fn deposit_sign(
        &self,
        _request: Request<DepositSignSession>,
    ) -> Result<Response<Self::DepositSignStream>, Status> {
        todo!()
    }

    #[tracing::instrument(skip(self), err(level = tracing::Level::ERROR), ret(level = tracing::Level::TRACE))]
    async fn new_withdrawal_sig(
        &self,
        _: Request<NewWithdrawalSigParams>,
    ) -> Result<Response<NewWithdrawalSigResponse>, Status> {
        todo!()
    }

    #[tracing::instrument(skip(self), err(level = tracing::Level::ERROR), ret(level = tracing::Level::TRACE))]
    async fn withdrawal_finalized(
        &self,
        request: Request<WithdrawalFinalizedParams>,
    ) -> Result<Response<Empty>, Status> {
        // Decode inputs.
        let withdrawal_idx = request.get_ref().withdrawal_id;
        let deposit_outpoint: OutPoint = request
            .get_ref()
            .deposit_outpoint
            .clone()
            .ok_or(BridgeError::RPCRequiredFieldError("deposit_outpoint"))?
            .try_into()?;

        self.check_citrea_for_withdrawal(withdrawal_idx, deposit_outpoint)
            .await?;

        self.withdrawal_proved_on_citrea_rpc(withdrawal_idx, deposit_outpoint)
            .await
            .unwrap();

        Ok(Response::new(Empty {}))
    }
}
