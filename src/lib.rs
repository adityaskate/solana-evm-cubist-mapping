//! Wallet Provisioning Types
//!
//! This library exports types used for Solana→EVM wallet provisioning.
//! The actual WASM policy that runs on CubeSigner is in `policy/src/main.rs`.
//! ## Flow
//!
//! ### Provision (batch creation):
//! - Input: solana_address + chain_ids (e.g., [1, 137, 42161])
//! - Backend creates ONE EVM wallet via `cs key create`
//! - Policy stores mapping for ALL chains: solA -> { 1: 0xevmA, 137: 0xevmA, 42161: 0xevmA }
//!
//! ### Update (admin only, per-chain):
//! - Input: solana_address + single chain_id + new_evm_address
//! - Backend creates NEW EVM wallet via `cs key create`
//! - Policy updates ONLY that chain's mapping, others unchanged

use serde::{Deserialize, Serialize};

/// Request to provision EVM wallets for a Solana address across multiple chains
#[derive(Deserialize, Clone)]
pub struct ProvisionRequest {
    pub solana_pubkey: String,
    /// List of chain IDs to provision (e.g., [1, 137, 42161])
    pub chain_ids: Vec<u64>,
}

/// Request to update the EVM address for a specific chain (admin only)
#[derive(Deserialize, Clone)]
pub struct UpdateMappingRequest {
    pub solana_pubkey: String,
    /// The specific chain to update
    pub chain_id: u64,
}

/// Response containing the provisioned EVM address and all chain mappings
#[derive(Serialize, Debug)]
pub struct ProvisionResponse {
    /// The EVM address created (same for all chains)
    pub evm_address: String,
    /// Map of chain_id -> evm_address for all provisioned chains
    pub chain_mappings: std::collections::HashMap<u64, String>,
}

/// Response for update mapping (admin operation)
#[derive(Serialize, Debug)]
pub struct UpdateMappingResponse {
    pub success: bool,
    /// The NEW EVM address created for this chain
    pub new_evm_address: String,
    /// The chain that was updated
    pub chain_id: u64,
}
