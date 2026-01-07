use cubist_wallet_provisioner::{ProvisionRequest, ProvisionResponse, UpdateMappingRequest, UpdateMappingResponse};
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Mock KV store for testing
#[derive(Clone)]
struct MockKvStore {
    data: Arc<Mutex<HashMap<String, String>>>,
    write_attempts: Arc<Mutex<Vec<String>>>,
    delete_attempts: Arc<Mutex<Vec<String>>>,
}

impl MockKvStore {
    fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
            write_attempts: Arc::new(Mutex::new(Vec::new())),
            delete_attempts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn get(&self, key: &str) -> Option<String> {
        self.data.lock().unwrap().get(key).cloned()
    }

    /// Atomic write - returns Ok(()) if key doesn't exist, Err if it does
    fn set_if_not_exists(&self, key: &str, value: &str) -> Result<()> {
        self.write_attempts.lock().unwrap().push(key.to_string());
        
        let mut data = self.data.lock().unwrap();
        if data.contains_key(key) {
            return Err(anyhow!("Key already exists (IfExists::Deny failed)"));
        }
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }
    
    /// Set with overwrite allowed (for admin updates)
    fn set(&self, key: &str, value: &str) -> Result<()> {
        let mut data = self.data.lock().unwrap();
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Attempt to delete a key - should always fail for immutable storage
    fn delete(&self, key: &str) -> Result<()> {
        self.delete_attempts.lock().unwrap().push(key.to_string());
        Err(anyhow!("Delete operation not supported (immutable storage)"))
    }
}

/// Mock implementations using the test KV store
struct TestContext {
    kv: MockKvStore,
    /// Counter for default keys (one per Solana address)
    default_key_counter: Arc<Mutex<u32>>,
    /// Counter for chain-specific keys (for admin updates)
    chain_key_counter: Arc<Mutex<u32>>,
}

impl TestContext {
    fn new() -> Self {
        Self {
            kv: MockKvStore::new(),
            default_key_counter: Arc::new(Mutex::new(0)),
            chain_key_counter: Arc::new(Mutex::new(1000)), // Start at 1000 to differentiate
        }
    }

    fn get_existing_mapping(&self, solana_pubkey: &str, chain_id: u64) -> Result<Option<String>> {
        let key = kv_key(solana_pubkey, chain_id);
        Ok(self.kv.get(&key))
    }
    
    fn get_default_evm_address(&self, solana_pubkey: &str) -> Result<Option<String>> {
        let key = default_key(solana_pubkey);
        Ok(self.kv.get(&key))
    }

    fn store_mapping_once(&self, solana_pubkey: &str, chain_id: u64, evm_address: &str) -> Result<()> {
        let key = kv_key(solana_pubkey, chain_id);
        self.kv.set_if_not_exists(&key, evm_address)
    }
    
    fn store_default_evm_address(&self, solana_pubkey: &str, evm_address: &str) -> Result<()> {
        let key = default_key(solana_pubkey);
        self.kv.set_if_not_exists(&key, evm_address)
    }
    
    fn update_mapping(&self, solana_pubkey: &str, chain_id: u64, evm_address: &str) -> Result<()> {
        let key = kv_key(solana_pubkey, chain_id);
        self.kv.set(&key, evm_address)
    }

    /// Create default EVM key (one per Solana address, used across all chains)
    fn create_cubesigner_evm_key(&self, _solana_pubkey: &str) -> Result<String> {
        let mut counter = self.default_key_counter.lock().unwrap();
        *counter += 1;
        Ok(format!("0x{:040x}", *counter))
    }

    /// Create chain-specific EVM key (for admin updates)
    fn create_cubesigner_evm_key_for_chain(&self, _solana_pubkey: &str, _chain_id: u64) -> Result<String> {
        let mut counter = self.chain_key_counter.lock().unwrap();
        *counter += 1;
        Ok(format!("0x{:040x}", *counter))
    }

