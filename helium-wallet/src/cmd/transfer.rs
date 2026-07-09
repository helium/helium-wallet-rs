use crate::{
    cmd::{squads::SquadsOpts, *},
    result::Result,
};
use helium_lib::{
    blockchain_api::types::{
        ApiTransactions, MultiTransferRequest, Recipient, TokenAmountInput, TokenTransferRequest,
    },
    keypair::{serde_pubkey, Pubkey, Signer},
    programs::KnownProgram,
    solana_sdk::transaction::VersionedTransaction,
    token::{Token, TokenAmount},
};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: PayCmd,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        self.cmd.run(opts).await
    }
}

#[derive(Debug, clap::Subcommand)]
/// Send one (or more) HNT payments to given addresses.
///
/// The payment is not submitted to the system unless the '--commit' option is
/// given.
pub enum PayCmd {
    /// Pay a single payee in HNT (8 decimals of precision).
    One(One),
    /// Pay multiple payees in HNT
    Multi(Multi),
}

#[derive(Debug, clap::Args)]
pub struct One {
    #[command(flatten)]
    payee: Payee,
    #[command(flatten)]
    squads: SquadsOpts,
    /// Commit the payment to the API
    #[command(flatten)]
    commit: CommitOpts,
}

/// Multiple payment descriptor file
///
/// The input file for multiple payments is expected to be a json file with a
/// list of payees and amounts. All payments are in HNT.
/// Notes:
///   "address" is required.
///   "amount" is required and must be a positive number.
///
/// For example:
///
/// [
///     {
///         "address": "<address1>",
///         "amount": 1.6
///     },
///     {
///         "address": "<address2>",
///         "amount": 3
///     }
/// ]
///
#[derive(Debug, clap::Args)]
pub struct Multi {
    /// File to read multiple payments from.
    path: PathBuf,
    #[command(flatten)]
    squads: SquadsOpts,
    /// Commit the payments
    #[command(flatten)]
    commit: CommitOpts,
}

impl PayCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let payments = self.collect_payments()?;
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let squads = self.squads();

        // Transfers build via the blockchain-api. A single recipient goes
        // through tokens/transfer (as a Squads vault proposal when --squads is
        // set); multiple recipients go through the single-mint multi-transfer
        // endpoint. --squads supports a single recipient only. All transfers
        // are HNT.
        let api = opts.blockchain_api()?;
        let wallet = signer.pubkey();
        let wallet_address = wallet.to_string();
        // A --squads transfer is built as a multisig vault proposal, which wraps
        // the transfer inside the proposal rather than as a top-level SPL
        // transfer, so the decode-and-assert below does not apply to it.
        let mut is_proposal = false;
        let response = match payments.as_slice() {
            [(destination, amount)] => {
                let (multisig, memo) = squads.resolve(&client, &wallet).await?;
                is_proposal = multisig.is_some();
                api.token_transfer(&TokenTransferRequest {
                    wallet_address,
                    destination: destination.to_string(),
                    token_amount: TokenAmountInput::new(amount.token.mint(), amount.amount),
                    multisig,
                    memo,
                })
                .await?
            }
            recipients => {
                if squads.squads.is_some() {
                    bail!("--squads supports a single recipient only");
                }
                let recipients = recipients
                    .iter()
                    .map(|(destination, amount)| Recipient {
                        destination: destination.to_string(),
                        amount: amount.amount.to_string(),
                    })
                    .collect();
                api.multi_transfer(&MultiTransferRequest {
                    wallet_address,
                    mint: Token::Hnt.mint().to_string(),
                    recipients,
                })
                .await?
            }
        };

        // Before signing, independently decode the server-built transaction(s)
        // and confirm they move exactly the HNT we asked for, to the recipients
        // we asked for — a compromised or buggy server cannot redirect or alter
        // funds without this failing first.
        if !is_proposal {
            let expected: Vec<ExpectedTransfer> = payments
                .iter()
                .map(|(recipient, amount)| ExpectedTransfer {
                    recipient: *recipient,
                    amount: amount.amount,
                })
                .collect();
            let unsigned = response.transaction_data().decode_transactions()?;
            assert_hnt_transfers(&unsigned, &wallet, &expected)?;
        }

        print_json(
            &self
                .commit()
                .commit_via_api(
                    &api,
                    &client,
                    &response,
                    &*signer,
                    ApiSigning::FreshBlockhash,
                )
                .await?
                .to_json(),
        )
    }

    fn squads(&self) -> &SquadsOpts {
        match &self {
            Self::One(one) => &one.squads,
            Self::Multi(multi) => &multi.squads,
        }
    }

    fn collect_payments(&self) -> Result<Vec<(Pubkey, TokenAmount)>> {
        match &self {
            Self::One(one) => Ok(vec![(one.payee.address, one.payee.token_amount()?)]),
            Self::Multi(multi) => {
                let file = std::fs::File::open(multi.path.clone())?;
                let payees: Vec<Payee> = serde_json::from_reader(file)?;
                payees
                    .iter()
                    .map(|p| Ok((p.address, p.token_amount()?)))
                    .collect()
            }
        }
    }

    fn commit(&self) -> &CommitOpts {
        match &self {
            Self::One(one) => &one.commit,
            Self::Multi(multi) => &multi.commit,
        }
    }
}

