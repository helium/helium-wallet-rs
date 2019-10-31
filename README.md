# he_wallet

[![Build Status](https://travis-ci.com/helium/he_wallet_rs.svg?branch=master)](https://travis-ci.com/helium/he_wallet_rs)

A [Helium](https://helium.com) wallet implementation in Rust.

This is a simple wallet implementation that enables the creation and
use of an encrypted wallet.

**NOTE:** This wallet is _not_ the absolute safest way to create and
store a private key. No guarantees are implied as to it's safety and
suitability for use as a wallet associated with Helium crypto-tokens.

## Installation

### From Source

You will need a working Rust toolchain installed to build this CLI
from source.

Clone this repo:

```
git clone https://github.com/helium/he_wallet_rs
```

and build it using cargo:

```
cd he_wallet_rs
cargo build --release
```

The resulting `target/release/he_wallet` is ready for use. A
convenient shortcut is placed in `bin/he_wallet`.

## Usage

At any time use `-h` or `--help` to get more help for a command.

### Create a wallet

```
    bin/he_wallet create basic
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
    bin/he_wallet create sharded -n 5 -k 3
```

This will create wallet.key.1 through wallet.key.5 (the base name of
the wallet file can be supplied with the `-o` parameter).

When keys are sharded using `verify` will require at least K distinct
keys:

```
    bin/he_wallet verify -f wallet.key.1 -f wallet.key.2 -f wallet.key.5
```

The password will also be needed when verifying a sharded key.

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
    bin/he_wallet info
```

The wallet in `wallet.key` will be read and the public key for the
wallet displayed. Any sharded wallet file will be able to return the
public key for the wallet without having all the shards available.
