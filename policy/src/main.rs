//! Cubist Policy for Skate Wallet Provisioning
//!
//! This policy handles KV store operations for Solana→EVM address mappings.
//! KEY CREATION happens in the backend (via cs CLI or API), NOT in this policy.
//!
//! ## Architecture
//! ```
//! Backend                          CubeSigner
//!    │                                 │
//!    ├── 1. cs key create ────────────►│ (create EVM key)
//!    │◄── 2. returns 0xABC... ─────────┤
//!    │                                 │
//!    ├── 3. invoke policy ────────────►│ (store mapping)
//!    │      {action: "store",          │
//!    │       solana_pubkey: "...",     │
//!    │       chain_ids: [...],         │
//!    │       evm_address: "0xABC"}     │
//!    │◄── 4. success ──────────────────┤
//! ```
//!
//! ## Build
//! ```bash
//! cd policy && cargo build --release
//! cs policy update --name "skate_wallet_provisioner" \
//!   target/wasm32-wasip2/release/skate_provisioner.wasm
//! ```

use cubist_policy_sdk::{
    error::Result,
    keyvalue::{self, IfExists, Value, OperationError},
    policy,
    AccessDecision,
    AccessRequest,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Bucket name for Solana to EVM mappings
const BUCKET_NAME: &str = "solana_to_evm";

// =============================================================================
// REQUEST/RESPONSE TYPES
// =============================================================================

#[derive(Deserialize)]
#[serde(tag = "action")]
enum PolicyRequest {
    /// Store mappings for a Solana address (called after backend creates key)
    #[serde(rename = "store")]
    Store {
        solana_pubkey: String,
        chain_ids: Vec<u64>,
        evm_address: String,
    },
    
    /// Get existing mappings for a Solana address
    #[serde(rename = "get")]
    Get {
        solana_pubkey: String,
        chain_ids: Vec<u64>,
    },
    
    /// Update mapping for a specific chain (admin only, after backend creates new key)
    #[serde(rename = "update")]
    Update {
        solana_pubkey: String,
        chain_id: u64,
        new_evm_address: String,
    },
}

#[derive(Serialize)]
struct StoreResponse {
    success: bool,
    evm_address: String,
    chain_mappings: HashMap<u64, String>,
}

#[derive(Serialize)]
struct GetResponse {
    success: bool,
    default_address: Option<String>,
    chain_mappings: HashMap<u64, String>,
}

#[derive(Serialize)]
struct UpdateResponse {
    success: bool,
    new_evm_address: String,
    chain_id: u64,
}

#[derive(Serialize)]
struct ErrorResponse {
    success: bool,
    error: String,
}

// =============================================================================
// KV STORE OPERATIONS
// =============================================================================

fn get_existing_mapping(solana_pubkey: &str, chain_id: u64) -> std::result::Result<Option<String>, String> {
    let bucket = keyvalue::open(BUCKET_NAME)
        .map_err(|e| format!("Failed to open bucket: {:?}", e))?;
    
    let key = format!("{}:{}", solana_pubkey, chain_id);
    
    match bucket.get(&key) {
        Ok(Some(Value::Str(addr))) => Ok(Some(addr)),
        Ok(Some(_)) => Err("Unexpected value type".into()),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("KV read error: {:?}", e)),
    }
}

fn get_default_evm_address(solana_pubkey: &str) -> std::result::Result<Option<String>, String> {
    let bucket = keyvalue::open(BUCKET_NAME)
        .map_err(|e| format!("Failed to open bucket: {:?}", e))?;
    
    let key = format!("default:{}", solana_pubkey);
    
    match bucket.get(&key) {
        Ok(Some(Value::Str(addr))) => Ok(Some(addr)),
        Ok(Some(_)) => Err("Unexpected value type".into()),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("KV read error: {:?}", e)),
    }
}

fn store_mapping_once(solana_pubkey: &str, chain_id: u64, evm_address: &str) -> std::result::Result<(), String> {
    let bucket = keyvalue::open(BUCKET_NAME)
        .map_err(|e| format!("Failed to open bucket: {:?}", e))?;
    
    let key = format!("{}:{}", solana_pubkey, chain_id);
    let value = Value::Str(evm_address.to_string());
    
    match bucket.set(&key, &value, IfExists::Deny) {
        Ok(()) => Ok(()),
        Err(OperationError::ConditionFailed(_)) => Ok(()), // Already exists - fine
        Err(e) => Err(format!("KV write error: {:?}", e)),
    }
}

fn store_default_evm_address(solana_pubkey: &str, evm_address: &str) -> std::result::Result<(), String> {
    let bucket = keyvalue::open(BUCKET_NAME)
        .map_err(|e| format!("Failed to open bucket: {:?}", e))?;
    
    let key = format!("default:{}", solana_pubkey);
    let value = Value::Str(evm_address.to_string());
    
    match bucket.set(&key, &value, IfExists::Deny) {
        Ok(()) => Ok(()),
        Err(OperationError::ConditionFailed(_)) => Ok(()), // Already exists - fine
        Err(e) => Err(format!("KV write error: {:?}", e)),
    }
}

