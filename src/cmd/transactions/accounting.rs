use super::{Balance, Difference};
use chrono::{DateTime, NaiveDateTime, Utc};
use helium_api::transactions::*;
use helium_api::transactions::{Reward, Transaction};
use helium_api::Client;
use prettytable::{Cell, Row};

pub trait IntoRow {
    fn into_row(&self, account: &Pubkey, balance: &mut Balance, client: &Client) -> Row;
}

trait GetBalanceDifference {
    fn get_balance_difference(
        &self,
        account: &Pubkey,
        balance: &mut Balance,
        client: &Client,
    ) -> Difference;
}

// fn txn_cost(balance: &Balance, fee: u64, height: u64, client: &Client) -> Difference {
//     if balance.dc <= fee {
//         Difference {
//             counterparty: None,
//             bones: 0,
//             dc: -(fee as isize),
//         }
//     } else {
//         let (oracle_price, _) = client.get_oracle_price_at_height(height as usize).unwrap();
//         Difference {
//             counterparty: None,
//             bones: -((fee / oracle_price) as isize),
//             dc: 0,
//         }
//     }
// }

impl GetBalanceDifference for PaymentV1 {
    fn get_balance_difference(
        &self,
        account: &Pubkey,
        _balance: &mut Balance,
        _client: &Client,
    ) -> Difference {
        let counterparty = Some(self.payee.clone().to_string());
        // This account is paying HNT
        if self.payer == *account {
            Difference {
                counterparty,
                bones: -(self.amount as isize),
                dc: 0,
            }
        }
        // this account is receiving HNT
        else {
            Difference {
                counterparty,
                bones: self.amount as isize,
                dc: 0,
            }
        }
    }
}

impl GetBalanceDifference for PaymentV2 {
    fn get_balance_difference(
        &self,
        account: &Pubkey,
        _balance: &mut Balance,
        _client: &Client,
    ) -> Difference {
        // This account is paying HNT
        if self.payer == *account {
            let counterparty = Some(if self.payments.len() == 1 {
                self.payments[0].payee.to_string()
            } else {
                "many_payees".to_string()
            });
            let mut bones = 0;
            for payment in &self.payments {
                bones -= payment.amount as isize;
            }
            Difference {
                counterparty,
                bones,
                dc: 0,
            }
        }
        // this account is receiving HNT
        else {
            let counterparty = Some(self.payer.to_string());
            let mut bones = 0;
            for payment in &self.payments {
                if payment.payee == *account {
                    bones += payment.amount as isize;
                }
            }
            Difference {
                counterparty,
                bones,
                dc: 0,
            }
        }
    }
}

impl GetBalanceDifference for RewardsV1 {
    fn get_balance_difference(
        &self,
        _account: &Pubkey,
        _balance: &mut Balance,
        _client: &Client,
    ) -> Difference {
        let mut bones = 0;
        // summate rewards for all reward types
        for reward in &self.rewards {
            bones += match reward {
                Reward::Securities(data) => data.amount,
                Reward::DataCredits(data) => data.amount,
                Reward::PocChallengees(data) => data.amount,
                Reward::PocChallengers(data) => data.amount,
                Reward::PocWitnesses(data) => data.amount,
                Reward::Consensus(data) => data.amount,
            } as isize;
        }
        Difference {
            counterparty: Some("Rewards".to_string()),
            bones,
            dc: 0,
        }
    }
}

impl GetBalanceDifference for TokenBurnV1 {
    fn get_balance_difference(
        &self,
        account: &Pubkey,
        _balance: &mut Balance,
        client: &Client,
    ) -> Difference {
        // This account is burning HNT
        let (bones, counterparty) = if self.payer == *account {
            (-(self.amount as isize), Some(self.payee.to_string()))
        }
        // This account is not burning any HNT,
        // so it must just be receiving the DC
        else {
            (self.amount as isize, Some(self.payer.to_string()))
        };

        // This account is receiving DC
        let dc = if self.payee == *account {
            let (oracle_price, _) = client
                .get_oracle_price_at_height(self.height as usize)
                .unwrap();
            (self.amount * oracle_price / 100000000000) as isize
        }
        // This account is not receiving HNT
        else {
            0
        };

        Difference {
            counterparty,
            bones,
            dc,
        }
    }
}

