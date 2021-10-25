use super::TxnEnvelope;
use crate::result::Result;
use helium_api::models::HotspotStakingMode;
use helium_proto::*;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct TxnFeeConfig {
    // whether transaction fees are active
    txn_fees: bool,
    // a mutliplier which will be applied to the txn fee of all txns, in order
    // to make their DC costs meaningful
    txn_fee_multiplier: u64,
    // the staking fee in DC for each OUI
    staking_fee_txn_oui_v1: u64,
    // the staking fee in DC for each OUI/routing address
    staking_fee_txn_oui_v1_per_address: u64,
    // the staking fee in DC for adding a gateway
    #[serde(default = "TxnFeeConfig::default_add_full_staking_fee")]
    staking_fee_txn_add_gateway_v1: u64,
    // the staking fee in DC for adding a data only gateway
    #[serde(default = "TxnFeeConfig::default_add_dataonly_staking_fee")]
    staking_fee_txn_add_dataonly_gateway_v1: u64,
    // the staking fee in DC for adding a light gateway
    #[serde(default = "TxnFeeConfig::default_add_light_staking_fee")]
    staking_fee_txn_add_light_gateway_v1: u64,
    // the staking fee in DC for asserting a location
    #[serde(default = "TxnFeeConfig::default_assert_full_staking_fee")]
    staking_fee_txn_assert_location_v1: u64,
    // the staking fee in DC for asserting location for a dataonly gateway
    #[serde(default = "TxnFeeConfig::default_assert_dataonly_staking_fee")]
    staking_fee_txn_assert_location_dataonly_gateway_v1: u64,
    // the staking fee in DC for asserting location for a light gateway
    #[serde(default = "TxnFeeConfig::default_assert_light_staking_fee")]
    staking_fee_txn_assert_location_light_gateway_v1: u64,
}

pub const LEGACY_STAKING_FEE: u64 = 1;
pub const LEGACY_TXN_FEE: u64 = 0;

impl TxnFeeConfig {
    pub fn legacy() -> Self {
        Self {
            txn_fees: false,
            txn_fee_multiplier: 0,
            staking_fee_txn_oui_v1: LEGACY_STAKING_FEE,
            staking_fee_txn_oui_v1_per_address: 0,
            staking_fee_txn_add_gateway_v1: LEGACY_STAKING_FEE,
            staking_fee_txn_add_dataonly_gateway_v1: LEGACY_STAKING_FEE,
            staking_fee_txn_add_light_gateway_v1: LEGACY_STAKING_FEE,
            staking_fee_txn_assert_location_v1: LEGACY_STAKING_FEE,
            staking_fee_txn_assert_location_dataonly_gateway_v1: LEGACY_STAKING_FEE,
            staking_fee_txn_assert_location_light_gateway_v1: LEGACY_STAKING_FEE,
        }
    }

    pub fn dc_payload_size(&self) -> usize {
        if self.txn_fees {
            24
        } else {
            1
        }
    }

    fn default_add_full_staking_fee() -> u64 {
        4000000
    }

    fn default_add_dataonly_staking_fee() -> u64 {
        1000000
    }

    fn default_add_light_staking_fee() -> u64 {
        4000000
    }

    fn default_assert_full_staking_fee() -> u64 {
        1000000
    }

    fn default_assert_dataonly_staking_fee() -> u64 {
        500000
    }

    fn default_assert_light_staking_fee() -> u64 {
        1000000
    }
}

pub trait TxnFee {
    fn txn_fee(&self, config: &TxnFeeConfig) -> Result<u64>;
}

pub trait TxnStakingFee {
    fn txn_staking_fee(&self, config: &TxnFeeConfig) -> Result<u64>;
}

pub trait TxnModeStakingFee {
    fn txn_mode_staking_fee(&self, mode: &HotspotStakingMode, config: &TxnFeeConfig)
        -> Result<u64>;
}

fn calculate_txn_fee(payload_size: usize, config: &TxnFeeConfig) -> u64 {
    let dc_payload_size = config.dc_payload_size();
    if payload_size <= dc_payload_size {
        1
    } else {
        // integer div/ceil from: https://stackoverflow.com/a/2745086
        ((payload_size + dc_payload_size - 1) / dc_payload_size) as u64
    }
}

const TXN_FEE_SIGNATURE_SIZE: usize = 64;

macro_rules! payer_sig_clear {
    (basic, $txn:ident) => {};
    (payer, $txn:ident) => {
        if $txn.payer.is_empty() {
            $txn.payer_signature = vec![]
        } else {
            $txn.payer_signature = vec![0; TXN_FEE_SIGNATURE_SIZE]
        };
    };
}

