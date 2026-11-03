//! Token swaps quoted and built by the blockchain-api, signed only once the
//! quote and its transaction pass [`crate::verify::swap_route`]'s checks.
//!
//! [`prepare`] runs the checks and returns a [`PreparedSwap`] holding the
//! unsigned transaction they passed. Only this crate constructs one, and
//! [`PreparedSwap::sign`] signs only a message [`prepare`] checked. A caller
//! who signs the output of `Client::swap_instructions` by other means gets
//! none of the checks. Preparing touches no signer, so a caller can show the
//! checked transaction and stop without signing.

use crate::{
    blockchain_api::{
        types::{SwapInstructionsRequest, SwapQuote, TransactionData},
        BlockchainApiError, Client,
    },
    client::SolanaRpcClient,
    keypair::Pubkey,
    solana_sdk::signer::Signer,
    transaction::VersionedTransaction,
    verify::swap_route::{
        check_quote, verify_unsigned, SwapChain, SwapRefusal, SwapRouteError, SwapTerms,
    },
};
use serde_json::Value;

/// The swap a caller asks for: exactly `amount` of `input_mint` into at least
/// `min_output` of `output_mint`, all in each mint's smallest units.
#[derive(Debug, Clone, Copy)]
pub struct SwapRequest {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount: u64,
    /// The caller's own floor on the output, independent of the quote. Must
    /// be greater than zero.
    pub min_output: u64,
    /// The slippage the quote must be built with, in basis points.
    pub slippage_bps: u16,
}

/// A swap transaction that passed every check, not yet signed. Built only by
/// [`prepare`].
#[derive(Debug)]
pub struct PreparedSwap {
    quote: SwapQuote,
    data: TransactionData,
    transaction: VersionedTransaction,
    last_valid_block_height: u64,
    taker: Pubkey,
}

impl PreparedSwap {
    /// The quote the transaction was built from.
    pub fn quote(&self) -> &SwapQuote {
        &self.quote
    }

    /// The envelope the transaction came in.
    pub fn data(&self) -> &TransactionData {
        &self.data
    }

    /// The unsigned transaction the checks passed, with the fresh blockhash
    /// they simulated it under and an empty signature slot for the taker.
    pub fn transaction(&self) -> &VersionedTransaction {
        &self.transaction
    }

    /// Sign the checked transaction's message as the taker, and nothing else.
    /// A keypair whose wallet is not the taker is refused before it is asked
    /// to sign.
    ///
    /// Expiry is not checked: a prepared swap held past its last valid block
    /// height still signs, and the signed transaction cannot land.
    pub fn sign(self, keypair: &(dyn Signer + Sync)) -> Result<SignedSwap, SwapRouteError> {
        let signer = keypair.try_pubkey().map_err(SwapRouteError::Signing)?;
        if signer != self.taker {
            return Err(SwapRouteError::SignerNotTaker {
                signer,
                taker: self.taker,
            });
        }
        let transaction = VersionedTransaction::try_new(self.transaction.message, &[keypair])
            .map_err(SwapRouteError::Signing)?;
        Ok(SignedSwap {
            quote: self.quote,
            data: self.data,
            transaction,
            last_valid_block_height: self.last_valid_block_height,
        })
    }
}

/// A swap transaction that passed every check and is signed by the taker.
#[derive(Debug)]
pub struct SignedSwap {
    /// The quote the transaction was built from.
    pub quote: SwapQuote,
    /// The envelope the transaction came in, for submission through
    /// [`Client::submit_signed`].
    pub data: TransactionData,
    /// The signed transaction.
    pub transaction: VersionedTransaction,
    /// The last block height at which the transaction can land.
    pub last_valid_block_height: u64,
}

