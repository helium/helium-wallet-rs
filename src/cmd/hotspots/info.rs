use crate::{cmd::*, dao::SubDao, result::Result};

#[derive(Clone, Debug, clap::Args)]
/// Get details for a given hotspot
pub struct Cmd {
    ecc_key: helium_crypto::PublicKey,
}

impl Cmd {
    pub fn run(&self, opts: Opts) -> Result {
        let client = new_client(&opts.url)?;
        let info = client.get_hotspot_info(&SubDao::all(), &self.ecc_key)?;
        print_json(&info)

        // // let wallet = load_wallet(&opts.files)?;
        // // let password = get_wallet_password(false)?;
        // // let keypair = wallet.decrypt(password.as_bytes())?;

        // // let cluster = anchor_client::Cluster::from_str(
        // //     "https://solana-rpc.web.helium.io:443?session-key=Pluto",
        // // )?;
        // let cluster = anchor_client::Cluster::Mainnet;
        // println!("CLUSTER: {cluster:?}");
        // let client = anchor_client::Client::new(cluster, Rc::new(Keypair::void()));
        // let program = client.program(helium_entity_manager::id());
        // let hotspot_key = self.sub_dao.info_key(&self.ecc_key)?;
        // println!(
        //     "ECC: {}\nREWARDABLE: {}\nINFO: {}",
        //     &self.ecc_key,
        //     self.sub_dao.rewardable_entity_config_key(),
        //     &hotspot_key
        // );
        // let account = program.account::<helium_entity_manager::IotHotspotInfoV0>(hotspot_key)?;
        // let hotspot = Hotspot::for_address(&self.ecc_key, Some(account.try_into()?))?;
        // Get DC balance for wallet
        // let dc_address = anchor_spl::associated_token::get_associated_token_address(
        //     &wallet.public_key,
        //     crate::token::Token::Dc.mint(),
        // );
        // let program = client.program(lazy_distributor::id());
        // let account = program.rpc().get_account(&dc_address)?;

        // use anchor_client::anchor_lang::AccountDeserialize;
        // let account_data =
        //     anchor_spl::token::TokenAccount::try_deserialize(&mut account.data.as_ref())?;
        // println!("{}", account_data.amount);
        //
        //
        // let program = client.program(lazy_distributor::id());
        // println!("PROGRAM: {program:?}");
        // println!("LDK: {}", self.sub_dao.lazy_distributor_key());
        // let account = program
        //     .account::<lazy_distributor::LazyDistributorV0>(self.sub_dao.lazy_distributor_key())?;
        // println!("POST ACCOUNTS");
        // println!(
        //     "{:?}",
        //     account
        //         .oracles
        //         .iter()
        //         .map(|config| config.url.clone())
        //         .collect::<Vec<String>>()
        // );
        // Ok(())
        // lazy_distributor::program::
        // let _client = new_client(&opts.url).await?;

        // let info = self.sub_dao.info_key(&self.ecc_key)?;
        // let ld_key = self.sub_dao.lazy_distributor_key();
        // // let info = client.get_hotspot_info(&self.sub_dao, &self.ecc_key).await?;
        // let json = json!( {
        //     "info": info.to_string(),
        //     "lazy": ld_key.to_string(),
        // });
        // print_json(&json)
    }
}