    /// Main provision handler - batch creation for multiple chains
    fn handle(&self, req: ProvisionRequest) -> Result<ProvisionResponse> {
        if req.chain_ids.is_empty() {
            return Err(anyhow!("chain_ids cannot be empty"));
        }

        // 1. Check if default EVM address already exists
        let evm_address = if let Some(addr) = self.get_default_evm_address(&req.solana_pubkey)? {
            addr
        } else {
            // 2. Create new EVM key (one per Solana address)
            let addr = self.create_cubesigner_evm_key(&req.solana_pubkey)?;
            
            // Store as default address (atomic, first-writer-wins)
            self.store_default_evm_address(&req.solana_pubkey, &addr)?;
            
            addr
        };

        // 3. Store chain-specific mappings for ALL provided chain IDs
        let mut chain_mappings = HashMap::new();
        
        for &chain_id in &req.chain_ids {
            // Check if chain mapping already exists
            if let Some(existing) = self.get_existing_mapping(&req.solana_pubkey, chain_id)? {
                chain_mappings.insert(chain_id, existing);
            } else {
                // Store new mapping (atomic, first-writer-wins)
                self.store_mapping_once(&req.solana_pubkey, chain_id, &evm_address)?;
                chain_mappings.insert(chain_id, evm_address.clone());
            }
        }

        Ok(ProvisionResponse { 
            evm_address,
            chain_mappings,
        })
    }
    
    /// Admin-only update handler - creates NEW wallet for specific chain
    fn handle_update_mapping(&self, req: UpdateMappingRequest) -> Result<UpdateMappingResponse> {
        // 1. Verify Solana address has been provisioned
        let _default_addr = self.get_default_evm_address(&req.solana_pubkey)?
            .ok_or_else(|| anyhow!(
                "Solana address {} has not been provisioned yet", 
                req.solana_pubkey
            ))?;

        // 2. Create NEW EVM key (chain-specific)
        let new_evm_address = self.create_cubesigner_evm_key_for_chain(
            &req.solana_pubkey, 
            req.chain_id
        )?;

        // 3. Update the chain-specific mapping (allows overwrite)
        self.update_mapping(&req.solana_pubkey, req.chain_id, &new_evm_address)?;

        Ok(UpdateMappingResponse {
            success: true,
            new_evm_address,
            chain_id: req.chain_id,
        })
    }
}

fn kv_key(solana_pubkey: &str, chain_id: u64) -> String {
    format!("{}:{}", solana_pubkey, chain_id)
}

fn default_key(solana_pubkey: &str) -> String {
    format!("default:{}", solana_pubkey)
}

// =============================================================================
// PROVISION TESTS (Batch Creation)
// =============================================================================

#[test]
fn test_provision_creates_wallet_for_all_chains() {
    let ctx = TestContext::new();
    let req = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_ids: vec![1, 137, 42161],
    };

    let result = ctx.handle(req).unwrap();
    
    // Should create ONE address
    assert_eq!(result.evm_address, "0x0000000000000000000000000000000000000001");
    
    // Should have mappings for all 3 chains
    assert_eq!(result.chain_mappings.len(), 3);
    
    // All chains should have the SAME address
    assert_eq!(result.chain_mappings.get(&1), Some(&"0x0000000000000000000000000000000000000001".to_string()));
    assert_eq!(result.chain_mappings.get(&137), Some(&"0x0000000000000000000000000000000000000001".to_string()));
    assert_eq!(result.chain_mappings.get(&42161), Some(&"0x0000000000000000000000000000000000000001".to_string()));
    
    // Should have only created one key
    assert_eq!(*ctx.default_key_counter.lock().unwrap(), 1);
}

#[test]
fn test_provision_is_idempotent() {
    let ctx = TestContext::new();
    let req = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_ids: vec![1, 137, 42161],
    };

    // First provision
    let result1 = ctx.handle(req.clone()).unwrap();
    
    // Second provision (same request)
    let result2 = ctx.handle(req).unwrap();
    
    // Should return the same address
    assert_eq!(result1.evm_address, result2.evm_address);
    assert_eq!(result1.chain_mappings, result2.chain_mappings);
    
    // Should only have created one key (not two)
    assert_eq!(*ctx.default_key_counter.lock().unwrap(), 1);
}

