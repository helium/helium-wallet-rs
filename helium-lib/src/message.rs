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

pub const COMMON_LUT_DEVNET: Pubkey = pubkey!("FnqYkQ6ZKnVKdkvYCGsEeiP5qgGqVbcFUkGduy2ta4gA");
pub const COMMON_LUT: Pubkey = pubkey!("43eY9L2spbM2b1MPDFFBStUiFGt29ziZ1nc1xbpzsfVt");

pub use solana_sdk::message::VersionedMessage;

pub async fn get_lut_accounts<C: AsRef<SolanaRpcClient>>(
    client: &C,
    addresses: &[Pubkey],
) -> Result<Vec<AddressLookupTableAccount>, Error> {
    use futures::{stream, StreamExt, TryStreamExt};
    let solana_client = AsRef::<SolanaRpcClient>::as_ref(client);
    stream::iter(addresses)
        .map(Ok)
        .map_ok(|address| async move {
            let raw = solana_client.get_account(address).await?;
            let lut = AddressLookupTable::deserialize(&raw.data).map_err(Error::from)?;
            Ok(AddressLookupTableAccount {
                key: *address,
                addresses: lut.addresses.to_vec(),
            })
        })
        .try_buffered(addresses.len().min(5))
        .try_collect()
        .await
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