/// Quote `request` through `api`, have it build the swap for `taker`, and
/// return the unsigned transaction only if the quote names exactly the
/// request and its transaction does nothing beyond swapping `request.amount`
/// into at least `request.min_output` for that wallet (see
/// [`crate::verify::swap_route`]). The simulation that check relies on runs
/// on `client`'s RPC node. Nothing is signed; [`PreparedSwap::sign`] does
/// that.
///
/// A request with a zero amount or minimum output, or the same mint on both
/// sides, is refused before any request is made.
pub async fn prepare<C: AsRef<SolanaRpcClient>>(
    api: &Client,
    client: &C,
    request: &SwapRequest,
    taker: Pubkey,
) -> Result<PreparedSwap, SwapRouteError> {
    prepare_with(api, client.as_ref(), request, taker).await
}

/// The blockchain-api calls a swap makes, behind a trait so the checks run
/// against recorded responses in tests.
pub(crate) trait SwapApi {
    async fn quote(&self, request: &SwapRequest) -> Result<SwapQuote, BlockchainApiError>;
    async fn instructions(
        &self,
        request: &SwapInstructionsRequest,
    ) -> Result<TransactionData, BlockchainApiError>;
}

impl SwapApi for Client {
    async fn quote(&self, request: &SwapRequest) -> Result<SwapQuote, BlockchainApiError> {
        self.swap_quote(
            &request.input_mint,
            &request.output_mint,
            request.amount,
            request.slippage_bps,
        )
        .await
    }

    async fn instructions(
        &self,
        request: &SwapInstructionsRequest,
    ) -> Result<TransactionData, BlockchainApiError> {
        self.swap_instructions(request).await
    }
}

pub(crate) async fn prepare_with<A: SwapApi, S: SwapChain>(
    api: &A,
    chain: &S,
    request: &SwapRequest,
    taker: Pubkey,
) -> Result<PreparedSwap, SwapRouteError> {
    let terms = SwapTerms::new(request, taker)?;

    let mut quote = api.quote(request).await.map_err(SwapRouteError::api)?;
    let min_out = check_quote(&quote, &terms)?;
    request_v2_route(&mut quote);
    let data = api
        .instructions(&SwapInstructionsRequest {
            quote_response: quote.clone(),
            user_public_key: taker.to_string(),
            destination_token_account: None,
        })
        .await
        .map_err(SwapRouteError::api)?;

    // The checks bound one transaction's effects by the whole swap, and a
    // later transaction's simulation would run against state an earlier one
    // has not yet changed, so anything but one transaction is refused.
    if data.transactions.len() != 1 {
        return Err(SwapRefusal::TransactionCount {
            found: data.transactions.len(),
        }
        .into());
    }
    let txn = data
        .decode_transactions()
        .map_err(SwapRouteError::TransactionDecode)?
        .remove(0);
    let (transaction, last_valid_block_height) =
        verify_unsigned(chain, &terms, min_out, txn).await?;
    Ok(PreparedSwap {
        quote,
        data,
        transaction,
        last_valid_block_height,
        taker,
    })
}

