pub mod asset;
pub mod b64;
pub mod dao;
pub mod dc;
pub mod entity_key;
pub mod hotspot;
pub mod keypair;
pub mod onboarding;
pub mod priority_fee;
pub mod programs;
pub mod result;
pub mod reward;
pub mod settings;
pub mod token;

pub use anchor_client::{self, solana_client};
pub use helium_anchor_gen;
pub use solana_sdk;
pub use solana_sdk::bs58;

pub(crate) trait Zero {
    const ZERO: Self;
}

impl Zero for u32 {
    const ZERO: Self = 0;
}
impl Zero for u16 {
    const ZERO: Self = 0;
}

pub(crate) fn is_zero<T>(value: &T) -> bool
where
    T: PartialEq + Zero,
{
    value == &T::ZERO
}