#[test]
fn test_provision_can_add_new_chains_later() {
    let ctx = TestContext::new();
    
    // First provision with chains 1, 137
    let req1 = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_ids: vec![1, 137],
    };
    let result1 = ctx.handle(req1).unwrap();
    
    // Later provision with chain 42161 added
    let req2 = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_ids: vec![1, 137, 42161],
    };
    let result2 = ctx.handle(req2).unwrap();
    
    // All should have the same address (including new chain)
    assert_eq!(result1.evm_address, result2.evm_address);
    assert_eq!(result2.chain_mappings.len(), 3);
    assert_eq!(result2.chain_mappings.get(&42161), Some(&result1.evm_address));
    
    // Still only one key created
    assert_eq!(*ctx.default_key_counter.lock().unwrap(), 1);
}

#[test]
fn test_provision_fails_with_empty_chain_ids() {
    let ctx = TestContext::new();
    let req = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_ids: vec![],
    };

    let result = ctx.handle(req);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("chain_ids cannot be empty"));
}

#[test]
fn test_different_solana_addresses_get_different_wallets() {
    let ctx = TestContext::new();
    
    let req1 = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_ids: vec![1, 137, 42161],
    };
    
    let req2 = ProvisionRequest {
        solana_pubkey: "B4fiuy1rJgmbTrraeZpcEtGtFzmt2GVYr1XEoSY7HqqC".to_string(),
        chain_ids: vec![1, 137, 42161],
    };

    let result1 = ctx.handle(req1).unwrap();
    let result2 = ctx.handle(req2).unwrap();
    
    // Different Solana addresses → different EVM wallets
    assert_ne!(result1.evm_address, result2.evm_address);
    
    // Two keys created (one per Solana address)
    assert_eq!(*ctx.default_key_counter.lock().unwrap(), 2);
}

// =============================================================================
// UPDATE TESTS (Admin Only)
// =============================================================================

#[test]
fn test_update_creates_new_wallet_for_specific_chain() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    // First provision all chains with same default address
    let provision_req = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_ids: vec![1, 137, 42161],
    };
    let provision_result = ctx.handle(provision_req).unwrap();
    let default_address = provision_result.evm_address.clone();
    
    // Admin updates chain 137 to a NEW wallet
    let update_req = UpdateMappingRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 137,
    };
    let update_result = ctx.handle_update_mapping(update_req).unwrap();
    
    // Update should succeed
    assert!(update_result.success);
    assert_eq!(update_result.chain_id, 137);
    
    // New address should be different from default
    assert_ne!(update_result.new_evm_address, default_address);
    
    // Chain 137 should now have new address
    let chain_137 = ctx.get_existing_mapping(solana_pubkey, 137).unwrap();
    assert_eq!(chain_137, Some(update_result.new_evm_address.clone()));
    
    // Other chains should still have default address
    let chain_1 = ctx.get_existing_mapping(solana_pubkey, 1).unwrap();
    let chain_42161 = ctx.get_existing_mapping(solana_pubkey, 42161).unwrap();
    assert_eq!(chain_1, Some(default_address.clone()));
    assert_eq!(chain_42161, Some(default_address.clone()));
}

#[test]
fn test_update_fails_if_not_provisioned() {
    let ctx = TestContext::new();
    
    // Try to update without provisioning first
    let update_req = UpdateMappingRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 137,
    };
    
    let result = ctx.handle_update_mapping(update_req);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("has not been provisioned yet"));
}

