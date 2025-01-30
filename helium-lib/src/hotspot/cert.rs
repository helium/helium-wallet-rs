use crate::{
    b64,
    client::CERT_URL_MAINNET,
    error::{DecodeError, EncodeError},
    keypair::Keypair,
    solana_sdk::signer::Signer,
    Error, Pubkey,
};
use chrono::{DateTime, Utc};
use futures::TryFutureExt;
use helium_crypto::PublicKey;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt::{self, Debug};

pub async fn get<C: AsRef<Client>>(
    client: C,
    hotspot: PublicKey,
    keypair: &Keypair,
) -> Result<CertResponse, Error> {
    get_or_create(client, None, hotspot, keypair, false).await
}

pub async fn get_or_create<C: AsRef<Client>>(
    client: C,
    location_info: Option<LocationInfo>,
    hotspot: PublicKey,
    keypair: &Keypair,
    dry_run: bool,
) -> Result<CertResponse, Error> {
    let location = LocationData::for_info(location_info, hotspot, keypair.pubkey());
    let req = CertRequest::for_location(location, keypair, dry_run)?;
    client
        .as_ref()
        .post("/v1/locations/residential", &req)
        .map_err(Error::from)
        .await
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertRequest {
    /// Base64 encoded version of a serialized LocationData
    pub location_data: String,
    /// Signature of the location_bytes
    pub signature: String,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertResponse {
    #[serde(flatten)]
    pub location: LocationInfo,
    #[serde(flatten)]
    pub cert: CertInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CertInfo {
    pub radsec_private_key: String,
    pub radsec_certificate: String,
    pub radsec_cert_expire: DateTime<Utc>,
}

impl CertRequest {
    pub fn for_location(
        data: LocationData,
        keypair: &Keypair,
        dry_run: bool,
    ) -> Result<Self, Error> {
        let location_data = b64::encode(serde_json::to_string(&data).map_err(EncodeError::from)?);
        let signature = b64::encode(keypair.sign(location_data.as_bytes())?);
        Ok(Self {
            location_data,
            signature,
            dry_run,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LocationData {
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub info: Option<LocationInfo>,
    #[serde(with = "crate::keypair::serde_pubkey")]
    pub wallet: Pubkey,
    pub blockchain_pubkey: PublicKey,
    pub timestamp: DateTime<Utc>,
}

impl LocationData {
    fn for_info(info: Option<LocationInfo>, hotspot: PublicKey, owner: Pubkey) -> Self {
        Self {
            info,
            wallet: owner,
            blockchain_pubkey: hotspot,
            timestamp: Utc::now(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LocationInfo {
    pub location_address: String,
    pub location_lat: f64,
    pub location_lon: f64,
    /// NAS ID. Only one nas_id is supported
    pub nas_ids: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Request(#[from] RequestError),
    #[error("encode: {0}")]
    Encode(#[from] crate::error::EncodeError),
}

#[derive(Debug)]
pub struct RequestError {
    pub error: reqwest::Error,
    pub message: Option<String>,
}

impl std::error::Error for RequestError {}

impl From<reqwest::Error> for RequestError {
    fn from(value: reqwest::Error) -> Self {
        Self {
            error: value,
            message: None,
        }
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(value: reqwest::Error) -> Self {
        RequestError::from(value).into()
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(message) = &self.message {
            write!(f, "cert service: {message}")
        } else {
            fmt::Display::fmt(&self.error, f)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    inner: reqwest::Client,
    base_url: reqwest::Url,
    token: Option<String>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new(CERT_URL_MAINNET, None).unwrap()
    }
}

impl Client {
    pub fn new(url: &str, token: Option<String>) -> Result<Self, Error> {
        let inner = reqwest::Client::new();
        let base_url = url.parse().map_err(DecodeError::from)?;
        Ok(Self {
            inner,
            base_url,
            token,
        })
    }

    pub async fn post<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R, ClientError> {
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.token {
            headers.insert(
                HeaderName::from_static("x-internal-api-key"),
                HeaderValue::from_str(token)
                    .map_err(|_| EncodeError::other("invalid api token format"))?,
            );
        }
        let response = self
            .inner
            .post(format!("{}{}", &self.base_url, path))
            .json(body)
            .headers(headers)
            .send()
            .await?;
        match response.error_for_status_ref() {
            Ok(_) => Ok(response.json::<R>().await?),
            Err(err) => {
                #[derive(Debug, Deserialize)]
                struct ErrorMessage {
                    message: String,
                }
                let mut request_err = RequestError::from(err);
                if let Ok(error_message) = response.json::<ErrorMessage>().await {
                    request_err.message = Some(error_message.message);
                }
                Err(request_err.into())
            }
        }
    }
}