impl IntoRow for Transaction {
    fn into_row(&self, account: &Pubkey, balance: &mut Balance, client: &Client) -> Row {
        match self {
            Transaction::PaymentV1(payment) => payment.into_row(account, balance, client),
            Transaction::PaymentV2(payment_v2) => payment_v2.into_row(account, balance, client),
            Transaction::RewardsV1(reward) => reward.into_row(account, balance, client),
            Transaction::TokenBurnV1(burn) => {
                println!("{:?}", burn);
                burn.into_row(account, balance, client)
            }
            Transaction::AddGatewayV1(add_gateway) => {
                add_gateway.into_row(account, balance, client)
            }
            Transaction::AssertLocationV1(assert_location) => {
                assert_location.into_row(account, balance, client)
            }
            Transaction::CoinbaseV1(coinbase) => coinbase.into_row(account, balance, client),
            Transaction::CreateHtlcV1(create_htlc) => {
                create_htlc.into_row(account, balance, client)
            }

            Transaction::GenGatewayV1(gen_gateway) => {
                gen_gateway.into_row(account, balance, client)
            }
            Transaction::ConsensusGroupV1(consensus_group) => {
                consensus_group.into_row(account, balance, client)
            }
            Transaction::OuiV1(oui) => oui.into_row(account, balance, client),
            Transaction::PocReceiptsV1(poc_receipts) => {
                poc_receipts.into_row(account, balance, client)
            }
            Transaction::PocRequestV1(poc_request) => {
                poc_request.into_row(account, balance, client)
            }
            Transaction::RedeemHtlcV1(redeem_htlc) => {
                redeem_htlc.into_row(account, balance, client)
            }
            Transaction::SecurityCoinbaseV1(security_coinbase) => {
                security_coinbase.into_row(account, balance, client)
            }
            Transaction::RoutingV1(routing) => routing.into_row(account, balance, client),
            Transaction::SecurityExchangeV1(security_exchange) => {
                security_exchange.into_row(account, balance, client)
            }
            Transaction::VarsV1(vars) => vars.into_row(account, balance, client),

            Transaction::DcCoinbaseV1(dc_coinbase) => {
                dc_coinbase.into_row(account, balance, client)
            }
            Transaction::TokenBurnExchangeRateV1(token_burn_exchange_rate) => {
                token_burn_exchange_rate.into_row(account, balance, client)
            }
            Transaction::BundleV1(bundle) => bundle.into_row(account, balance, client),

            Transaction::StateChannelOpenV1(state_channel_open) => {
                state_channel_open.into_row(account, balance, client)
            }

            Transaction::UpdateGatewayOuiV1(update_gateway_oui) => {
                update_gateway_oui.into_row(account, balance, client)
            }

            Transaction::StateChannelCloseV1(state_channel_close) => {
                state_channel_close.into_row(account, balance, client)
            }
            Transaction::PriceOracleV1(price_oracle) => {
                price_oracle.into_row(account, balance, client)
            }

            Transaction::GenPriceOracleV1(gen_price_oracle) => {
                gen_price_oracle.into_row(account, balance, client)
            }
        }
    }
}

pub trait GetCommonRows {
    fn get_common_rows(&self) -> (Cell, Cell, Cell);
}

fn utc_timestamp_from_epoch(time: usize) -> DateTime<Utc> {
    DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(time as i64, 0), Utc)
}

macro_rules! dummy_difference {
    ($txn:ident) => {
        impl GetBalanceDifference for $txn {
            fn get_balance_difference(
                &self,
                _account: &Pubkey,
                _balance: &mut Balance,
                _client: &Client,
            ) -> Difference {
                Difference {
                    counterparty: None,
                    bones: 0,
                    dc: 0,
                }
            }
        }
    };
}