/// Ask for the route instructions whose terms the checks read at a fixed
/// offset: `instructionVersion` V2, with each route-plan step's share in
/// basis points, which those instructions carry in place of a percentage.
/// Only fields outside [`check_quote`]'s are changed, so the quote sent is
/// the quote checked. A builder that ignores the request returns an older
/// route form, which the instruction check refuses.
fn request_v2_route(quote: &mut SwapQuote) {
    let Value::Object(fields) = &mut quote.extra else {
        return;
    };
    fields.insert("instructionVersion".to_string(), "V2".into());
    let Some(Value::Array(steps)) = fields.get_mut("routePlan") else {
        return;
    };
    for step in steps.iter_mut().filter_map(Value::as_object_mut) {
        if step.get("bps").is_some_and(|bps| !bps.is_null()) {
            continue;
        }
        if let Some(percent) = step.get("percent").and_then(Value::as_u64) {
            step.insert("bps".to_string(), percent.saturating_mul(100).into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blockchain_api::types::TransactionItem,
        solana_sdk::signature::Signature,
        verify::swap_route::{
            tests::{case, Case, CountingSigner, HNT_USDC, LAST_VALID_BLOCK_HEIGHT},
            InvalidSwapRequest,
        },
    };
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use serde_json::json;
    use std::sync::Mutex;

    /// Answers with a recorded quote and transactions, and records what it
    /// was asked.
    struct FakeApi {
        quote: Result<SwapQuote, u16>,
        transactions: Vec<String>,
        quotes_asked: Mutex<Vec<SwapRequest>>,
        built: Mutex<Vec<SwapInstructionsRequest>>,
    }

    impl FakeApi {
        fn new(case: &Case) -> Self {
            let transaction =
                BASE64.encode(bincode::serialize(&case.transaction).expect("serialize fixture"));
            Self {
                quote: Ok(case.quote.clone()),
                transactions: vec![transaction],
                quotes_asked: Mutex::default(),
                built: Mutex::default(),
            }
        }

        fn calls(&self) -> (usize, usize) {
            (
                self.quotes_asked.lock().expect("lock").len(),
                self.built.lock().expect("lock").len(),
            )
        }
    }

    impl SwapApi for FakeApi {
        async fn quote(&self, request: &SwapRequest) -> Result<SwapQuote, BlockchainApiError> {
            self.quotes_asked.lock().expect("lock").push(*request);
            self.quote
                .clone()
                .map_err(|status| BlockchainApiError::Api {
                    status,
                    message: "unavailable".to_string(),
                })
        }

        async fn instructions(
            &self,
            request: &SwapInstructionsRequest,
        ) -> Result<TransactionData, BlockchainApiError> {
            self.built.lock().expect("lock").push(request.clone());
            Ok(TransactionData {
                transactions: self
                    .transactions
                    .iter()
                    .map(|serialized| TransactionItem {
                        serialized_transaction: serialized.clone(),
                        metadata: None,
                    })
                    .collect(),
                parallel: false,
                tag: None,
                action_metadata: None,
            })
        }
    }

    fn request(case: &Case) -> SwapRequest {
        let terms = case.request;
        SwapRequest {
            input_mint: terms.input_mint,
            output_mint: terms.output_mint,
            amount: terms.amount,
            min_output: terms.min_output,
            slippage_bps: terms.slippage_bps,
        }
    }

    async fn run(
        api: &FakeApi,
        case: &Case,
        request: &SwapRequest,
    ) -> (Result<SignedSwap, SwapRouteError>, usize) {
        let signer = CountingSigner::new(&case.request.taker);
        let result = prepare_with(api, &case.chain, request, case.request.taker)
            .await
            .and_then(|prepared| prepared.sign(&signer));
        (result, signer.signed())
    }

    fn refusal(result: Result<SignedSwap, SwapRouteError>) -> SwapRefusal {
        match result {
            Err(SwapRouteError::Refused(refusal)) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_recorded_swap_is_quoted_built_for_the_wallet_and_signed() {
        let case = case(HNT_USDC);
        let api = FakeApi::new(&case);
        let request = request(&case);
        let (result, signed) = run(&api, &case, &request).await;
        let swap = result.expect("signed");
        assert_eq!(signed, 1);
        assert_eq!(swap.last_valid_block_height, LAST_VALID_BLOCK_HEIGHT);
        assert_eq!(swap.transaction.signatures.len(), 1);
        assert_eq!(swap.data.transactions.len(), 1);

        let asked = api.quotes_asked.lock().expect("lock");
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].amount, request.amount);
        let built = api.built.lock().expect("lock");
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].user_public_key, case.request.taker.to_string());
        assert_eq!(built[0].quote_response.extra["instructionVersion"], "V2");
        // The terms the checks read are sent as they were checked.
        assert_eq!(built[0].quote_response.in_amount, case.quote.in_amount);
        assert_eq!(
            built[0].quote_response.other_amount_threshold,
            case.quote.other_amount_threshold
        );
    }

    #[tokio::test]
    async fn an_unusable_request_is_refused_before_any_request() {
        let case = case(HNT_USDC);
        let base = request(&case);
        for (request, expected) in [
            (
                SwapRequest { amount: 0, ..base },
                InvalidSwapRequest::ZeroAmount,
            ),
            (
                SwapRequest {
                    min_output: 0,
                    ..base
                },
                InvalidSwapRequest::ZeroMinOutput,
            ),
            (
                SwapRequest {
                    output_mint: base.input_mint,
                    ..base
                },
                InvalidSwapRequest::SameMint,
            ),
        ] {
            let api = FakeApi::new(&case);
            let (result, signed) = run(&api, &case, &request).await;
            assert!(
                matches!(&result, Err(SwapRouteError::InvalidRequest(refused)) if *refused == expected),
                "{result:?}"
            );
            assert_eq!(api.calls(), (0, 0));
            assert_eq!(signed, 0);
        }
    }

    #[tokio::test]
    async fn a_quote_below_the_callers_minimum_is_refused_before_it_is_built() {
        let case = case(HNT_USDC);
        let api = FakeApi::new(&case);
        let threshold: u64 = case
            .quote
            .other_amount_threshold
            .parse()
            .expect("threshold");
        let request = SwapRequest {
            min_output: threshold + 1,
            ..request(&case)
        };
        let (result, signed) = run(&api, &case, &request).await;
        assert!(matches!(
            refusal(result),
            SwapRefusal::ThresholdBelowMinimum { min_output, .. } if min_output == threshold + 1
        ));
        assert_eq!(api.calls(), (1, 0));
        assert_eq!(signed, 0);
    }

    #[tokio::test]
    async fn a_quote_for_another_swap_is_refused_before_it_is_built() {
        let case = case(HNT_USDC);
        let mut api = FakeApi::new(&case);
        if let Ok(quote) = &mut api.quote {
            quote.in_amount = (case.request.amount - 1).to_string();
        }
        let (result, signed) = run(&api, &case, &request(&case)).await;
        assert!(matches!(
            refusal(result),
            SwapRefusal::QuoteMismatch {
                field: "inAmount",
                ..
            }
        ));
        assert_eq!(api.calls(), (1, 0));
        assert_eq!(signed, 0);
    }

    #[tokio::test]
    async fn anything_but_one_transaction_is_refused_unsigned() {
        let case = case(HNT_USDC);
        for found in [0, 2] {
            let mut api = FakeApi::new(&case);
            api.transactions = vec![api.transactions[0].clone(); found];
            let (result, signed) = run(&api, &case, &request(&case)).await;
            assert!(matches!(
                refusal(result),
                SwapRefusal::TransactionCount { found: refused } if refused == found
            ));
            assert_eq!(signed, 0);
        }
    }

    #[tokio::test]
    async fn a_transaction_that_does_not_decode_is_refused_unsigned() {
        let case = case(HNT_USDC);
        let mut api = FakeApi::new(&case);
        api.transactions = vec![BASE64.encode([1, 2, 3])];
        let (result, signed) = run(&api, &case, &request(&case)).await;
        assert!(
            matches!(result, Err(SwapRouteError::TransactionDecode(_))),
            "{result:?}"
        );
        assert_eq!(signed, 0);
    }

    #[tokio::test]
    async fn a_failed_quote_request_is_reported_as_the_api_error() {
        let case = case(HNT_USDC);
        let mut api = FakeApi::new(&case);
        api.quote = Err(503);
        let (result, signed) = run(&api, &case, &request(&case)).await;
        assert!(
            matches!(
                result,
                Err(SwapRouteError::Api(BlockchainApiError::Api {
                    status: 503,
                    ..
                }))
            ),
            "{result:?}"
        );
        assert_eq!(api.calls(), (1, 0));
        assert_eq!(signed, 0);
    }

    #[tokio::test]
    async fn a_transaction_built_for_another_wallet_is_refused() {
        // The taker is the keypair's wallet, so a transaction whose fee payer
        // is any other wallet is refused for this keypair.
        let case = case(HNT_USDC);
        let api = FakeApi::new(&case);
        let other = Pubkey::new_unique();
        let result = prepare_with(&api, &case.chain, &request(&case), other).await;
        assert!(
            matches!(
                result,
                Err(SwapRouteError::Refused(SwapRefusal::FeePayerNotTaker))
            ),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_prepared_swap_is_unsigned_and_signing_signs_its_message_once() {
        let case = case(HNT_USDC);
        let api = FakeApi::new(&case);
        let prepared = prepare_with(&api, &case.chain, &request(&case), case.request.taker)
            .await
            .expect("prepared");
        assert_eq!(
            prepared.transaction().signatures,
            vec![Signature::default()]
        );
        let message = prepared.transaction().message.clone();
        let signer = CountingSigner::new(&case.request.taker);
        let swap = prepared.sign(&signer).expect("signed");
        assert_eq!(signer.signed(), 1);
        assert_eq!(swap.transaction.message, message);
    }

    #[tokio::test]
    async fn a_prepared_swap_refuses_a_keypair_that_is_not_the_taker() {
        let case = case(HNT_USDC);
        let api = FakeApi::new(&case);
        let prepared = prepare_with(&api, &case.chain, &request(&case), case.request.taker)
            .await
            .expect("prepared");
        let other = Pubkey::new_unique();
        let signer = CountingSigner::new(&other);
        let result = prepared.sign(&signer);
        assert!(
            matches!(
                &result,
                Err(SwapRouteError::SignerNotTaker { signer, taker })
                    if *signer == other && *taker == case.request.taker
            ),
            "{result:?}"
        );
        assert_eq!(signer.signed(), 0);
    }

    #[tokio::test]
    async fn api_errors_carry_no_url() {
        let api = Client::new("http://127.0.0.1:1/SECRET-PATH");
        let case = case(HNT_USDC);
        let err = SwapApi::quote(&api, &request(&case))
            .await
            .map_err(SwapRouteError::api)
            .expect_err("nothing listens on port 1");
        assert!(matches!(err, SwapRouteError::Api(_)), "{err:?}");
        let mut text = format!("{err} {err:?}");
        let mut source = std::error::Error::source(&err);
        while let Some(cause) = source {
            text.push_str(&format!(" {cause} {cause:?}"));
            source = cause.source();
        }
        assert!(
            text.contains("Connect") || text.contains("connect"),
            "{text}"
        );
        assert!(!text.contains("SECRET-PATH"), "{text}");
    }

    #[test]
    fn prepare_future_is_send() {
        fn send<T: Send>(_: T) {}
        let case = case(HNT_USDC);
        let api = Client::new("http://127.0.0.1:1");
        let rpc = std::sync::Arc::new(SolanaRpcClient::new("http://127.0.0.1:1".to_string()));
        // Building the future reaches no network; it is dropped unpolled.
        send(prepare(&api, &rpc, &request(&case), case.request.taker));
    }

    #[test]
    fn the_quote_asks_for_a_v2_route_with_step_shares_in_basis_points() {
        let mut quote: SwapQuote = serde_json::from_value(json!({
            "inputMint": "a",
            "inAmount": "1",
            "outputMint": "b",
            "outAmount": "2",
            "otherAmountThreshold": "1",
            "swapMode": "ExactIn",
            "slippageBps": 100,
            "priceImpactPct": "0",
            "routePlan": [
                { "swapInfo": {}, "percent": 60, "bps": null },
                { "swapInfo": {}, "percent": 40 },
                { "swapInfo": {}, "percent": 100, "bps": 7 },
            ],
        }))
        .expect("quote");
        let checked = (
            quote.input_mint.clone(),
            quote.in_amount.clone(),
            quote.other_amount_threshold.clone(),
        );
        request_v2_route(&mut quote);
        assert_eq!(quote.extra["instructionVersion"], "V2");
        let steps = quote.extra["routePlan"].as_array().expect("steps");
        assert_eq!(steps[0]["bps"], 6_000);
        assert_eq!(steps[1]["bps"], 4_000);
        assert_eq!(steps[2]["bps"], 7);
        assert_eq!(
            checked,
            (
                quote.input_mint,
                quote.in_amount,
                quote.other_amount_threshold
            )
        );
    }
}
