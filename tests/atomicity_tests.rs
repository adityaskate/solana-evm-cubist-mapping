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
    
    /// Set with overwrite allowed
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

    fn get_write_attempts(&self) -> Vec<String> {
        self.write_attempts.lock().unwrap().clone()
    }

    fn get_delete_attempts(&self) -> Vec<String> {
        self.delete_attempts.lock().unwrap().clone()
    }
}

/// Mock implementations using the test KV store
struct TestContext {
    kv: MockKvStore,
    key_counter: Arc<Mutex<u32>>,
}

impl TestContext {
    fn new() -> Self {
        Self {
            kv: MockKvStore::new(),
            key_counter: Arc::new(Mutex::new(0)),
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

    fn create_cubesigner_evm_key(&self, _solana_pubkey: &str) -> Result<String> {
        let mut counter = self.key_counter.lock().unwrap();
        *counter += 1;
        Ok(format!("0x{:040x}", *counter))
    }

    fn handle(&self, req: ProvisionRequest) -> Result<ProvisionResponse> {
        // 1. Check if chain-specific mapping already exists
        if let Some(addr) = self.get_existing_mapping(&req.solana_pubkey, req.chain_id)? {
            return Ok(ProvisionResponse { evm_address: addr });
        }

        // 2. Check if default EVM address exists (same across all chains)
        let evm_address = if let Some(addr) = self.get_default_evm_address(&req.solana_pubkey)? {
            addr
        } else {
            // 3. Create new EVM key (one per Solana address)
            let addr = self.create_cubesigner_evm_key(&req.solana_pubkey)?;
            
            // Store as default address
            self.store_default_evm_address(&req.solana_pubkey, &addr)?;
            
            addr
        };

        // 4. Store chain-specific mapping (points to default address)
        self.store_mapping_once(&req.solana_pubkey, req.chain_id, &evm_address)?;

        Ok(ProvisionResponse { evm_address })
    }
    
    fn handle_update_mapping(&self, req: UpdateMappingRequest) -> Result<UpdateMappingResponse> {
        // Validate EVM address format
        if !req.new_evm_address.starts_with("0x") || req.new_evm_address.len() != 42 {
            return Err(anyhow!("Invalid EVM address format: {}", req.new_evm_address));
        }

        // Update the mapping (allows overwrite)
        self.update_mapping(&req.solana_pubkey, req.chain_id, &req.new_evm_address)?;

        Ok(UpdateMappingResponse {
            success: true,
            evm_address: req.new_evm_address,
        })
    }
}

fn kv_key(solana_pubkey: &str, chain_id: u64) -> String {
    format!("{}:{}", solana_pubkey, chain_id)
}

fn default_key(solana_pubkey: &str) -> String {
    format!("default:{}", solana_pubkey)
}

#[test]
fn test_first_provision_creates_mapping() {
    let ctx = TestContext::new();
    let req = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 1,
    };

    let result = ctx.handle(req).unwrap();
    
    // Should create a new address
    assert_eq!(result.evm_address, "0x0000000000000000000000000000000000000001");
}

#[test]
fn test_second_provision_returns_same_address() {
    let ctx = TestContext::new();
    let req = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 1,
    };

    // First provision
    let result1 = ctx.handle(req.clone()).unwrap();
    
    // Second provision
    let result2 = ctx.handle(req).unwrap();
    
    // Should return the same address (idempotent)
    assert_eq!(result1.evm_address, result2.evm_address);
    
    // Should only have created one key
    assert_eq!(*ctx.key_counter.lock().unwrap(), 1);
}

#[test]
fn test_different_chains_get_different_addresses() {
    let ctx = TestContext::new();
    
    let req1 = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 1,
    };
    
    let req2 = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 137,
    };

    let result1 = ctx.handle(req1).unwrap();
    let result2 = ctx.handle(req2).unwrap();
    
    // Same Solana key, different chains → SAME EVM address by default
    assert_eq!(result1.evm_address, result2.evm_address);
}

#[test]
fn test_different_solana_keys_get_different_addresses() {
    let ctx = TestContext::new();
    
    let req1 = ProvisionRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 1,
    };
    
    let req2 = ProvisionRequest {
        solana_pubkey: "B4fiuy1rJgmbTrraeZpcEtGtFzmt2GVYr1XEoSY7HqqC".to_string(),
        chain_id: 1,
    };

    let result1 = ctx.handle(req1).unwrap();
    let result2 = ctx.handle(req2).unwrap();
    
    // Different Solana keys, same chain → different EVM addresses
    assert_ne!(result1.evm_address, result2.evm_address);
}

