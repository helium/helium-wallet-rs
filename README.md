# helium-wallet

![Continuous Integration](https://github.com/helium/helium-wallet-rs/workflows/Continuous%20Integration/badge.svg)

A [Helium](https://helium.com) wallet implementation in Rust.

This is a simple wallet implementation that enables the creation and
use of an encrypted wallet.

**NOTE:** This wallet is _not_ the absolute safest way to create and
store a private key. No guarantees are implied as to its safety and
suitability for use as a wallet associated with Helium crypto-tokens.

## Installation

### From Binary

Download the latest binary for your platform here from
[Releases](https://github.com/helium/helium-wallet-rs/releases/latest). Unpack
the zip file and place the `helium-wallet` binary in your `$PATH`
somewhere.

### From Source

You will need a working Rust tool-chain installed to build this CLI
from source.

Clone this repo:

```
git clone https://github.com/helium/helium-wallet-rs
```

and build it using cargo:

```
cd helium-wallet-rs
cargo build --release
```

The resulting `target/release/helium-wallet` is ready for use. Place
it somewhere in your `$PATH` or run it straight from the the target
folder.

## Usage

At any time use `-h` or `--help` to get more help for a command.

### Global options

Global options _precede_ the actual command on the command line.

The following global options are supported

* `-f` / `--file` can be used once or multiple times to specify either
  shard files for a wallet or multiple wallets if the command supports
  it. If not specified a file called `wallet.key` is assumed to be the
  wallet to use for the command.

* `--format json|table` can be used to set the output of the command
  to either a tabular format or a json output.

### Create a wallet

```
    helium-wallet create basic
```

The basic wallet will be stored in `wallet.key` after specifying an
encryption password on the command line. Options exist to specify the
wallet output file and to force overwriting an existing wallet.

A `--seed` option followed by space seprated mnemonic words can be
used to construct the keys for the wallet.


### Create a sharded wallet

Sharding wallet keys is supported via [Shamir's Secret
Sharing](https://github.com/dsprenkels/sss).  A key can be broken into
N shards such that recovering the original key needs K distinct
shards. This can be done by passing options to `create`:

```
    helium-wallet create sharded -n 5 -k 3
```

This will create wallet.key.1 through wallet.key.5 (the base name of
the wallet file can be supplied with the `-o` parameter).

When keys are sharded using `verify` will require at least K distinct
keys.

A `--seed` option followed by space seprated mnemonic words can be
used to construct the keys for the wallet.

#### Implementation details

A ed25519 key is generated via libsodium. The provided password is run
through PBKDF2, with a configurable number of iterations and a random
salt, and the resulting value is used as an AES key. When sharding is
enabled, an additional AES key is randomly generated and the 2 keys
are combined using a sha256 HMAC into the final AES key.

The private key is then encrypted with AES256-GCM and stored in the
file along with the sharding information, the key share (if
applicable), the AES initialization vector, the PBKDF2 salt and
iteration count and the AES-GCM authentication tag.


### Public Key

```
    helium-wallet info
    helium-wallet -f my.key info
    helium-wallet -f wallet.key.1 -f wallet.key.2 -f my.key info
```

The given wallets will be read and information about the wallet,
including the public key, displayed. This command works for all wallet
types.

### Displaying

Displaying information for one or more wallets without needing its
password can be done using;


```
    helium-wallet info
```

To display a QR code for the public key of the given wallet use:

```
    helium-wallet info --qr
```

This is useful for sending tokens to the wallet from the mobile
wallet.

### Verifying

Verifying a wallet takes a password and one or more wallet files and
attempts to decrypt the wallet.

The wallet is assumed to be sharded if the first file given to the
verify command is a sharded wallet. The rest of the given files then
also have to be wallet shards. For a sharded wallet to be verified, at
least `K` wallet files must be passed in, where `K` is the value given
when creating the wallet.

```
    helium-wallet verify
    helium-wallet -f wallet.key verify
    helium-wallet -f wallet.key.1 -f wallet.key.2 -f wallet.key.5 verify
```

### Sending Tokens

To send tokens to other accounts use:

```
    helium-wallet pay -p<payee>=<hnt>
    helium-wallet -p<payee>=<hnt> --commit

```

Where `<payee>` is the wallet address for the wallet you want to
send tokens to, `<hnt>` is the number of HNT you want to send. Since 1 HNT
is 100,000,000 bones the `hnt` value can go up to 8 decimal digits of
precision.

The default behavior of the `pay` command is to print out what the
intended payment is going to be _without_ submiting it to the
blockchain.  In the second example the `--commit` option commits the
actual payment to the API for processing by the blockchain.


### Environment Variables

The following environment variables are supported:

* `HELIUM_API_URL` - The API URL to use for commands that need API
  access, for example sending tokens.

* `HELIUM_WALLET_PASSWORD` - The password to use to decrypt the
  wallet. Useful for scripting or other non-interactive commands, but
  use with care.
