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

#[derive(Deserialize, Clone)]
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

/// Idempotent read:
/// If a mapping already exists, return it.
///
/// NOTE: This is a placeholder. Real implementation requires Cubist C2F SDK.
fn get_existing_mapping(
    _solana_pubkey: &str,
    _chain_id: u64,
) -> Result<Option<String>> {
    // Example real implementation (C2F):
    //
    // let bucket = keyvalue::open("solana_to_evm")?;
    // let key = format!("{}:{}", solana_pubkey, chain_id);
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
///
/// NOTE: This is a placeholder. Real implementation requires Cubist C2F SDK.
fn store_mapping_once(
    _solana_pubkey: &str,
    _chain_id: u64,
    _evm_address: &str,
) -> Result<()> {
    // Example real implementation (C2F):
    //
    // let bucket = keyvalue::open("solana_to_evm")?;
    // let key = format!("{}:{}", solana_pubkey, chain_id);
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
/// Creates a new Secp256k1 EVM key using CubeSigner CLI.
/// Tags the key with solana_pubkey and chain_id for tracking.
fn create_cubesigner_evm_key(
    solana_pubkey: &str,
    chain_id: u64,
) -> Result<String> {
    use std::process::Command;
    
    // Generate key material ID based on solana_pubkey and chain_id
    let key_material_id = format!("EVM_{}_{}", solana_pubkey, chain_id);
    
    // Create Secp256k1 key via CubeSigner CLI
    let output = Command::new("cs")
        .args(&[
            "key",
            "create",
            "--type", "Secp256k1",
            "--material-id", &key_material_id,
        ])
        .output()
        .map_err(|e| anyhow!("Failed to execute CubeSigner CLI: {}", e))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("CubeSigner key creation failed: {}", stderr));
    }
    
    // Parse output to extract EVM address
    let stdout = String::from_utf8_lossy(&output.stdout);
    
    // Expected output format (JSON):
    // { "key_id": "Key#...", "address": "0x...", ... }
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .map_err(|e| anyhow!("Failed to parse CubeSigner output: {}", e))?;
    
    let address = parsed["address"]
        .as_str()
        .ok_or_else(|| anyhow!("No address field in CubeSigner response"))?
        .to_string();
    
    // Validate it's a proper EVM address (0x + 40 hex chars)
    if !address.starts_with("0x") || address.len() != 42 {
        return Err(anyhow!("Invalid EVM address format: {}", address));
    }
    
    Ok(address)
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

