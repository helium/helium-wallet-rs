# helium-lib

Core Rust library for interacting with the Helium network on Solana. Used by
the `helium-wallet` CLI and available for integration into other Rust projects.

## Modules

| Module | Purpose |
|--------|---------|
| `asset` | Compressed NFT asset operations (fetch, transfer, burn, search) |
| `b64` | Base64 message encoding/decoding |
| `boosting` | Mobile hotspot hex boosting |
| `client` | Solana RPC client wrapper with DAS (Digital Asset Standard) and gRPC config support |
| `dao` | Helium DAO and SubDAO operations (HNT, IOT, MOBILE) |
| `dc` | Data credit minting, delegation, and burning |
| `entity_key` | Entity key management for hotspots |
| `error` | Error types (`Error`, `DecodeError`, `EncodeError`, `ConfirmationError`, `JupiterError`) |
| `hotspot` | Hotspot CRUD, transfer, and onboarding (submodules: `dataonly`, `info`, `cert`) |
| `jupiter` | Token swaps via Jupiter V2 API |
| `keypair` | Solana keypair wrappers with optional BIP39 mnemonic support |
| `kta` | KeyToAsset (KTA) lookups |
| `memo` | Memo encoding/decoding |
| `message` | Versioned transaction message building |
| `onboarding` | Hotspot onboarding API client |
| `priority_fee` | Compute budget and priority fee calculation |
| `programs` | On-chain program IDs (`helium_sub_daos`, `lazy_distributor`, etc.) |
| `queue` | Reward queue operations |
| `reward` | Reward claiming via lazy distributor |
| `schedule` | Schedule-based reward claiming |
| `token` | Token operations (HNT, MOBILE, IOT, DC, SOL, USDC) -- balances, transfers, burns, prices |
| `transaction` | Versioned transaction building and confirmation |

## Feature Flags

| Feature | Description |
|---------|-------------|
| `clap` | Adds CLI argument parsing support (value parsers for `Token` types) |
| `mnemonic` | Enables BIP39 mnemonic/seed phrase support for keypairs via `helium-mnemonic` |

## Re-exports

The crate re-exports several foundational Solana and Anchor crates for
convenience, so downstream consumers do not need to manage version alignment
themselves:

- `anchor_client`
- `anchor_lang`
- `anchor_spl`
- `solana_sdk` (includes `bs58`)
- `solana_program`
- `solana_client`
- `solana_transaction_status`
- `tuktuk_sdk`

## Usage

Add `helium-lib` to your `Cargo.toml`:

```toml
[dependencies]
helium-lib = { git = "https://github.com/helium/helium-wallet-rs" }
```

### Creating a client and querying a token balance

```rust
use helium_lib::{
    client::Client,
    keypair::Pubkey,
    token::{self, Token},
};
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to mainnet (accepts "m", "mainnet-beta", "d", "devnet", or a URL)
    let client = Client::try_from("m")?;

    // Initialize the KeyToAsset cache
    helium_lib::init(client.solana_client.clone())?;

    // Look up the HNT associated token address for a wallet
    let wallet = Pubkey::from_str("YOUR_SOLANA_PUBKEY")?;
    let hnt_address = Token::Hnt.associated_token_address(&wallet);

    // Fetch the balance
    if let Some(balance) = token::balance_for_address(&client, &hnt_address).await? {
        println!("HNT balance: {}", balance.amount);
    }

    Ok(())
}
```
