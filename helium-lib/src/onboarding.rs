use crate::{hotspot::HotspotInfoUpdate, keypair};
use futures::TryFutureExt;
use rust_decimal::prelude::*;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_with::serde_as;
use std::marker::Send;

pub struct Client {
    base_url: String,
    inner: reqwest::Client,
}

impl Client {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            inner: reqwest::Client::new(),
        }
    }

    pub async fn get<T>(&self, path: &str) -> Result<T, OnboardingError>
    where
        T: 'static + DeserializeOwned + Send,
    {
        let url = format!("{}{}", &self.base_url, path);
        let resp = self.inner.get(&url).send().await?;
        let onboarding_resp = resp.json::<OnboardingResponse<T>>().await?;
        if !onboarding_resp.success {
            return Err(OnboardingError::from(onboarding_resp));
        }
        onboarding_resp.data.ok_or_else(|| OnboardingError::NoData)
    }

    pub async fn post<T, P>(&self, path: &str, params: &P) -> Result<T, OnboardingError>
    where
        T: 'static + DeserializeOwned + Send,
        P: Serialize + ?Sized,
    {
        let url = format!("{}{}", &self.base_url, path);
        let resp = self.inner.post(&url).json(&params).send().await?;
        let onboarding_resp = resp.json::<OnboardingResponse<T>>().await?;
        if !onboarding_resp.success {
            return Err(OnboardingError::from(onboarding_resp));
        }
        onboarding_resp.data.ok_or_else(|| OnboardingError::NoData)
    }

    pub async fn get_hotspot(
        &self,
        hotspot: &helium_crypto::PublicKey,
    ) -> Result<Hotspot, OnboardingError> {
        self.get::<Hotspot>(&format!("/hotspots/{}", hotspot)).await
    }

    pub async fn get_update_txn(
        &self,
        hotspot: &helium_crypto::PublicKey,
        signer: &keypair::Pubkey,
        update: HotspotInfoUpdate,
    ) -> Result<solana_sdk::transaction::Transaction, OnboardingError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        #[serde_as]
        struct UpdateParams {
            entity_key: helium_crypto::PublicKey,
            #[serde(with = "keypair::serde_pubkey")]
            wallet: keypair::Pubkey,
            #[serde_with(as = "Option<SerializeDisplay>")]
            location: Option<u64>,
            gain: Option<f64>,
            elevation: Option<i32>,
        }

        let params = UpdateParams {
            entity_key: hotspot.clone(),
            wallet: *signer,
            location: update.location().map(Into::into),
            gain: update.gain().and_then(|gain| gain.to_f64()),
            elevation: update.elevation().to_owned(),
        };

        self.post::<OnboardingResponseTransactions, _>(
            &format!("/transactions/{}/update-metadata", update.subdao()),
            &params,
        )
        .and_then(|resp_data| async move {
            bincode::deserialize::<solana_sdk::transaction::Transaction>(
                &resp_data.solana_transactions[0].data,
            )
            .map_err(|_| OnboardingError::InvalidData)
        })
        .await
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all(deserialize = "camelCase"))]
pub struct Hotspot {
    pub maker: Maker,
    pub onboarding_key: helium_crypto::PublicKey,
    pub public_address: helium_crypto::PublicKey,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all(deserialize = "camelCase"))]
pub struct Maker {
    pub id: u32,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub address: crate::keypair::Pubkey,
    pub location_nonce_limit: u16,
    pub name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum OnboardingError {
    #[error("onboarding request: {0}")]
    Client(#[from] reqwest::Error),
    #[error("onboarding response: {code} {reason}")]
    Server { code: u32, reason: String },
    #[error("no data in response")]
    NoData,
    #[error("invalid data in resopnse")]
    InvalidData,
}

impl<T> From<OnboardingResponse<T>> for OnboardingError {
    fn from(value: OnboardingResponse<T>) -> Self {
        Self::Server {
            code: value.code,
            reason: value.error_message.unwrap_or("unknown".to_string()),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingResponse<T> {
    code: u32,
    success: bool,
    error_message: Option<String>,
    data: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardingResponseTransactions {
    solana_transactions: Vec<OnboardingResponseTransaction>,
}

#[derive(Deserialize)]
struct OnboardingResponseTransaction {
    data: Vec<u8>,
}
