//! C2F-side wallet provisioning logic
//!
//! IMPORTANT:
//! This file is written against Cubist's C2F KV abstractions.
//! The required C2F SDK (KV + execution runtime) is NOT part of
//! `cubist-policy-sdk`, so this file is NOT expected to compile locally.
//!
//! The logic here has been validated separately using a mocked KV store
//! and deterministic key creation.
//!

use serde::{Deserialize, Serialize};
use anyhow::{Result, anyhow};

/// NOTE: These imports require the real Cubist C2F SDK.
/// They are intentionally left here to show the exact integration shape.
///
/// use cubist_c2f::keyvalue::{self, IfExists, Value};

const MAPPING_BUCKET: &str = "solana_to_evm";

#[derive(Deserialize)]
pub struct ProvisionRequest {
    pub solana_pubkey: String,
    pub chain_id: u64,
}

#[derive(Serialize)]
pub struct ProvisionResponse {
    pub evm_address: String,
}

// --------------------------------------------------
// Helpers
// --------------------------------------------------

fn kv_key(solana_pubkey: &str, chain_id: u64) -> String {
    format!("{}:{}", solana_pubkey, chain_id)
}

/// Idempotent read:
/// If a mapping already exists, return it.
#[allow(dead_code)]
fn get_existing_mapping(
    _solana_pubkey: &str,
    _chain_id: u64,
) -> Result<Option<String>> {
    // Example real implementation (C2F):
    //
    // let bucket = keyvalue::open(MAPPING_BUCKET)?;
    // let key = kv_key(solana_pubkey, chain_id);
    //
    // match bucket.get(&key)? {
    //     Some(Value::String(addr)) => Ok(Some(addr)),
    //     _ => Ok(None),
    // }

    Err(anyhow!(
        "C2F KV not available in local environment"
    ))
}

/// Atomic write:
/// Store mapping only if it does not already exist.
#[allow(dead_code)]
fn store_mapping_once(
    _solana_pubkey: &str,
    _chain_id: u64,
    _evm_address: &str,
) -> Result<()> {
    // Example real implementation (C2F):
    //
    // let bucket = keyvalue::open(MAPPING_BUCKET)?;
    // let key = kv_key(solana_pubkey, chain_id);
    //
    // bucket.set(
    //     &key,
    //     &Value::from(evm_address),
    //     IfExists::Deny,
    // )?;
    //
    // Ok(())

    Err(anyhow!(
        "C2F KV not available in local environment"
    ))
}

/// CubeSigner key creation
///
/// This is intentionally isolated so that integration is mechanical.
/// Once CubeSigner wiring is decided, only this function changes.
#[allow(dead_code)]
fn create_cubesigner_evm_key(
    _solana_pubkey: &str,
    _chain_id: u64,
) -> Result<String> {
    Err(anyhow!(
        "CubeSigner integration not wired yet (intentional)"
    ))
}

// --------------------------------------------------
// C2F entrypoint
// --------------------------------------------------

/// Provision (or fetch) an EVM wallet for a Solana wallet + chainId.
///
/// Flow:
/// 1. Idempotent read from KV
/// 2. Create CubeSigner EVM key if missing
/// 3. Atomically store mapping
/// 4. Return EVM address
///
/// This function is intended to run inside Cubist C2F.
pub fn handle(req: ProvisionRequest) -> Result<ProvisionResponse> {
    // 1. Check if mapping already exists
    if let Some(addr) =
        get_existing_mapping(&req.solana_pubkey, req.chain_id)?
    {
        return Ok(ProvisionResponse { evm_address: addr });
    }

    // 2. Create new EVM wallet via CubeSigner
    let evm_address =
        create_cubesigner_evm_key(&req.solana_pubkey, req.chain_id)?;

    // 3. Store mapping atomically (first writer wins)
    store_mapping_once(
        &req.solana_pubkey,
        req.chain_id,
        &evm_address,
    )?;

    Ok(ProvisionResponse { evm_address })
}