macro_rules! impl_txn_fee {
    (($kind:ident, $txn_type:ty), $( $sig:ident ),+ ) => {
        impl TxnFee for $txn_type {
            fn txn_fee(&self, config: &TxnFeeConfig) -> Result<u64> {
                let mut txn: $txn_type = self.clone();
                txn.fee = 0;
                $(txn.$sig = vec![0; TXN_FEE_SIGNATURE_SIZE];)+
                payer_sig_clear!($kind, txn);
                let mut buf = vec![];
                txn.in_envelope().encode(&mut buf)?;
                Ok(calculate_txn_fee(buf.len(), config) * config.txn_fee_multiplier)
            }
        }
    };
    ($txn_type:ty, $($tail:tt)*) => {
        impl_txn_fee!((basic, $txn_type), $($tail)*);
    }
}

macro_rules! impl_txn_staking_fee {
    ($txn_type: ty, $field: ident) => {
        impl TxnStakingFee for $txn_type {
            fn txn_staking_fee(&self, config: &TxnFeeConfig) -> Result<u64> {
                Ok(config.$field)
            }
        }
    };
}

impl TxnModeStakingFee for BlockchainTxnAddGatewayV1 {
    fn txn_mode_staking_fee(
        &self,
        mode: &HotspotStakingMode,
        config: &TxnFeeConfig,
    ) -> Result<u64> {
        let result = match mode {
            HotspotStakingMode::Full => config.staking_fee_txn_add_gateway_v1,
            HotspotStakingMode::DataOnly => config.staking_fee_txn_add_dataonly_gateway_v1,
            HotspotStakingMode::Light => config.staking_fee_txn_add_light_gateway_v1,
        };
        Ok(result)
    }
}

impl TxnStakingFee for BlockchainTxnAddGatewayV1 {
    fn txn_staking_fee(&self, config: &TxnFeeConfig) -> Result<u64> {
        self.txn_mode_staking_fee(&HotspotStakingMode::Full, config)
    }
}

impl TxnModeStakingFee for BlockchainTxnAssertLocationV2 {
    fn txn_mode_staking_fee(
        &self,
        mode: &HotspotStakingMode,
        config: &TxnFeeConfig,
    ) -> Result<u64> {
        let result = match mode {
            HotspotStakingMode::Full => config.staking_fee_txn_assert_location_v1,
            HotspotStakingMode::DataOnly => {
                config.staking_fee_txn_assert_location_dataonly_gateway_v1
            }
            HotspotStakingMode::Light => config.staking_fee_txn_assert_location_light_gateway_v1,
        };
        Ok(result)
    }
}

impl TxnStakingFee for BlockchainTxnAssertLocationV2 {
    fn txn_staking_fee(&self, config: &TxnFeeConfig) -> Result<u64> {
        self.txn_mode_staking_fee(&HotspotStakingMode::Full, config)
    }
}

impl_txn_fee!(BlockchainTxnPaymentV1, signature);
impl_txn_fee!(BlockchainTxnPaymentV2, signature);
impl_txn_fee!(BlockchainTxnCreateHtlcV1, signature);
impl_txn_fee!(BlockchainTxnRedeemHtlcV1, signature);
impl_txn_fee!(BlockchainTxnSecurityExchangeV1, signature);
impl_txn_fee!(BlockchainTxnTokenBurnV1, signature);
impl_txn_fee!(
    (payer, BlockchainTxnAddGatewayV1),
    owner_signature,
    gateway_signature
);
impl_txn_fee!(
    (payer, BlockchainTxnAssertLocationV1),
    owner_signature,
    gateway_signature
);
impl_txn_staking_fee!(
    BlockchainTxnAssertLocationV1,
    staking_fee_txn_assert_location_v1
);
impl_txn_fee!((payer, BlockchainTxnAssertLocationV2), owner_signature);
impl_txn_fee!((payer, BlockchainTxnOuiV1), owner_signature);

impl_txn_fee!(
    BlockchainTxnTransferHotspotV1,
    buyer_signature,
    seller_signature
);
impl_txn_fee!(BlockchainTxnTransferHotspotV2, owner_signature);
impl_txn_fee!(BlockchainTxnStakeValidatorV1, owner_signature);
impl_txn_fee!(BlockchainTxnUnstakeValidatorV1, owner_signature);
impl_txn_fee!(
    BlockchainTxnTransferValidatorStakeV1,
    old_owner_signature,
    new_owner_signature
);
impl_txn_fee!(BlockchainTxnRoutingV1, signature);

impl TxnStakingFee for BlockchainTxnOuiV1 {
    fn txn_staking_fee(&self, config: &TxnFeeConfig) -> Result<u64> {
        let fee = config.staking_fee_txn_oui_v1
            + (self.requested_subnet_size as u64 * config.staking_fee_txn_oui_v1_per_address);
        Ok(fee)
    }
}