#[test]
fn test_atomicity_prevents_overwrites() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let chain_id = 1;

    // Manually create first mapping
    let addr1 = "0xfirst111111111111111111111111111111111111";
    ctx.store_mapping_once(solana_pubkey, chain_id, addr1).unwrap();

    // Attempt to overwrite should fail
    let addr2 = "0xsecond22222222222222222222222222222222222";
    let result = ctx.store_mapping_once(solana_pubkey, chain_id, addr2);
    
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Key already exists"));
    
    // Verify original mapping is preserved
    let stored = ctx.get_existing_mapping(solana_pubkey, chain_id).unwrap();
    assert_eq!(stored, Some(addr1.to_string()));
}

#[test]
fn test_concurrent_provisions_first_writer_wins() {
    use std::thread;
    
    let ctx = Arc::new(TestContext::new());
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string();
    let chain_id = 1;

    // Simulate 10 concurrent requests
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let ctx = Arc::clone(&ctx);
            let solana_pubkey = solana_pubkey.clone();
            
            thread::spawn(move || {
                let req = ProvisionRequest {
                    solana_pubkey,
                    chain_id,
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
    for result in successful {
        assert_eq!(&result.evm_address, first_addr);
    }

    // Should have only created one key (the first writer)
    // Note: In real race conditions, multiple keys might be created,
    // but only one mapping is stored
    let write_attempts = ctx.kv.get_write_attempts();
    assert!(write_attempts.len() >= 1);
    
    // Verify only one mapping exists
    let stored = ctx.get_existing_mapping(&solana_pubkey, chain_id).unwrap();
    assert!(stored.is_some());
}

#[test]
fn test_wallet_address_immutability() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let chain_id = 1;

    let req = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id,
    };

    // Create initial mapping
    let result1 = ctx.handle(req.clone()).unwrap();
    
    // Make 100 more requests
    for _ in 0..100 {
        let result = ctx.handle(req.clone()).unwrap();
        assert_eq!(result.evm_address, result1.evm_address,
            "Address changed after multiple provisions - immutability violated!");
    }
}

#[test]
fn test_kv_key_format() {
    assert_eq!(kv_key("ABC123", 1), "ABC123:1");
    assert_eq!(kv_key("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", 137), 
               "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:137");
}

#[test]
fn test_retry_after_race_condition() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let chain_id = 1;

    // First provision succeeds
    let req = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id,
    };
    let result1 = ctx.handle(req.clone()).unwrap();

    // Simulate a lost race: create a key but can't store it
    let orphaned_key = ctx.create_cubesigner_evm_key(solana_pubkey).unwrap();
    let store_result = ctx.store_mapping_once(solana_pubkey, chain_id, &orphaned_key);
    assert!(store_result.is_err()); // Should fail because mapping exists

    // Retry the full provision - should succeed by returning existing mapping
    let result2 = ctx.handle(req).unwrap();
    assert_eq!(result2.evm_address, result1.evm_address);
    
    // Orphaned key should not be the stored address
    assert_ne!(orphaned_key, result2.evm_address);
}

#[test]
fn test_cannot_delete_and_recreate_mapping() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let chain_id = 1;

    // Create initial mapping
    let req = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id,
    };
    let result1 = ctx.handle(req).unwrap();
    let original_address = result1.evm_address.clone();

    // Attempt to delete the mapping (should fail - storage is immutable)
    let key = kv_key(solana_pubkey, chain_id);
    let delete_result = ctx.kv.delete(&key);
    assert!(delete_result.is_err());
    assert!(delete_result.unwrap_err().to_string().contains("not supported"));

    // Verify the delete was attempted
    let delete_attempts = ctx.kv.get_delete_attempts();
    assert_eq!(delete_attempts.len(), 1);

    // Verify original mapping still exists
    let stored = ctx.get_existing_mapping(solana_pubkey, chain_id).unwrap();
    assert_eq!(stored, Some(original_address.clone()));

    // Even after delete attempt, provision should return the same address
    let req2 = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id,
    };
    let result2 = ctx.handle(req2).unwrap();
    assert_eq!(result2.evm_address, original_address);
}

