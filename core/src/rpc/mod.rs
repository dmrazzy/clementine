#[allow(clippy::all)]
#[rustfmt::skip]
pub mod clementine;

use crate::{operator::Operator, verifier::Verifier};
use bitcoin_mock_rpc::RpcApiWrapper;
use clementine::{
    clementine_operator_server::ClementineOperator, clementine_verifier_server::ClementineVerifier,
    DepositSignSession, Empty, NonceGenResponse, OperatorBurnSig, OperatorParams, PartialSig,
    VerifierDepositFinalizeParams, VerifierDepositSignParams, VerifierPublicKeys, WatchtowerParams,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{async_trait, Request, Response, Status, Streaming};

#[async_trait]
impl<T> ClementineOperator for Operator<T>
where
    T: RpcApiWrapper,
{
    async fn get_params(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<OperatorParams>, Status> {
        todo!()
    }

    async fn deposit_sign(
        &self,
        _request: Request<DepositSignSession>,
    ) -> Result<Response<Self::DepositSignStream>, Status> {
        todo!()
    }

    #[doc = " Server streaming response type for the DepositSign method."]
    type DepositSignStream = ReceiverStream<Result<OperatorBurnSig, Status>>;
}

#[async_trait]
impl<T> ClementineVerifier for Verifier<T>
where
    T: RpcApiWrapper,
{
    type NonceGenStream = ReceiverStream<Result<NonceGenResponse, Status>>;
    type DepositSignStream = ReceiverStream<Result<PartialSig, Status>>;

    async fn set_verifiers(
        &self,
        _request: Request<VerifierPublicKeys>,
    ) -> Result<Response<Empty>, Status> {
        todo!()
    }

    async fn set_operator(
        &self,
        _request: Request<OperatorParams>,
    ) -> Result<Response<Empty>, Status> {
        todo!()
    }

    async fn set_watchtower(
        &self,
        _request: Request<WatchtowerParams>,
    ) -> Result<Response<Empty>, Status> {
        todo!()
    }

    async fn nonce_gen(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::NonceGenStream>, Status> {
        todo!()
    }

    async fn deposit_sign(
        &self,
        _request: Request<Streaming<VerifierDepositSignParams>>,
    ) -> Result<Response<Self::DepositSignStream>, Status> {
        todo!()
    }

    async fn deposit_finalize(
        &self,
        _request: tonic::Request<tonic::Streaming<VerifierDepositFinalizeParams>>,
    ) -> Result<Response<PartialSig>, Status> {
        todo!()
    }
}
