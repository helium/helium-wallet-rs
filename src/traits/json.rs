use crate::{
    keypair::PublicKey,
    result::{anyhow, Result},
    traits::B64,
};
use helium_proto::*;
use serde_json::json;

pub(crate) fn maybe_b58(data: &[u8]) -> Result<Option<String>> {
    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PublicKey::from_bytes(data)?.to_string()))
    }
}

pub(crate) fn maybe_b64_url(data: &[u8]) -> Result<Option<String>> {
    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some(data.to_vec().to_b64_url()?))
    }
}

pub trait ToJson {
    fn to_json(&self) -> Result<serde_json::Value>;
}

impl<T> ToJson for Vec<T>
where
    T: ToJson,
{
    fn to_json(&self) -> Result<serde_json::Value> {
        let mut seq = Vec::with_capacity(self.len());
        for entry in self {
            seq.push(entry.to_json()?)
        }
        Ok(json!(seq))
    }
}

fn vec_to_strings(vec: &[Vec<u8>]) -> Result<Vec<String>> {
    let mut seq = Vec::with_capacity(vec.len());
    for entry in vec {
        seq.push(String::from_utf8(entry.to_vec())?);
    }
    Ok(seq)
}

fn vec_to_b58s(vec: &[Vec<u8>]) -> Result<Vec<String>> {
    let mut seq = Vec::with_capacity(vec.len());
    for entry in vec {
        seq.push(PublicKey::from_bytes(entry)?.to_string());
    }
    Ok(seq)
}

fn vec_to_b64_urls(vec: &[Vec<u8>]) -> Result<Vec<String>> {
    let mut seq = Vec::with_capacity(vec.len());
    for entry in vec {
        seq.push(entry.to_b64_url()?);
    }
    Ok(seq)
}

impl ToJson for BlockchainTxnVarsV1 {
    fn to_json(&self) -> Result<serde_json::Value> {
        let map = json!({
            "type": "vars_v1",
            "version_predicate": self.version_predicate,
            "nonce": self.nonce,
            "proof": maybe_b64_url(&self.proof)?,
            "master_key": maybe_b58(&self.master_key)?,
            "key_proof": maybe_b64_url(&self.key_proof)?,
            "vars": self.vars.to_json()?,
            "unsets": vec_to_strings(&self.unsets)?,
            "cancels": vec_to_strings(&self.cancels)?,
            "multi_keys": vec_to_b58s(&self.multi_keys)?,
            "multi_proofs": vec_to_b64_urls(&self.multi_proofs)?,
            "multi_key_proofs": vec_to_b64_urls(&self.multi_key_proofs)?,
        });
        Ok(map)
    }
}

impl ToJson for BlockchainVarV1 {
    fn to_json(&self) -> Result<serde_json::Value> {
        let value = match &self.r#type[..] {
            "int" => {
                let v: i64 = String::from_utf8(self.value.to_vec())?.parse::<i64>()?;
                json!(v)
            }
            "float" => {
                let v: f64 = String::from_utf8(self.value.to_vec())?.parse::<f64>()?;
                json!(v)
            }
            "string" | "atom" => {
                let v: String = String::from_utf8(self.value.to_vec())?;
                json!(v)
            }
            _ => return Err(anyhow!("Invalid variable {:?}", self)),
        };
        Ok(json!({
            "name": self.name,
            "type": self.r#type,
            "value": value
        }))
    }
}