fn update_mapping(solana_pubkey: &str, chain_id: u64, evm_address: &str) -> std::result::Result<(), String> {
    let bucket = keyvalue::open(BUCKET_NAME)
        .map_err(|e| format!("Failed to open bucket: {:?}", e))?;
    
    let key = format!("{}:{}", solana_pubkey, chain_id);
    let value = Value::Str(evm_address.to_string());
    
    bucket.set(&key, &value, IfExists::Overwrite)
        .map_err(|e| format!("KV write error: {:?}", e))
}

// =============================================================================
// HANDLERS
// =============================================================================

/// Store mappings for a Solana address across multiple chains
/// Called by backend AFTER it creates the EVM key via CubeSigner API
fn handle_store(solana_pubkey: String, chain_ids: Vec<u64>, evm_address: String) -> std::result::Result<StoreResponse, String> {
    if chain_ids.is_empty() {
        return Err("chain_ids cannot be empty".into());
    }
    
    // Validate EVM address format
    if !evm_address.starts_with("0x") || evm_address.len() != 42 {
        return Err(format!("Invalid EVM address format: {}", evm_address));
    }

    // Store default address (first-writer-wins)
    store_default_evm_address(&solana_pubkey, &evm_address)?;

    // Store chain-specific mappings
    let mut chain_mappings = HashMap::new();
    
    for chain_id in chain_ids {
        match get_existing_mapping(&solana_pubkey, chain_id)? {
            Some(existing) => {
                // Already exists, use existing value
                chain_mappings.insert(chain_id, existing);
            }
            None => {
                store_mapping_once(&solana_pubkey, chain_id, &evm_address)?;
                chain_mappings.insert(chain_id, evm_address.clone());
            }
        }
    }

    Ok(StoreResponse { 
        success: true,
        evm_address,
        chain_mappings,
    })
}

/// Get existing mappings for a Solana address
fn handle_get(solana_pubkey: String, chain_ids: Vec<u64>) -> std::result::Result<GetResponse, String> {
    let default_address = get_default_evm_address(&solana_pubkey)?;
    
    let mut chain_mappings = HashMap::new();
    for chain_id in chain_ids {
        if let Some(addr) = get_existing_mapping(&solana_pubkey, chain_id)? {
            chain_mappings.insert(chain_id, addr);
        }
    }

    Ok(GetResponse {
        success: true,
        default_address,
        chain_mappings,
    })
}

/// Update mapping for a specific chain (admin only)
/// Called by backend AFTER it creates a new EVM key
fn handle_update(solana_pubkey: String, chain_id: u64, new_evm_address: String) -> std::result::Result<UpdateResponse, String> {
    // Validate EVM address format
    if !new_evm_address.starts_with("0x") || new_evm_address.len() != 42 {
        return Err(format!("Invalid EVM address format: {}", new_evm_address));
    }

    // Verify Solana address has been provisioned
    get_default_evm_address(&solana_pubkey)?
        .ok_or_else(|| format!("Solana address {} not provisioned", solana_pubkey))?;

    // Update the mapping (allows overwrite)
    update_mapping(&solana_pubkey, chain_id, &new_evm_address)?;

    Ok(UpdateResponse {
        success: true,
        new_evm_address,
        chain_id,
    })
}

// =============================================================================
// POLICY ENTRY POINT
// =============================================================================

#[policy]
async fn main(request: AccessRequest) -> Result<AccessDecision> {
    let body = match &request.request {
        Some(body) => body,
        None => {
            let resp = serde_json::to_string(&ErrorResponse {
                success: false,
                error: "Missing request body".into(),
            }).unwrap();
            return Ok(AccessDecision::Deny(resp));
        }
    };
    
    let policy_req: PolicyRequest = match serde_json::from_str(body) {
        Ok(req) => req,
        Err(e) => {
            let resp = serde_json::to_string(&ErrorResponse {
                success: false,
                error: format!("Invalid request: {}", e),
            }).unwrap();
            return Ok(AccessDecision::Deny(resp));
        }
    };
    
    let response_json = match policy_req {
        PolicyRequest::Store { solana_pubkey, chain_ids, evm_address } => {
            match handle_store(solana_pubkey, chain_ids, evm_address) {
                Ok(res) => serde_json::to_string(&res).unwrap(),
                Err(e) => serde_json::to_string(&ErrorResponse {
                    success: false,
                    error: e,
                }).unwrap(),
            }
        }
        
        PolicyRequest::Get { solana_pubkey, chain_ids } => {
            match handle_get(solana_pubkey, chain_ids) {
                Ok(res) => serde_json::to_string(&res).unwrap(),
                Err(e) => serde_json::to_string(&ErrorResponse {
                    success: false,
                    error: e,
                }).unwrap(),
            }
        }
        
        PolicyRequest::Update { solana_pubkey, chain_id, new_evm_address } => {
            match handle_update(solana_pubkey, chain_id, new_evm_address) {
                Ok(res) => serde_json::to_string(&res).unwrap(),
                Err(e) => serde_json::to_string(&ErrorResponse {
                    success: false,
                    error: e,
                }).unwrap(),
            }
        }
    };
    
    // Return response in Deny reason (this is a data policy, not signing)
    Ok(AccessDecision::Deny(response_json))
}
