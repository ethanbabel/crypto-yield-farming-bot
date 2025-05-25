use eyre::Result;
use reqwest::Client;
use serde::Deserialize;
use std::{fs::File, io::Write, path::Path};

use crate::config::Config;

#[derive(Debug, Deserialize)]
struct AbiV2Response {
    status: String,
    message: String,
    result: String, // now holds the ABI JSON string
}

/// Fetches ABI for a given contract address from Etherscan V2 and saves to the specified path
pub async fn fetch_abi_v2(
    config: &Config,
    contract_address: &str,
    output_path: &Path,
) -> Result<()> {
    let client = Client::new();
    let url = format!(
        "https://api.etherscan.io/v2/api?chainid={}&module=contract&action=getabi&address={}&apikey={}",
        config.chain_id, contract_address, config.etherscan_api_key
    );

    let response = client.get(&url).send().await?.json::<AbiV2Response>().await?;

    if response.status != "1" {
        eyre::bail!("Failed to fetch ABI: {}", response.message);
    }

    let abi_json_raw = response.result;
    let abi_json = serde_json::from_str::<serde_json::Value>(&abi_json_raw)?
        .to_string();

    let mut file = File::create(output_path)?;
    file.write_all(abi_json.as_bytes())?;

    Ok(())
}

/// Downloads all ABIs needed for the project if `REFRESH_ABIS=true` in .env
pub async fn fetch_all_abis(config: &Config) -> Result<()> {
    let abis = vec![
        ("Reader", config.gmx_reader),
        ("DataStore", config.gmx_datastore),
    ];

    if config.refetch_abis {
        for (name, address) in abis {
            let path = Path::new("fetched_abis").join(format!("{}.json", name));
            println!("Fetching ABI for {} at {}", name, address);
            fetch_abi_v2(config, &format!("{:?}", address), &path).await?;
        }
    }

    Ok(())
}