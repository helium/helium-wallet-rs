use crate::{
    client::SolanaRpcClient,
    keypair::pubkey,
    solana_sdk::{
        address_lookup_table::{state::AddressLookupTable, AddressLookupTableAccount},
        instruction::Instruction,
        message::v0,
    },
    Error, Pubkey,
};
use itertools::Itertools;

pub const COMMON_LUT_DEVNET: Pubkey = pubkey!("FnqYkQ6ZKnVKdkvYCGsEeiP5qgGqVbcFUkGduy2ta4gA");
pub const COMMON_LUT: Pubkey = pubkey!("43eY9L2spbM2b1MPDFFBStUiFGt29ziZ1nc1xbpzsfVt");

pub use solana_sdk::message::VersionedMessage;

pub async fn get_lut_accounts<C: AsRef<SolanaRpcClient>>(
    client: &C,
    addresses: &[Pubkey],
) -> Result<Vec<AddressLookupTableAccount>, Error> {
    itertools::izip!(
        addresses,
        client.as_ref().get_multiple_accounts(addresses).await?
    )
    .filter_map(|(address, maybe_account)| {
        maybe_account.map(|account| {
            AddressLookupTable::deserialize(&account.data)
                .map_err(Error::from)
                .map(|lut| AddressLookupTableAccount {
                    key: *address,
                    addresses: lut.addresses.to_vec(),
                })
        })
    })
    .try_collect()
}

pub async fn mk_message<C: AsRef<SolanaRpcClient>>(
    client: &C,
    ixs: &[Instruction],
    lut_accounts: &[Pubkey],
    payer: &Pubkey,
) -> Result<(VersionedMessage, u64), Error> {
    let solana_client = AsRef::<SolanaRpcClient>::as_ref(client);
    let lut_accounts = get_lut_accounts(client, lut_accounts).await?;
    let (recent_blockhash, recent_blockheight) = solana_client
        .get_latest_blockhash_with_commitment(solana_client.commitment())
        .await?;
    let msg = VersionedMessage::V0(v0::Message::try_compile(
        payer,
        ixs,
        &lut_accounts,
        recent_blockhash,
    )?);
    Ok((msg, recent_blockheight))
}