macro_rules! into_row {
    ($Txn:ident, $Label:expr) => {
        impl GetCommonRows for $Txn {
            fn get_common_rows(&self) -> (Cell, Cell, Cell) {
                (
                    Cell::new(&utc_timestamp_from_epoch(self.time).to_rfc3339()),
                    Cell::new(format!("{}", self.height).as_str()),
                    Cell::new(&self.hash.to_string()),
                )
            }
        }

        impl IntoRow for $Txn {
            fn into_row(&self, account: &Pubkey, balance: &mut Balance, client: &Client) -> Row {
                let common = self.get_common_rows();
                let difference = self.get_balance_difference(account, balance, client);

                let counterparty = if let Some(counterparty) = &difference.counterparty {
                    counterparty.as_str()
                } else {
                    "NA"
                };

                balance.update(&difference);
                Row::new(vec![
                    Cell::new(format!("{: <25}", $Label).as_str()),
                    common.0,
                    common.1,
                    common.2,
                    Cell::new(&counterparty),
                    Cell::new(format!("{:>21}", &difference.bones.to_string()).as_str()),
                    Cell::new(format!("{:>21}", (&balance.bones.to_string())).as_str()),
                    Cell::new(format!("{:>21}", (&difference.dc.to_string())).as_str()),
                    Cell::new(format!("{:>21}", (&balance.dc.to_string())).as_str()),
                ])
            }
        }
    };
}

into_row!(AddGatewayV1, "AddGatewayV1");
into_row!(AssertLocationV1, "AssertLocationV1");
into_row!(CoinbaseV1, "CoinbaseV1");
into_row!(CreateHtlcV1, "CreateHtlcV1");
into_row!(GenGatewayV1, "GenGatewayV1");
into_row!(ConsensusGroupV1, "ConsensusGroupV1");
into_row!(OuiV1, "OuiV1");
into_row!(PaymentV1, "PaymentV1");
into_row!(PocReceiptsV1, "PocReceiptsV1");
into_row!(PocRequestV1, "PocRequestV1");
into_row!(RedeemHtlcV1, "RedeemHtlcV1");
into_row!(SecurityCoinbaseV1, "SecurityCoinbaseV1");
into_row!(RoutingV1, "RoutingV1");
into_row!(SecurityExchangeV1, "SecurityExchangeV1");
into_row!(VarsV1, "VarsV1");
into_row!(RewardsV1, "RewardsV1");
into_row!(TokenBurnV1, "TokenBurnV1");
into_row!(DcCoinbaseV1, "DcCoinbaseV1");
into_row!(TokenBurnExchangeRateV1, "TokenBurnExchangeRateV1");
into_row!(StateChannelOpenV1, "StateChannelOpenV1");
into_row!(UpdateGatewayOuiV1, "UpdateGatewayOuiV1");
into_row!(StateChannelCloseV1, "StateChannelCloseV1");
into_row!(PaymentV2, "PaymentV2");
into_row!(PriceOracleV1, "PriceOracleV1");
into_row!(GenPriceOracleV1, "GenPriceOracleV1");
into_row!(BundleV1, "BundleV1");

dummy_difference!(AddGatewayV1);
dummy_difference!(AssertLocationV1);
dummy_difference!(CoinbaseV1);
dummy_difference!(CreateHtlcV1);
dummy_difference!(GenGatewayV1);
dummy_difference!(ConsensusGroupV1);
dummy_difference!(OuiV1);
dummy_difference!(PocReceiptsV1);
dummy_difference!(PocRequestV1);
dummy_difference!(RedeemHtlcV1);
dummy_difference!(SecurityCoinbaseV1);
dummy_difference!(RoutingV1);
dummy_difference!(SecurityExchangeV1);
dummy_difference!(VarsV1);
dummy_difference!(DcCoinbaseV1);
dummy_difference!(TokenBurnExchangeRateV1);
dummy_difference!(StateChannelOpenV1);
dummy_difference!(UpdateGatewayOuiV1);
dummy_difference!(StateChannelCloseV1);
dummy_difference!(PriceOracleV1);
dummy_difference!(GenPriceOracleV1);
dummy_difference!(BundleV1);