impl TxnStakingFee for BlockchainTxnRoutingV1 {
    fn txn_staking_fee(&self, config: &TxnFeeConfig) -> Result<u64> {
        Ok(
            if let Some(blockchain_txn_routing_v1::Update::RequestSubnet(size)) = &self.update {
                *size as u64 * config.staking_fee_txn_oui_v1_per_address
            } else {
                0
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keypair::Keypair;
    use helium_proto::Payment;

    macro_rules! assert_txn_fee {
        ($txn: expr, $cfg: expr, $expected: expr) => {
            let actual = $txn.txn_fee($cfg).unwrap();
            assert_eq!(actual, $expected);
        };
    }

    macro_rules! assert_txn_staking_fee {
        ($txn: expr, $cfg: expr, $expected: expr) => {
            let actual = $txn.txn_staking_fee($cfg).unwrap();
            assert_eq!(actual, $expected);
        };
    }

    macro_rules! assert_txn_mode_staking_fee {
        ($txn: expr, $mode: expr, $cfg: expr, $expected: expr) => {
            let actual = $txn.txn_mode_staking_fee($mode, $cfg).unwrap();
            assert_eq!(actual, $expected);
        };
    }

    const STAKING_FEE_ASSERT_LOCATION: u64 = 40 * 100_000;
    const STAKING_FEE_ASSERT_LOCATION_DATAONLY_GATEWAY: u64 = 10 * 100_000;
    const STAKING_FEE_ASSERT_LOCATION_LIGHT_GATEWAY: u64 = 20 * 100_000;
    const STAKING_FEE_ADD_GATEWAY: u64 = 20 * 100_000;
    const STAKING_FEE_ADD_DATAONLY_GATEWAY: u64 = 5 * 100_000;
    const STAKING_FEE_ADD_LIGHT_GATEWAY: u64 = 10 * 100_000;
    const STAKING_FEE_OUI: u64 = 100 * 100_000;
    const STAKING_FEE_OUI_PER_ADDRESS: u64 = 100 * 100_000;

    impl TxnFeeConfig {
        pub fn active() -> Self {
            Self {
                txn_fees: true,
                txn_fee_multiplier: 5000,
                staking_fee_txn_add_gateway_v1: STAKING_FEE_ADD_GATEWAY,
                staking_fee_txn_add_dataonly_gateway_v1: STAKING_FEE_ADD_DATAONLY_GATEWAY,
                staking_fee_txn_add_light_gateway_v1: STAKING_FEE_ADD_LIGHT_GATEWAY,
                staking_fee_txn_assert_location_v1: STAKING_FEE_ASSERT_LOCATION,
                staking_fee_txn_assert_location_dataonly_gateway_v1:
                    STAKING_FEE_ASSERT_LOCATION_DATAONLY_GATEWAY,
                staking_fee_txn_assert_location_light_gateway_v1:
                    STAKING_FEE_ASSERT_LOCATION_LIGHT_GATEWAY,
                staking_fee_txn_oui_v1: STAKING_FEE_OUI,
                staking_fee_txn_oui_v1_per_address: STAKING_FEE_OUI_PER_ADDRESS,
            }
        }
    }

    #[test]
    fn payment_v1_fee() {
        let payer = Keypair::default();
        let payee = Keypair::default();
        let fee_config = TxnFeeConfig::active();
        let mut txn = BlockchainTxnPaymentV1 {
            payee: payee.public_key().to_vec(),
            payer: payer.public_key().to_vec(),
            amount: 0,
            nonce: 1,
            fee: 0,
            signature: vec![],
        };
        txn.amount = 10_000;
        assert_txn_fee!(txn, &TxnFeeConfig::legacy(), 0);
        assert_txn_fee!(txn, &fee_config, 30_000);

        txn.amount = 16_383;
        assert_txn_fee!(txn, &fee_config, 30_000);

        txn.amount = 16_384;
        assert_txn_fee!(txn, &fee_config, 35_000);
    }

    #[test]
    fn payment_v2_fee() {
        let payer = Keypair::default();
        let payee = Keypair::default();
        let fee_config = TxnFeeConfig::active();
        let payment = Payment {
            payee: payee.public_key().to_vec(),
            memo: 0,
            amount: 10_000,
        };
        let txn = BlockchainTxnPaymentV2 {
            payer: payer.public_key().to_vec(),
            payments: vec![payment],
            nonce: 1,
            fee: 0,
            signature: vec![],
        };
        assert_txn_fee!(txn, &TxnFeeConfig::legacy(), 0);
        assert_txn_fee!(txn, &fee_config, 35_000);
    }

    #[test]
    fn create_htlc_fee() {
        let payer = Keypair::default();
        let payee = Keypair::default();
        let fee_config = TxnFeeConfig::active();
        let txn = BlockchainTxnCreateHtlcV1 {
            amount: 10_000,
            fee: 0,
            payee: payee.public_key().to_vec(),
            payer: payer.public_key().to_vec(),
            address: payer.public_key().to_vec(),
            hashlock: vec![],
            timelock: 1,
            nonce: 1,
            signature: vec![],
        };
        assert_txn_fee!(txn, &TxnFeeConfig::legacy(), 0);
        assert_txn_fee!(txn, &fee_config, 40_000);
    }

    #[test]
    fn redeem_htlc_fee() {
        let payer = Keypair::default();
        let payee = Keypair::default();
        let fee_config = TxnFeeConfig::active();
        let txn = BlockchainTxnRedeemHtlcV1 {
            fee: 0,
            payee: payee.public_key().to_vec(),
            address: payer.public_key().to_vec(),
            preimage: vec![],
            signature: vec![],
        };
        assert_txn_fee!(txn, &TxnFeeConfig::legacy(), 0);
        assert_txn_fee!(txn, &fee_config, 30_000);
    }

    #[test]
    fn security_exchange_fee() {
        let payer = Keypair::default();
        let payee = Keypair::default();
        let fee_config = TxnFeeConfig::active();
        let txn = BlockchainTxnSecurityExchangeV1 {
            payee: payee.public_key().to_vec(),
            payer: payer.public_key().to_vec(),
            amount: 10_000,
            nonce: 1,
            fee: 0,
            signature: vec![],
        };
        assert_txn_fee!(txn, &TxnFeeConfig::legacy(), 0);
        assert_txn_fee!(txn, &fee_config, 30_000);
    }

    #[test]
    fn add_gateway_fee() {
        let owner = Keypair::default();
        let gateway = Keypair::default();
        let mut txn = BlockchainTxnAddGatewayV1 {
            owner: owner.public_key().to_vec(),
            gateway: gateway.public_key().to_vec(),
            payer: vec![],
            staking_fee: 0,
            fee: 0,
            owner_signature: vec![],
            gateway_signature: vec![],
            payer_signature: vec![],
        };
        assert_txn_fee!(txn, &TxnFeeConfig::legacy(), 0);
        assert_txn_mode_staking_fee!(
            txn,
            &HotspotStakingMode::Full,
            &TxnFeeConfig::legacy(),
            LEGACY_STAKING_FEE
        );

        let fee_config = TxnFeeConfig::active();
        // Check txn fee and staking fee
        assert_txn_fee!(txn, &fee_config, 45_000);
        assert_txn_staking_fee!(txn, &fee_config, STAKING_FEE_ADD_GATEWAY);
        assert_txn_mode_staking_fee!(
            txn,
            &HotspotStakingMode::Full,
            &fee_config,
            STAKING_FEE_ADD_GATEWAY
        );
        assert_txn_mode_staking_fee!(
            txn,
            &HotspotStakingMode::DataOnly,
            &fee_config,
            STAKING_FEE_ADD_DATAONLY_GATEWAY
        );
        assert_txn_mode_staking_fee!(
            txn,
            &HotspotStakingMode::Light,
            &fee_config,
            STAKING_FEE_ADD_LIGHT_GATEWAY
        );

        // Check fee without a payer but wiht staking fee
        txn.staking_fee = txn.txn_staking_fee(&fee_config).unwrap();
        assert_txn_fee!(txn, &fee_config, 45_000);

        // With a payer
        txn.payer = owner.public_key().to_vec();
        assert_txn_fee!(txn, &fee_config, 65_000);
        assert_txn_staking_fee!(txn, &fee_config, STAKING_FEE_ADD_GATEWAY);
    }

    #[test]
    fn oui_fee() {
        let owner = Keypair::default();
        let mut txn = BlockchainTxnOuiV1 {
            owner: owner.public_key().to_vec(),
            payer: vec![],
            filter: vec![],
            addresses: vec![],
            staking_fee: 0,
            requested_subnet_size: 8,
            fee: 0,
            oui: 1,
            owner_signature: vec![],
            payer_signature: vec![],
        };
        assert_txn_fee!(txn, &TxnFeeConfig::legacy(), 0);
        assert_txn_staking_fee!(txn, &TxnFeeConfig::legacy(), LEGACY_STAKING_FEE);

        let fee_config = TxnFeeConfig::active();
        let expected_staking_fee = STAKING_FEE_OUI + (8 * STAKING_FEE_OUI_PER_ADDRESS);
        assert_txn_fee!(txn, &fee_config, 25_000);
        assert_txn_staking_fee!(txn, &fee_config, expected_staking_fee);

        // with payer
        txn.payer = owner.public_key().to_vec();
        assert_txn_fee!(txn, &fee_config, 45_000);
        assert_txn_staking_fee!(txn, &fee_config, expected_staking_fee);
    }
}