// deny_unknown_fields so a typo'd or unsupported key in a multi-payment
// file (e.g. "token", dropped when transfers became HNT-only, or "memo",
// which the old doc advertised but was silently dropped) is an error
// instead of being ignored.
#[derive(Debug, Deserialize, clap::Args)]
#[serde(deny_unknown_fields)]
pub struct Payee {
    /// Address to send the HNT to.
    #[serde(with = "serde_pubkey")]
    address: Pubkey,
    /// Amount of HNT to send
    amount: f64,
}

impl Payee {
    pub fn token_amount(&self) -> Result<TokenAmount> {
        Ok(TokenAmount::from_f64(Token::Hnt, self.amount)?)
    }
}

/// One payment the CLI asked the blockchain-api to build: the recipient's
/// wallet and the raw (base-unit) HNT amount.
struct ExpectedTransfer {
    recipient: Pubkey,
    amount: u64,
}

/// Independently decode the server-built transfer transaction(s) and confirm
/// they move exactly the HNT the CLI was asked to send — the expected amount to
/// each recipient's HNT associated-token account, out of this wallet's own HNT
/// account, and nothing else. Aborts before signing on any mismatch so a
/// compromised or buggy server cannot redirect or alter funds.
///
/// Soundness without resolving lookup tables: the accounts that pin the outcome
/// (this wallet's HNT ATA as the source, the wallet as the authority, and each
/// recipient's HNT ATA as the destination) are all wallet-specific and are
/// never packed into the shared Helium lookup table, so they are always static
/// keys. Requiring the source to be this wallet's HNT ATA fixes the mint to HNT
/// without reading the (possibly table-loaded) mint account. If any account we
/// must check is table-loaded, or any non-transfer SPL-token instruction is
/// present, we fail closed rather than sign something we cannot verify.
fn assert_hnt_transfers(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    expected: &[ExpectedTransfer],
) -> Result<()> {
    let spl_token = KnownProgram::SplToken.id();
    let sender_ata = Token::Hnt.associated_token_address(wallet);

    // Expected destination ATA -> total raw amount (summed so a recipient listed
    // more than once still balances).
    let mut want: HashMap<Pubkey, u64> = HashMap::new();
    for transfer in expected {
        let dest_ata = Token::Hnt.associated_token_address(&transfer.recipient);
        *want.entry(dest_ata).or_default() += transfer.amount;
    }

    let mut got: HashMap<Pubkey, u64> = HashMap::new();
    for tx in unsigned {
        let keys = tx.message.static_account_keys();
        let resolve = |slot: usize, accounts: &[u8]| -> Result<Pubkey> {
            let account_index = *accounts
                .get(slot)
                .ok_or_else(|| anyhow!("SPL-token transfer is missing an account"))?
                as usize;
            keys.get(account_index).copied().ok_or_else(|| {
                anyhow!("transfer references a lookup-table account; cannot verify it safely")
            })
        };
        for ix in tx.message.instructions() {
            let program = keys
                .get(ix.program_id_index as usize)
                .copied()
                .ok_or_else(|| {
                    anyhow!("transfer references a lookup-table program; cannot verify it safely")
                })?;
            if program != spl_token {
                continue;
            }
            // The only SPL-token instruction a transfer should contain is a
            // (checked) transfer; account layouts are [source, dest, authority]
            // for Transfer (tag 3) and [source, mint, dest, authority] for
            // TransferChecked (tag 12). Anything else — burn, approve, close,
            // set-authority — could move or destroy funds, so refuse to sign.
            let (source, dest, owner) = match ix.data.first() {
                Some(3) => (
                    resolve(0, &ix.accounts)?,
                    resolve(1, &ix.accounts)?,
                    resolve(2, &ix.accounts)?,
                ),
                Some(12) => (
                    resolve(0, &ix.accounts)?,
                    resolve(2, &ix.accounts)?,
                    resolve(3, &ix.accounts)?,
                ),
                _ => bail!("transfer transaction contains an unexpected SPL-token instruction"),
            };
            if owner != *wallet {
                bail!("transfer is authorized by {owner}, not this wallet");
            }
            if source != sender_ata {
                bail!("transfer moves funds from {source}, not this wallet's HNT account");
            }
            *got.entry(dest).or_default() += transfer_amount(&ix.data)?;
        }
    }

    if got != want {
        bail!(
            "the server-built transfer does not match the requested payment(s); \
             refusing to sign. expected {}, decoded {}",
            format_transfers(&want),
            format_transfers(&got),
        );
    }

    for transfer in expected {
        let dest_ata = Token::Hnt.associated_token_address(&transfer.recipient);
        eprintln!(
            "→ verified: {} HNT to {} (token account {dest_ata})",
            f64::from(&TokenAmount {
                token: Token::Hnt,
                amount: transfer.amount,
            }),
            transfer.recipient,
        );
    }
    Ok(())
}

