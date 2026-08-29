use crate::{
    cmd::{squads::SquadsOpts, *},
    result::Result,
};
use helium_lib::{
    anchor_spl::token::spl_token,
    blockchain_api::types::{
        ApiTransactions, MultiTransferRequest, Recipient, TokenAmountInput, TokenTransferRequest,
    },
    keypair::{serde_pubkey, Pubkey, Signer},
    programs::KnownProgram,
    solana_sdk::transaction::VersionedTransaction,
    token::{Token, TokenAmount},
    verify,
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
/// Send one (or more) token payments to given addresses.
///
/// Supported tokens: iot, mobile, hnt, sol, usdc. The payment is not submitted
/// unless the '--commit' option is given.
pub enum PayCmd {
    /// Pay a single payee in the given token.
    One(One),
    /// Pay multiple payees in a single token.
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
/// The input file is a json list of payees and amounts; every payee is paid in
/// the single token given by `--token` (mixing tokens in one batch is not
/// supported). Notes:
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
    /// Token to send to every payee (iot, mobile, hnt, sol, usdc).
    #[arg(long, default_value_t = Token::Hnt, value_parser = Token::transferrable_value_parser)]
    token: Token,
    #[command(flatten)]
    squads: SquadsOpts,
    /// Commit the payments
    #[command(flatten)]
    commit: CommitOpts,
}

impl PayCmd {
    pub async fn run(&self, opts: Opts) -> Result {
        let payments = self.collect_payments()?;
        // A batch is a single token (mixing is unsupported); take it from the
        // first payment and drive both the request mint and the assert with it.
        let token = payments
            .first()
            .ok_or_else(|| anyhow!("no payments to send"))?
            .1
            .token;
        let signer = opts.load_signer()?;
        let client = opts.client()?;
        let squads = self.squads();

        // Transfers build via the blockchain-api. A single recipient goes
        // through tokens/transfer (as a Squads vault proposal when --squads is
        // set); multiple recipients go through the single-mint multi-transfer
        // endpoint. --squads supports a single recipient only.
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
                    token_amount: TokenAmountInput::new(
                        &transfer_mint(amount.token),
                        amount.amount,
                    ),
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
                    mint: transfer_mint(token).to_string(),
                    recipients,
                })
                .await?
            }
        };

        // Before signing, independently decode the server-built transaction(s).
        // A direct transfer must move exactly the tokens asked for to the
        // recipients asked for. A proposal wraps the transfer inside a Squads
        // instruction this cannot read, so it is instead held to moving nothing
        // at the top level: without that, a plain SPL transfer returned in place
        // of a proposal would drain the proposer's own wallet unverified.
        if is_proposal {
            assert_wraps_no_transfer(&response.decode_transactions()?)?;
        }
        if !is_proposal {
            let expected: Vec<ExpectedTransfer> = payments
                .iter()
                .map(|(recipient, amount)| ExpectedTransfer {
                    recipient: *recipient,
                    amount: amount.amount,
                })
                .collect();
            let unsigned = response.transaction_data().decode_transactions()?;
            assert_transfers(&unsigned, &wallet, token, &expected)?;
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
                let payees: Vec<MultiPayee> = serde_json::from_reader(file)?;
                payees
                    .iter()
                    .map(|p| Ok((p.address, TokenAmount::from_f64(multi.token, p.amount)?)))
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

#[derive(Debug, clap::Args)]
pub struct Payee {
    /// Address to send tokens to.
    address: Pubkey,
    /// Amount of the token to send.
    amount: f64,
    /// Token to send (iot, mobile, hnt, sol, usdc). Defaults to hnt.
    #[arg(default_value_t = Token::Hnt, value_parser = Token::transferrable_value_parser)]
    token: Token,
}

impl Payee {
    pub fn token_amount(&self) -> Result<TokenAmount> {
        Ok(TokenAmount::from_f64(self.token, self.amount)?)
    }
}

// One entry in a multi-payment file. The token is chosen per-batch via
// `--token`, so it is not a field here. deny_unknown_fields so a typo'd or
// unsupported key (e.g. a per-payee "token", which mixed-token batches would
// need but the single-mint endpoint does not support) is an error rather than
// silently ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MultiPayee {
    #[serde(with = "serde_pubkey")]
    address: Pubkey,
    amount: f64,
}

