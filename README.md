# helium-wallet

[![Build Status](https://travis-ci.com/helium/helium-wallet-rs.svg?branch=master)](https://travis-ci.com/helium/helium-wallet-rs)

A [Helium](https://helium.com) wallet implementation in Rust.

This is a simple wallet implementation that enables the creation and
use of an encrypted wallet.

**NOTE:** This wallet is _not_ the absolute safest way to create and
store a private key. No guarantees are implied as to it's safety and
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

### Create a wallet

```
    helium-wallet create basic
```

The basic wallet will be stored in `wallet.key` after specifying an
encryption password on the command line. Options exist to specify the
wallet output file and to force overwriting an existing wallet.

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
keys:

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
    helium-wallet info -f my.key
    helium-wallet info -f wallet.key.1 -f wallet.key.2 -f my.key
```

The given wallets will be read and information about the wallet,
including the public key, displayed. This command works for all wallet
types.


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
    helium-wallet verify -f wallet.key
    helium-wallet verify -f wallet.key.1 -f wallet.key.2 -f wallet.key.5
```
