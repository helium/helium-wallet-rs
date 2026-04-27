# helium-wallet

[![Build Status][actions-badge]][actions-url]

[actions-badge]: https://github.com/helium/helium-wallet-rs/workflows/CI/badge.svg
[actions-url]: https://github.com/helium/helium-wallet-rs/actions?query=workflow%3ACI+branch%3Amaster

A [Helium](https://helium.com) wallet implementation in Rust.

**NOTE:** This wallet is _not_ the absolute safest way to create and
store a private key. No guarantees are implied as to its safety and
suitability for use as a wallet associated with Helium crypto-tokens.

## Workspace

| Crate | Description |
|---|---|
| `helium-wallet` | CLI binary for wallet operations |
| `helium-lib` | Core Rust library for Helium/Solana blockchain interaction |
| `helium-proto-crypto` | Protobuf message signing and verification |

## Installation

### From Binary

Download the latest binary for your platform from
[Releases](https://github.com/helium/helium-wallet-rs/releases/latest). Unpack
the zip file and place the `helium-wallet` binary in your `$PATH`.

### Building from Source

Requirements: Rust toolchain, C compiler, pkg-config, cmake, clang.

Ubuntu 20.04 setup:

```
sudo apt update && sudo apt install build-essential pkg-config cmake clang
curl https://sh.rustup.rs -sSf | sh
source $HOME/.cargo/env
```

Build:

```
git clone https://github.com/helium/helium-wallet-rs
cd helium-wallet-rs
cargo build --release
```

The resulting `target/release/helium-wallet` is ready for use.

### Releasing

Releases are cut with [`cargo-release`](https://github.com/crate-ci/cargo-release).
The shared config in `release.toml` enables GPG-signed commits and tags
and disables crates.io publishing. Tags follow `<package>-v<version>`;
pushing a `helium-wallet-v*` tag triggers
`.github/workflows/rust.yml`, which builds the linux and macos-arm64
binaries and uploads them to a GitHub Release.

From a clean `master`:

```sh
cargo release patch -p helium-wallet --execute
```

That bumps `helium-wallet/Cargo.toml`, refreshes `Cargo.lock`, creates
the signed `chore: Release` commit and tag, and pushes both. Branch
protection on `master` requires admin bypass at push time. Substitute
`minor` or `major` for non-patch bumps; use `-p helium-lib` for a lib
release (no CI hook today, just a versioned tag).

## Usage

Use `-h` or `--help` on any command for detailed help.

### Global Options

Global options _precede_ the subcommand.

| Option | Description |
|---|---|
| `-f` / `--file` | Wallet file(s). Repeat for sharded wallets or multiple wallets. Default: `wallet.key` |
| `--url <URL>` | Solana RPC URL. Shortcuts: `m` (mainnet, default), `d` (devnet). Any full URL also accepted |

### Common Command Options

Most commands that submit transactions accept these options via `--commit`:

| Option | Description |
|---|---|
| `--commit` | Submit the transaction. Without this flag the transaction is simulated (dry run) |
| `--skip-preflight` | Skip Solana preflight checks |
| `--min-priority-fee <u64>` | Minimum priority fee in micro-lamports |
| `--max-priority-fee <u64>` | Maximum priority fee in micro-lamports |

## Commands

### `create` -- Create a Wallet

#### Basic

```
helium-wallet create basic
```

The wallet is stored in `wallet.key` after specifying an encryption password. Use
`-o` to set a different output file and `--force` to overwrite an existing wallet.

Use the `--seed` option to restore from a previously generated seed phrase.
The CLI accepts 12 or 24 word BIP39 phrases from Helium mobile wallets or any
valid BIP39 word list.

#### Sharded

Sharding is supported via [Shamir's Secret Sharing](https://github.com/dsprenkels/sss).
A key is split into N shards requiring K shards to recover:

```
helium-wallet create sharded -n 5 -k 3
```

Creates `wallet.key.1` through `wallet.key.5`. Use `-o` to set the base name.

The `--seed` option works with sharded wallets too.

#### Keypair

```
helium-wallet create keypair
```

#### Implementation Details

A ed25519 key is generated via libsodium. The provided password is run
through PBKDF2, with a configurable number of iterations and a random
salt, and the resulting value is used as an AES key. When sharding is
enabled, an additional AES key is randomly generated and the 2 keys
are combined using a sha256 HMAC into the final AES key.

The private key is then encrypted with AES256-GCM and stored in the
file along with the sharding information, the key share (if
applicable), the AES initialization vector, the PBKDF2 salt and
iteration count and the AES-GCM authentication tag.

### `info` -- Wallet Information

```
helium-wallet info
helium-wallet -f my.key info
helium-wallet -f wallet.key.1 -f wallet.key.2 -f my.key info
```

Displays the wallet address and key type. Use `--qr` to display a QR code for
the public key (useful for receiving tokens from the mobile wallet).

This command also serves as verification for sharded wallets -- pass at least K
shard files:

```
helium-wallet -f wallet.key.1 -f wallet.key.2 -f wallet.key.5 info
```

### `balance` -- Token Balances

```
helium-wallet balance
```

Displays balances for HNT, MOBILE, IOT, DC, SOL, and USDC.

### `transfer` -- Send Tokens

#### Single Payee

```
helium-wallet transfer one <address> <amount> <token>
helium-wallet transfer one <address> <amount> <token> --commit
```

Tokens: `hnt`, `mobile`, `iot`, `usdc`, `sol`. HNT supports 8 decimal
places; MOBILE and IOT support 6.

#### Multiple Payees

```
helium-wallet transfer multi <path-to-json>
helium-wallet transfer multi <path-to-json> --commit
```

JSON format:

```json
[
  { "address": "<address1>", "amount": 1.6, "token": "hnt" },
  { "address": "<address2>", "amount": "max" },
  { "address": "<address3>", "amount": 3, "token": "mobile" }
]
```

Fields: `address` (required), `amount` (required, number or `"max"`),
`token` (optional, defaults to `hnt`), `memo` (optional, 8-byte base64).

### `swap` -- Swap Tokens via Jupiter

Swap between tokens using the Jupiter V2 API.

```
helium-wallet swap <input_token> <output_token> <amount>
helium-wallet swap hnt usdc 10 --commit
helium-wallet swap mobile hnt 1000 --slippage-bps 200 --commit
```

| Argument | Description |
|---|---|
| `input_token` | Source token: `hnt`, `mobile`, `iot`, `usdc`, `sol` |
| `output_token` | Destination token: `hnt`, `mobile`, `iot`, `usdc`, `sol` |
| `amount` | Human-readable amount (e.g. `1.5` for 1.5 HNT) |
| `--slippage-bps` | Slippage tolerance in basis points. Default: 100 (1%) |

Output includes `in_amount`, `out_amount`, `price_impact_pct`, and the
transaction signature (when `--commit` is used).

Jupiter environment variables (see [Environment Variables](#environment-variables)):
`JUP_API_KEY`, `JUP_API_URL`, `JUP_SLIPPAGE_BPS`.

### `burn` -- Burn Tokens

```
helium-wallet burn <subdao> <amount>
helium-wallet burn iot 100 --commit
```

Burns subdao tokens (IOT or MOBILE).

### `hotspots` -- Hotspot Management

Subcommands: `add`, `list`, `info`, `update`, `transfer`, `burn`, `rewards`, `updates`.

```
helium-wallet hotspots list
helium-wallet hotspots info <address>
helium-wallet hotspots add <subcommand>
helium-wallet hotspots update <address> [options] --commit
helium-wallet hotspots transfer <address> <new-owner> --commit
helium-wallet hotspots burn <address> --commit
helium-wallet hotspots rewards <subcommand>
helium-wallet hotspots updates <address>
```

### `dc` -- Data Credit Operations

Subcommands: `price`, `mint`, `delegate`, `burn`.

```
helium-wallet dc price
helium-wallet dc mint <hnt-amount> --commit
helium-wallet dc delegate <address> --commit
helium-wallet dc burn <amount> --commit
```

### `assets` -- Asset Management

Subcommands: `rewards`, `info`, `burn`, `transfer`.

```
helium-wallet assets info <asset>
helium-wallet assets rewards <subcommand>
helium-wallet assets burn <asset> --commit
helium-wallet assets transfer <asset> <address> --commit
```

### `sign` -- Sign and Verify

```
helium-wallet sign file <path>
helium-wallet sign msg <message>
helium-wallet sign verify file <path> --signature <sig>
helium-wallet sign verify msg <message> --signature <sig>
```

### `price` -- Token Prices

```
helium-wallet price
```

### `export` -- Export Wallet

```
helium-wallet export
```

### `upgrade` -- Upgrade Wallet Format

```
helium-wallet upgrade
```

### `memo` -- Encode/Decode Memos

```
helium-wallet memo <subcommand>
```

### `router` -- Router Operations

```
helium-wallet router <subcommand>
```

## Environment Variables

### Solana RPC

| Variable | Description |
|---|---|
| `SOLANA_MAINNET_URL` | Solana RPC URL for mainnet (used by default or with `--url m`). The default is a rate-limited API served by the Helium Foundation; use a custom provider for repeated or large requests |
| `SOLANA_DEVNET_URL` | Solana RPC URL for devnet (used with `--url d`) |

### Wallet

| Variable | Description |
|---|---|
| `HELIUM_WALLET_PASSWORD` | Wallet decryption password. Useful for scripting; use with care |
| `HELIUM_WALLET_SEED_WORDS` | Space-separated seed words for restoring a wallet |
| `HELIUM_WALLET_SECRET` | Solana-style byte array keypair secret |

### Jupiter (Swap)

| Variable | Default | Description |
|---|---|---|
| `JUP_API_KEY` | _(none)_ | Jupiter API key. Optional -- omit for keyless access at 0.5 RPS |
| `JUP_API_URL` | `https://api.jup.ag/swap/v2` | Jupiter API base URL |
| `JUP_SLIPPAGE_BPS` | `100` (1%) | Default slippage tolerance in basis points |