/// The mint the blockchain-api expects for `token`. helium-lib models native
/// SOL's "mint" as the system program, but the API keys SOL transfers off the
/// wrapped-SOL mint, so translate here.
fn transfer_mint(token: Token) -> Pubkey {
    match token {
        Token::Sol => spl_token::native_mint::ID,
        other => *other.mint(),
    }
}

/// One payment the CLI asked the blockchain-api to build: the recipient's
/// wallet and the raw (base-unit) amount.
struct ExpectedTransfer {
    recipient: Pubkey,
    amount: u64,
}

/// Independently decode the server-built transfer transaction(s) and confirm
/// they move exactly the tokens the CLI was asked to send, to the recipients we
/// asked for, and nothing else. Aborts before signing on any mismatch so a
/// compromised or buggy server cannot redirect or alter funds. Native SOL is a
/// system-program transfer; every other token is an SPL transfer.
fn assert_transfers(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    token: Token,
    expected: &[ExpectedTransfer],
) -> Result<()> {
    match token {
        Token::Sol => assert_sol_transfers(unsigned, wallet, expected),
        _ => assert_spl_transfers(unsigned, wallet, token, expected),
    }
}

/// Verify an SPL-token transfer batch for `token`: the expected amount to each
/// recipient's `token` associated-token account, out of this wallet's own
/// `token` account, and nothing else.
///
/// Soundness without resolving lookup tables: the accounts that pin the outcome
/// (this wallet's `token` ATA as the source, the wallet as the authority, and
/// each recipient's `token` ATA as the destination) are wallet-specific and are
/// never packed into the shared Helium lookup table, so they are always static
/// keys. Requiring the source to be this wallet's `token` ATA fixes the mint
/// without reading the (possibly table-loaded) mint account. If any account we
/// must check is table-loaded, or any unexpected program or non-transfer
/// SPL-token instruction is present, we fail closed.
fn assert_spl_transfers(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    token: Token,
    expected: &[ExpectedTransfer],
) -> Result<()> {
    let spl_token = KnownProgram::SplToken.id();
    // A transfer transaction legitimately touches only these programs: the
    // token program (the transfer itself), the associated-token program
    // (idempotent creation of a destination ATA), and the compute-budget
    // program. Any other program — including the system program (a bundled SOL
    // transfer) or an attacker program that CPIs a token transfer — is rejected
    // below.
    //
    // The compute-budget instructions bound the SOL this can cost, which is
    // checked where every command inherits it (`CommitOpts::commit_via_api`)
    // rather than here.
    let compute_budget = KnownProgram::ComputeBudget.id();
    let associated_token = KnownProgram::SplAssociatedToken.id();
    let sender_ata = token.associated_token_address(wallet);

    // Expected destination ATA -> total raw amount (summed so a recipient listed
    // more than once still balances).
    let mut want: HashMap<Pubkey, u64> = HashMap::new();
    for transfer in expected {
        let dest_ata = token.associated_token_address(&transfer.recipient);
        *want.entry(dest_ata).or_default() += transfer.amount;
    }

    let mut got: HashMap<Pubkey, u64> = HashMap::new();
    for tx in unsigned {
        for ix in tx.message.instructions() {
            let program = verify::instruction_program(tx, ix)?;
            if program == compute_budget {
                continue;
            }
            if program == associated_token {
                // Tag 1 is CreateIdempotent, the only reason a transfer creates
                // an account. Tag 0 (Create) funds a new ATA from the signer at
                // rent cost, for any owner the caller picks, which the amount
                // comparison below does not see.
                match ix.data.first() {
                    Some(1) => continue,
                    other => bail!(
                        "transfer creates an associated token account \
                         (instruction {other:?}) rather than creating one idempotently; \
                         refusing to sign"
                    ),
                }
            }
            if program != spl_token {
                bail!(
                    "transfer transaction invokes an unexpected program ({program}); \
                     refusing to sign"
                );
            }
            // The only SPL-token instruction a transfer should contain is a
            // (checked) transfer; account layouts are [source, dest, authority]
            // for Transfer (tag 3) and [source, mint, dest, authority] for
            // TransferChecked (tag 12). Anything else — burn, approve, close,
            // set-authority — could move or destroy funds, so refuse to sign.
            let account = |slot: usize| verify::instruction_account(tx, ix, slot);
            let (source, dest, owner) = match ix.data.first() {
                Some(3) => (account(0)?, account(1)?, account(2)?),
                Some(12) => (account(0)?, account(2)?, account(3)?),
                _ => bail!("transfer transaction contains an unexpected SPL-token instruction"),
            };
            if owner != *wallet {
                bail!("transfer is authorized by {owner}, not this wallet");
            }
            if source != sender_ata {
                bail!("transfer moves funds from {source}, not this wallet's {token} account");
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
        let dest_ata = token.associated_token_address(&transfer.recipient);
        eprintln!(
            "→ verified: {} {token} to {} (token account {dest_ata})",
            f64::from(&TokenAmount {
                token,
                amount: transfer.amount,
            }),
            transfer.recipient,
        );
    }
    Ok(())
}

/// Verify a Squads proposal moves nothing at the top level.
///
/// The proposal encodes the real transfer as data inside a Squads instruction,
/// which this does not decode; what it can establish is that the outer
/// transaction is a proposal and not a transfer in its own right. Any SPL-token
/// or system-program instruction at the top level means the response is not the
/// proposal that was asked for.
fn assert_wraps_no_transfer(unsigned: &[VersionedTransaction]) -> Result<()> {
    let compute_budget = KnownProgram::ComputeBudget.id();
    for tx in unsigned {
        for ix in tx.message.instructions() {
            let program = verify::instruction_program(tx, ix)?;
            if program == compute_budget {
                continue;
            }
            if program == KnownProgram::SplToken.id()
                || program == helium_lib::solana_sdk::system_program::id()
            {
                bail!(
                    "a Squads proposal must not move funds at the top level, but this \
                     transaction invokes {program}; refusing to sign"
                );
            }
        }
    }
    Ok(())
}

/// Verify a native-SOL transfer batch: each system-program transfer moves the
/// expected lamports from this wallet to the requested recipient, and the
/// transaction contains no other fund-moving instruction. Only the system
/// program (the transfer) and the compute-budget program are allowed.
fn assert_sol_transfers(
    unsigned: &[VersionedTransaction],
    wallet: &Pubkey,
    expected: &[ExpectedTransfer],
) -> Result<()> {
    let system = KnownProgram::SystemProgram.id();
    let compute_budget = KnownProgram::ComputeBudget.id();

    let mut want: HashMap<Pubkey, u64> = HashMap::new();
    for transfer in expected {
        *want.entry(transfer.recipient).or_default() += transfer.amount;
    }

    let mut got: HashMap<Pubkey, u64> = HashMap::new();
    for tx in unsigned {
        for ix in tx.message.instructions() {
            let program = verify::instruction_program(tx, ix)?;
            if program == compute_budget {
                continue;
            }
            if program != system {
                bail!("SOL transfer invokes an unexpected program ({program}); refusing to sign");
            }
            // system-program Transfer: 4-byte little-endian instruction index 2,
            // then the u64 lamports; accounts are [from, to]. Reject any other
            // system instruction (create-account, assign, ...).
            if ix.data.len() < 12 || u32::from_le_bytes(ix.data[0..4].try_into().unwrap()) != 2 {
                bail!("SOL transfer contains an unexpected system-program instruction");
            }
            let from = verify::instruction_account(tx, ix, 0)?;
            let to = verify::instruction_account(tx, ix, 1)?;
            if from != *wallet {
                bail!("SOL transfer moves funds from {from}, not this wallet");
            }
            let lamports =
                u64::from_le_bytes(ix.data[4..12].try_into().expect("slice of length 8"));
            *got.entry(to).or_default() += lamports;
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
        eprintln!(
            "→ verified: {} SOL to {}",
            f64::from(&TokenAmount {
                token: Token::Sol,
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
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Hnt,
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
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Hnt,
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
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Hnt,
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
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Hnt,
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
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Hnt,
            &[ExpectedTransfer {
                recipient,
                amount: 250
            }]
        )
        .is_err());
    }

    /// A correct HNT transfer with an extra associated-token instruction of the
    /// given tag bundled in.
    fn transfer_with_ata_ix(tag: u8, wallet: Pubkey, payee: Pubkey) -> VersionedTransaction {
        let transfer = Instruction {
            program_id: KnownProgram::SplToken.id(),
            accounts: vec![
                AccountMeta::new(ata(&wallet), false),
                AccountMeta::new(ata(&payee), false),
                AccountMeta::new_readonly(wallet, true),
            ],
            data: {
                let mut d = vec![3u8];
                d.extend_from_slice(&100u64.to_le_bytes());
                d
            },
        };
        let ata_ix = Instruction {
            program_id: KnownProgram::SplAssociatedToken.id(),
            accounts: vec![AccountMeta::new(wallet, true)],
            data: vec![tag],
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ata_ix, transfer], Some(&wallet))),
        }
    }

    #[test]
    fn transfer_allows_an_idempotent_ata_creation() {
        let (wallet, payee) = (Pubkey::new_unique(), Pubkey::new_unique());
        let expected = vec![ExpectedTransfer {
            recipient: payee,
            amount: 100,
        }];
        // Tag 1 is CreateIdempotent, which a legitimate transfer uses to make
        // the recipient's account.
        assert_transfers(
            &[transfer_with_ata_ix(1, wallet, payee)],
            &wallet,
            Token::Hnt,
            &expected,
        )
        .expect("an idempotent create must be allowed");
    }

    #[test]
    fn transfer_rejects_a_non_idempotent_ata_creation() {
        let (wallet, payee) = (Pubkey::new_unique(), Pubkey::new_unique());
        let expected = vec![ExpectedTransfer {
            recipient: payee,
            amount: 100,
        }];
        // Tag 0 is Create: funds a brand-new account from the signer at rent
        // cost, for any owner the caller picks. The amount comparison cannot
        // see it, so only this check refuses it.
        let err = assert_transfers(
            &[transfer_with_ata_ix(0, wallet, payee)],
            &wallet,
            Token::Hnt,
            &expected,
        )
        .expect_err("a non-idempotent create must be refused");
        assert!(
            err.to_string().contains("creating one idempotently"),
            "{err}"
        );
    }

    #[test]
    fn transfer_assert_rejects_bundled_foreign_program() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        // A correct HNT transfer bundled with an instruction to another program
        // (here the system program — e.g. a SOL drain) must be refused even
        // though the HNT amounts balance.
        let mut data = vec![12u8];
        data.extend_from_slice(&250u64.to_le_bytes());
        data.push(8);
        let transfer_ix = Instruction {
            program_id: KnownProgram::SplToken.id(),
            accounts: vec![
                AccountMeta::new(ata(&wallet), false),
                AccountMeta::new_readonly(Pubkey::new_unique(), false),
                AccountMeta::new(ata(&recipient), false),
                AccountMeta::new_readonly(wallet, true),
            ],
            data,
        };
        let system_ix = Instruction {
            program_id: helium_lib::solana_sdk::system_program::ID,
            accounts: vec![
                AccountMeta::new(wallet, true),
                AccountMeta::new(Pubkey::new_unique(), false),
            ],
            data: vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        };
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(
                &[transfer_ix, system_ix],
                Some(&wallet),
            )),
        };
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Hnt,
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
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Hnt,
            &[ExpectedTransfer {
                recipient,
                amount: 250
            }]
        )
        .is_err());
    }

    /// Build a legacy `VersionedTransaction` with a single system-program
    /// transfer of `lamports` from `from` to `to`.
    fn sol_tx(from: Pubkey, to: Pubkey, lamports: u64) -> VersionedTransaction {
        let mut data = vec![2u8, 0, 0, 0];
        data.extend_from_slice(&lamports.to_le_bytes());
        let ix = Instruction {
            program_id: KnownProgram::SystemProgram.id(),
            accounts: vec![AccountMeta::new(from, true), AccountMeta::new(to, false)],
            data,
        };
        VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[ix], Some(&from))),
        }
    }

    #[test]
    fn sol_transfer_assert_accepts_matching() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let tx = sol_tx(wallet, recipient, 500);
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Sol,
            &[ExpectedTransfer {
                recipient,
                amount: 500
            }]
        )
        .is_ok());
    }

    #[test]
    fn sol_transfer_assert_rejects_redirected() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let attacker = Pubkey::new_unique();
        let tx = sol_tx(wallet, attacker, 500);
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Sol,
            &[ExpectedTransfer {
                recipient,
                amount: 500
            }]
        )
        .is_err());
    }

    #[test]
    fn sol_transfer_assert_rejects_bundled_spl_instruction() {
        let wallet = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        // A SOL transfer bundled with an SPL-token instruction (not a program a
        // native-SOL transfer should touch) must be refused.
        let mut sol_data = vec![2u8, 0, 0, 0];
        sol_data.extend_from_slice(&500u64.to_le_bytes());
        let sol_ix = Instruction {
            program_id: KnownProgram::SystemProgram.id(),
            accounts: vec![
                AccountMeta::new(wallet, true),
                AccountMeta::new(recipient, false),
            ],
            data: sol_data,
        };
        let mut spl_data = vec![3u8];
        spl_data.extend_from_slice(&1u64.to_le_bytes());
        let spl_ix = Instruction {
            program_id: KnownProgram::SplToken.id(),
            accounts: vec![
                AccountMeta::new(ata(&wallet), false),
                AccountMeta::new(ata(&recipient), false),
                AccountMeta::new_readonly(wallet, true),
            ],
            data: spl_data,
        };
        let tx = VersionedTransaction {
            signatures: vec![],
            message: VersionedMessage::Legacy(Message::new(&[sol_ix, spl_ix], Some(&wallet))),
        };
        assert!(assert_transfers(
            &[tx],
            &wallet,
            Token::Sol,
            &[ExpectedTransfer {
                recipient,
                amount: 500
            }]
        )
        .is_err());
    }

    #[test]
    fn multi_payee_parses_address_and_amount() {
        let json = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": 1.6\
        }";
        let payee: MultiPayee = serde_json::from_str(json).expect("payee");
        assert_eq!(payee.amount, 1.6);
        assert_eq!(
            payee.address.to_string(),
            "JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf"
        );
    }

    #[test]
    fn multi_payee_rejects_per_payee_token() {
        // The batch token is chosen via --token; a per-payee "token" key in the
        // file must be rejected rather than silently ignored.
        let json = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": 0.5,\
            \"token\": \"mobile\"\
        }";
        let result: std::result::Result<MultiPayee, serde_json::Error> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn multi_payee_rejects_bad_amount() {
        let json = "{\
            \"address\": \"JBjajLx1b2MsugerDALTffjh9dVdNx5XTvgJd8SpwUPf\",\
            \"amount\": \"foo\"\
        }";
        let result: std::result::Result<MultiPayee, serde_json::Error> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