/// Read the little-endian `u64` amount that both Transfer and TransferChecked
/// carry immediately after their one-byte tag.
fn transfer_amount(data: &[u8]) -> Result<u64> {
    let bytes: [u8; 8] = data
        .get(1..9)
        .ok_or_else(|| anyhow!("SPL-token transfer has a truncated amount"))?
        .try_into()
        .expect("slice of length 8");
    Ok(u64::from_le_bytes(bytes))
}

/// Render a destination-ATA -> raw-amount map as a stable, sorted string for
/// mismatch errors.
fn format_transfers(transfers: &HashMap<Pubkey, u64>) -> String {
    let mut entries: Vec<_> = transfers
        .iter()
        .map(|(dest, amount)| format!("{amount} to {dest}"))
        .collect();
    entries.sort();
    format!("[{}]", entries.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helium_lib::solana_sdk::{
        instruction::{AccountMeta, Instruction},
        message::{Message, VersionedMessage},
    };

    /// Build a `VersionedTransaction` (legacy message, payer = `owner`) carrying
    /// a single SPL-token instruction with the given tag and account layout.
    /// `tag` 12 lays out [source, mint, dest, owner]; anything else lays out
    /// [source, dest, owner] like a legacy Transfer/Burn.
    fn spl_tx(
        tag: u8,
        amount: u64,
        source: Pubkey,
        dest: Pubkey,
        owner: Pubkey,
        program: Pubkey,
    ) -> VersionedTransaction {
        let mut data = vec![tag];
        data.extend_from_slice(&amount.to_le_bytes());
        let accounts = if tag == 12 {
            data.push(8); // decimals
            vec![
                AccountMeta::new(source, false),
                AccountMeta::new_readonly(Pubkey::new_unique(), false), // mint
                AccountMeta::new(dest, false),
                AccountMeta::new_readonly(owner, true),
            ]
        } else {
            vec![
                AccountMeta::new(source, false),
                AccountMeta::new(dest, false),
                AccountMeta::new_readonly(owner, true),
            ]
        };
        let ix = Instruction {
            program_id: program,
            accounts,
            data,
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&owner))),
        }
    }

    fn ata(owner: &Pubkey) -> Pubkey {
        Token::Hnt.associated_token_address(owner)
    }

    #[test]
    fn transfer_assert_accepts_matching_transfer() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let tx = spl_tx(
            12,
            250,
            ata(&wallet),
            ata(&recipient),
            wallet,
            KnownProgram::SplToken.id(),
        );
        assert!(assert_hnt_transfers(
            &[tx],
            &wallet,
            &[ExpectedTransfer {
                recipient,
                amount: 250
            }]
        )
        .is_ok());
    }

    #[test]
    fn transfer_assert_rejects_redirected_destination() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let attacker = Pubkey::new_unique();
        // Server sends the right amount to the wrong recipient's ATA.
        let tx = spl_tx(
            12,
            250,
            ata(&wallet),
            ata(&attacker),
            wallet,
            KnownProgram::SplToken.id(),
        );
        assert!(assert_hnt_transfers(
            &[tx],
            &wallet,
            &[ExpectedTransfer {
                recipient,
                amount: 250
            }]
        )
        .is_err());
    }

    #[test]
    fn transfer_assert_rejects_altered_amount() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let tx = spl_tx(
            12,
            999,
            ata(&wallet),
            ata(&recipient),
            wallet,
            KnownProgram::SplToken.id(),
        );
        assert!(assert_hnt_transfers(
            &[tx],
            &wallet,
            &[ExpectedTransfer {
                recipient,
                amount: 250
            }]
        )
        .is_err());
    }

    #[test]
    fn transfer_assert_rejects_foreign_source() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        // Funds leave an account that is not this wallet's HNT ATA.
        let tx = spl_tx(
            12,
            250,
            ata(&other),
            ata(&recipient),
            wallet,
            KnownProgram::SplToken.id(),
        );
        assert!(assert_hnt_transfers(
            &[tx],
            &wallet,
            &[ExpectedTransfer {
                recipient,
                amount: 250
            }]
        )
        .is_err());
    }

    #[test]
    fn transfer_assert_rejects_non_transfer_token_op() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        // A burn (tag 8) smuggled in on the token program must be refused.
        let tx = spl_tx(
            8,
            250,
            ata(&wallet),
            ata(&recipient),
            wallet,
            KnownProgram::SplToken.id(),
        );
        assert!(assert_hnt_transfers(
            &[tx],
            &wallet,
            &[ExpectedTransfer {
                recipient,
                amount: 250
            }]
        )
        .is_err());
    }

    #[test]
    fn transfer_assert_rejects_missing_transfer() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        // A transaction that moves nothing (only a non-token instruction) must
        // not pass when a payment was expected.
        let ix = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![AccountMeta::new_readonly(wallet, true)],
            data: vec![0],
        };
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&wallet))),
        };
        assert!(assert_hnt_transfers(
            &[tx],
            &wallet,
            &[ExpectedTransfer {
                recipient,
                amount: 250
            }]
        )
        .is_err());
    }

    #[test]
    fn test_json_hnt_input() {
        let json_hnt_input = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": 1.6\
        }";

        let payee: Payee = serde_json::from_str(json_hnt_input).expect("payee");
        assert_eq!(
            TokenAmount {
                amount: 160_000_000,
                token: Token::Hnt
            },
            payee.token_amount().expect("token amount")
        );
    }

    #[test]
    fn test_json_rejects_token_field() {
        // Transfers are HNT-only; a leftover "token" key must be rejected
        // rather than silently ignored (deny_unknown_fields).
        let json_with_token = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": 0.5,\
            \"token\": \"mobile\"\
        }";

        let result: std::result::Result<Payee, serde_json::Error> =
            serde_json::from_str(json_with_token);
        assert!(result.is_err());
    }

    #[test]
    fn test_json_bad_amount() {
        let json_hnt_input = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": \"foo\"\
        }";

        let result: std::result::Result<Payee, serde_json::Error> =
            serde_json::from_str(json_hnt_input);
        assert!(result.is_err());
    }
}