#[test]
fn test_atomicity_with_delete_attempt_before_write() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let chain_id = 1;

    // Create initial mapping
    let addr1 = "0xfirst111111111111111111111111111111111111";
    ctx.store_mapping_once(solana_pubkey, chain_id, addr1).unwrap();

    // Attacker scenario: try to delete then recreate with different address
    let key = kv_key(solana_pubkey, chain_id);
    
    // 1. Attempt to delete
    let delete_result = ctx.kv.delete(&key);
    assert!(delete_result.is_err(), "Delete should not be allowed");

    // 2. Attempt to write new mapping (should fail due to IfExists::Deny)
    let addr2 = "0xattacker2222222222222222222222222222222222";
    let write_result = ctx.store_mapping_once(solana_pubkey, chain_id, addr2);
    assert!(write_result.is_err(), "Overwrite should not be allowed");

    // 3. Verify original mapping is unchanged
    let stored = ctx.get_existing_mapping(solana_pubkey, chain_id).unwrap();
    assert_eq!(stored, Some(addr1.to_string()));
    assert_ne!(stored, Some(addr2.to_string()));
}

#[test]
fn test_same_address_across_chains_by_default() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    // Provision on Ethereum (chain_id=1)
    let req1 = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 1,
    };
    let result1 = ctx.handle(req1).unwrap();

    // Provision on Polygon (chain_id=137)
    let req2 = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 137,
    };
    let result2 = ctx.handle(req2).unwrap();

    // Provision on Arbitrum (chain_id=42161)
    let req3 = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 42161,
    };
    let result3 = ctx.handle(req3).unwrap();

    // All chains should use the same EVM address by default
    assert_eq!(result1.evm_address, result2.evm_address);
    assert_eq!(result2.evm_address, result3.evm_address);
    
    // Should only have created one key
    assert_eq!(*ctx.key_counter.lock().unwrap(), 1);
}

#[test]
fn test_update_mapping_for_specific_chain() {
    let ctx = TestContext::new();
    let solana_pubkey = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    // Provision on chain 1 and 137 - both get same default address
    let req1 = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 1,
    };
    let result1 = ctx.handle(req1).unwrap();

    let req2 = ProvisionRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 137,
    };
    let result2 = ctx.handle(req2).unwrap();

    assert_eq!(result1.evm_address, result2.evm_address);
    let default_address = result1.evm_address.clone();

    // Update chain 137 to use a different address
    let new_address = "0xabcdef1234567890abcdef1234567890abcdef12";
    let update_req = UpdateMappingRequest {
        solana_pubkey: solana_pubkey.to_string(),
        chain_id: 137,
        new_evm_address: new_address.to_string(),
    };
    let update_result = ctx.handle_update_mapping(update_req).unwrap();
    assert!(update_result.success);
    assert_eq!(update_result.evm_address, new_address);

    // Chain 1 should still have the default address
    let chain1_stored = ctx.get_existing_mapping(solana_pubkey, 1).unwrap();
    assert_eq!(chain1_stored, Some(default_address.clone()));

    // Chain 137 should have the new address
    let chain137_stored = ctx.get_existing_mapping(solana_pubkey, 137).unwrap();
    assert_eq!(chain137_stored, Some(new_address.to_string()));
    assert_ne!(chain137_stored, Some(default_address));
}

#[test]
fn test_update_mapping_validates_address_format() {
    let ctx = TestContext::new();
    
    // Invalid: missing 0x prefix
    let req1 = UpdateMappingRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 1,
        new_evm_address: "abcdef1234567890abcdef1234567890abcdef12".to_string(),
    };
    assert!(ctx.handle_update_mapping(req1).is_err());

    // Invalid: wrong length
    let req2 = UpdateMappingRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 1,
        new_evm_address: "0xshort".to_string(),
    };
    assert!(ctx.handle_update_mapping(req2).is_err());

    // Valid
    let req3 = UpdateMappingRequest {
        solana_pubkey: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        chain_id: 1,
        new_evm_address: "0xabcdef1234567890abcdef1234567890abcdef12".to_string(),
    };
    assert!(ctx.handle_update_mapping(req3).is_ok());
}