#[test]
fn test_update_can_be_called_multiple_times() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    // Provision
    let provision_req = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_ids: vec![1, 137, 42161],
    };
    ctx.handle(provision_req).unwrap();
    
    // First update for chain 137
    let update_req1 = UpdateMappingRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 137,
    };
    let result1 = ctx.handle_update_mapping(update_req1).unwrap();
    
    // Second update for chain 137 (e.g., key rotation)
    let update_req2 = UpdateMappingRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 137,
    };
    let result2 = ctx.handle_update_mapping(update_req2).unwrap();
    
    // Each update creates a new wallet
    assert_ne!(result1.new_evm_address, result2.new_evm_address);
    
    // Latest address should be stored
    let current = ctx.get_existing_mapping(solana_pubkey, 137).unwrap();
    assert_eq!(current, Some(result2.new_evm_address));
}

// =============================================================================
// ATOMICITY & CONCURRENCY TESTS
// =============================================================================

#[test]
fn test_atomicity_prevents_overwrites_on_provision() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    // Manually create a mapping first (simulating race condition)
    let addr1 = "0xfirst111111111111111111111111111111111111";
    ctx.store_default_evm_address(solana_pubkey, addr1).unwrap();
    ctx.store_mapping_once(solana_pubkey, 1, addr1).unwrap();

    // Attempt to provision (should not overwrite)
    let req = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_ids: vec![1, 137],
    };
    let result = ctx.handle(req).unwrap();
    
    // Should use existing default address
    assert_eq!(result.evm_address, addr1);
    
    // Chain 1 should have original address (not overwritten)
    assert_eq!(result.chain_mappings.get(&1), Some(&addr1.to_string()));
    
    // Chain 137 should also use the default
    assert_eq!(result.chain_mappings.get(&137), Some(&addr1.to_string()));
}

#[test]
fn test_concurrent_provisions_first_writer_wins() {
    use std::thread;
    
    let ctx = Arc::new(TestContext::new());
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string();

    // Simulate 10 concurrent provision requests
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let ctx = Arc::clone(&ctx);
            let solana_pubkey = solana_pubkey.clone();
            
            thread::spawn(move || {
                let req = ProvisionRequest {
                    solana_pubkey,
                    chain_ids: vec![1, 137, 42161],
                };
                ctx.handle(req)
            })
        })
        .collect();

    // Collect all results
    let results: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();

    // All successful results should have the same address
    let successful: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    assert!(!successful.is_empty());
    
    let first_addr = &successful[0].evm_address;
    for result in &successful {
        assert_eq!(&result.evm_address, first_addr);
    }

    // Verify consistent state
    let stored_default = ctx.get_default_evm_address(&solana_pubkey).unwrap();
    assert!(stored_default.is_some());
}

#[test]
fn test_wallet_mappings_immutable_after_creation() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    let req = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_ids: vec![1, 137, 42161],
    };

    // Create initial mappings
    let result1 = ctx.handle(req.clone()).unwrap();
    let original_address = result1.evm_address.clone();
    
    // Make 100 more provision requests
    for _ in 0..100 {
        let result = ctx.handle(req.clone()).unwrap();
        assert_eq!(result.evm_address, original_address,
            "Default address changed - immutability violated!");
        
        for chain_id in &[1u64, 137, 42161] {
            assert_eq!(
                result.chain_mappings.get(chain_id),
                Some(&original_address),
                "Chain {} mapping changed - immutability violated!", chain_id
            );
        }
    }
    
    // Still only one default key created
    assert_eq!(*ctx.default_key_counter.lock().unwrap(), 1);
}

#[test]
fn test_cannot_delete_mappings() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    // Create mappings
    let req = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_ids: vec![1, 137, 42161],
    };
    let result = ctx.handle(req).unwrap();
    let original_address = result.evm_address.clone();

    // Attempt to delete mappings (should fail)
    let default_key = default_key(solana_pubkey);
    let chain_key = kv_key(solana_pubkey, 1);
    
    assert!(ctx.kv.delete(&default_key).is_err());
    assert!(ctx.kv.delete(&chain_key).is_err());

    // Verify mappings still exist
    let stored_default = ctx.get_default_evm_address(solana_pubkey).unwrap();
    assert_eq!(stored_default, Some(original_address.clone()));
    
    let stored_chain = ctx.get_existing_mapping(solana_pubkey, 1).unwrap();
    assert_eq!(stored_chain, Some(original_address));
}

// =============================================================================
// KV KEY FORMAT TESTS
// =============================================================================

#[test]
fn test_kv_key_format() {
    assert_eq!(kv_key("ABC123", 1), "ABC123:1");
    assert_eq!(kv_key("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", 137), 
               "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:137");
}

#[test]
fn test_default_key_format() {
    assert_eq!(default_key("ABC123"), "default:ABC123");
    assert_eq!(default_key("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"), 
               "default:7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU");
}

// =============================================================================
// INTEGRATION SCENARIO TESTS
// =============================================================================

#[test]
fn test_full_user_journey() {
    let ctx = TestContext::new();
    
    // User A comes with Solana wallet
    let sol_a = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    
    // Step 1: Provision wallet for all chains
    let provision_req = ProvisionRequest {
        solana_pubkey: sol_a.to_string(),
        chain_ids: vec![1, 137, 42161],
    };
    let provision_result = ctx.handle(provision_req).unwrap();
    
    println!("Provisioned wallet: {}", provision_result.evm_address);
    println!("Chain mappings: {:?}", provision_result.chain_mappings);
    
    // Verify all chains have same address
    let default_addr = provision_result.evm_address.clone();
    assert_eq!(provision_result.chain_mappings.get(&1), Some(&default_addr));
    assert_eq!(provision_result.chain_mappings.get(&137), Some(&default_addr));
    assert_eq!(provision_result.chain_mappings.get(&42161), Some(&default_addr));
    
    // Step 2: Later, admin decides to update chain 137 to new address
    let update_req = UpdateMappingRequest {
        solana_pubkey: sol_a.to_string(),
        chain_id: 137,
    };
    let update_result = ctx.handle_update_mapping(update_req).unwrap();
    
    println!("Updated chain 137 to new wallet: {}", update_result.new_evm_address);
    
    // Step 3: Verify final state
    // Chain 1 and 42161 still have default address
    assert_eq!(ctx.get_existing_mapping(sol_a, 1).unwrap(), Some(default_addr.clone()));
    assert_eq!(ctx.get_existing_mapping(sol_a, 42161).unwrap(), Some(default_addr.clone()));
    
    // Chain 137 has new address
    assert_eq!(ctx.get_existing_mapping(sol_a, 137).unwrap(), Some(update_result.new_evm_address.clone()));
    assert_ne!(ctx.get_existing_mapping(sol_a, 137).unwrap(), Some(default_addr));
}

#[test]
fn test_multiple_users_independent() {
    let ctx = TestContext::new();
    
    let sol_a = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let sol_b = "B4fiuy1rJgmbTrraeZpcEtGtFzmt2GVYr1XEoSY7HqqC";
    
    // Provision both users
    let req_a = ProvisionRequest {
        solana_pubkey: sol_a.to_string(),
        chain_ids: vec![1, 137],
    };
    let req_b = ProvisionRequest {
        solana_pubkey: sol_b.to_string(),
        chain_ids: vec![1, 137],
    };
    
    let result_a = ctx.handle(req_a).unwrap();
    let result_b = ctx.handle(req_b).unwrap();
    
    // Different users have different wallets
    assert_ne!(result_a.evm_address, result_b.evm_address);
    
    // Update user A's chain 137
    let update_a = UpdateMappingRequest {
        solana_pubkey: sol_a.to_string(),
        chain_id: 137,
    };
    let update_result_a = ctx.handle_update_mapping(update_a).unwrap();
    
    // User B should be unaffected
    let b_chain_137 = ctx.get_existing_mapping(sol_b, 137).unwrap();
    assert_eq!(b_chain_137, Some(result_b.evm_address.clone()));
    
    // User A's chain 137 should be updated
    let a_chain_137 = ctx.get_existing_mapping(sol_a, 137).unwrap();
    assert_eq!(a_chain_137, Some(update_result_a.new_evm_address));
}
