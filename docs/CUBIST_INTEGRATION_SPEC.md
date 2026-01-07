# Cubist Integration Specification

**Purpose:** Define our requirements for Cubist C2F, KV store, and CubeSigner integration  
**Audience:** Cubist team  
**Date:** 2026-01-07

---

## System Overview

We're building a **deterministic Solana → EVM wallet provisioning system** where:
- **By default:** One Solana wallet maps to the same EVM address across all chains
- **Optionally:** Can override the default EVM address for specific chains
- Backend authenticates users via Solana signature verification
- C2F function handles wallet provisioning (KV lookup + CubeSigner key creation)
- All state is stored in Cubist KV

**Key requirements:** 
- Idempotent, atomic provisioning
- Same EVM address across chains by default (reduces key management complexity)
- Ability to update mappings for specific chains when needed

---

## 1. KV Store Requirements

### Bucket Configuration

```
Bucket name: solana_to_evm
```

### Key Schema

```
default:{solana_pubkey} → {evm_address}              # Default address used across all chains
{solana_pubkey}:{chain_id} → {evm_address}           # Chain-specific override (optional)
```

**Examples:**
```
default:7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU → 0xabc...def  # Used for all chains by default
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:137 → 0x123...456    # Polygon-specific override
```

### Required Operations

| Operation | API (assumed) | Purpose |
|-----------|---------------|---------|
| **Open bucket** | `keyvalue::open("solana_to_evm")` | Get bucket handle |
| **Read** | `bucket.get(key)` → `Option<Value>` | Idempotent lookup |
| **Atomic write** | `bucket.set(key, value, IfExists::Deny)` | First-writer-wins for defaults |
| **Update** | `bucket.set(key, value, IfExists::Allow)` | Update chain-specific mapping |

### Critical Requirements

1. **Atomicity:** `IfExists::Deny` must be atomic (prevent race conditions)
2. **Default Immutability:** Once a default address is created, it should not change
3. **Chain Override Flexibility:** Individual chain mappings can be updated
4. **Consistency:** All C2F instances must see the same KV state
5. **Error handling:** Clear error when `IfExists::Deny` fails (key exists)

### Questions for Cubist

- Is `IfExists::Deny` implemented as compare-and-swap or equivalent?
- What is the consistency model? (strong vs eventual)
- Is there a TTL/expiration mechanism? (we don't need it, but want to ensure mappings are permanent)
- Bucket creation: manual via UI or programmatic?

---

## 2. CubeSigner Integration

### Key Management

We create **Secp256k1 EVM keys** dynamically using CubeSigner CLI.

#### Implementation

```rust
fn create_cubesigner_evm_key(
    solana_pubkey: &str,
) -> Result<String> {
    // Generate unique key material ID (one per Solana address, not per chain)
    let key_material_id = format!("EVM_{}", solana_pubkey);
    
    // Create key via CLI
    cs key create \
      --type Secp256k1 \
      --material-id $key_material_id
    
    // Returns: { "key_id": "Key#...", "address": "0x...", ... }
}
```

#### Key Material ID Format

```
EVM_{solana_pubkey}
```

**Examples:**
```
EVM_7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU
EVM_B4fiuy1rJgmbTrraeZpcEtGtFzmt2GVYr1XEoSY7HqqC
```

**Note:** One EVM key is created per Solana address (chain-agnostic). This key is used across all EVM chains by default, simplifying key management.

#### Key Lifecycle

- **Creation:** On-demand when provisioning a new Solana → EVM mapping
- **Usage:** Signing EVM transactions via separate backend calls (not part of provisioning)
- **Deletion:** Never (keys are permanent)

### Questions for Cubist

- Does `cs key create --type Secp256k1` work in C2F environment or only locally?
- Is the output format consistent (JSON with `address` field)?
- Can we set custom `material-id` for tracking purposes?
- How do we authenticate C2F → CubeSigner calls? (implicit via C2F runtime?)

---

## 3. Solana Signing (Already Working)

We have validated Solana signing with CubeSigner CLI:

```bash
cs sign \
  --key-id Key#Solana_B4fiuy1rJgmbTrraeZpcEtGtFzmt2GVYr1XEoSY7HqqC \
  --message "<base64-encoded-message>"
```

**Output format:**
```json
{
  "signature": "0x<hex>",
  ...
}
```

### Message Format

For Solana transactions, we sign the **serialized TransactionMessage**:

```typescript
const message = new TransactionMessage({
  payerKey: solanaPubkey,
  recentBlockhash: blockhash,
  instructions: [...]
}).compileToV0Message();

const messageBytes = message.serialize();
const messageBase64 = Buffer.from(messageBytes).toString("base64");
```

### Signature Handling

```typescript
// Convert CubeSigner hex signature → base64 for Solana
const sigHex = csOutput.signature.replace(/^0x/, "");
const sigBase64 = Buffer.from(sigHex, "hex").toString("base64");

// Attach to transaction
const tx = new VersionedTransaction(message);
tx.signatures[0] = Buffer.from(sigBase64, "base64");
```

**This is working correctly.** No changes needed to Solana signing.

---

## 4. C2F Function Specification

### Provision Function

#### Input

```json
{
  "solana_pubkey": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "chain_id": 1
}
```

#### Output (success)

```json
{
  "evm_address": "0xabc123..."
}
```

#### Logic Flow

```rust
pub fn handle(req: ProvisionRequest) -> Result<ProvisionResponse> {
    // 1. Check if chain-specific mapping exists → return it
    if let Some(addr) = get_existing_mapping(&req.solana_pubkey, req.chain_id)? {
        return Ok(ProvisionResponse { evm_address: addr });
    }

    // 2. Check if default EVM address exists (same across all chains)
    let evm_address = if let Some(addr) = get_default_evm_address(&req.solana_pubkey)? {
        addr
    } else {
        // 3. Create new EVM key (one per Solana address)
        let addr = create_cubesigner_evm_key(&req.solana_pubkey)?;
        
        // Store as default address
        store_default_evm_address(&req.solana_pubkey, &addr)?;
        
        addr
    };

    // 4. Store chain-specific mapping (points to default address)
    store_mapping_once(&req.solana_pubkey, req.chain_id, &evm_address)?;

    Ok(ProvisionResponse { evm_address })
}
```

**Behavior:**
- First provision for any chain creates one EVM key
- Subsequent provisions for other chains reuse the same EVM address
- Idempotent: calling multiple times returns the same address

---

### Update Mapping Function

#### Input

```json
{
  "solana_pubkey": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "chain_id": 137,
  "new_evm_address": "0x1234567890abcdef1234567890abcdef12345678"
}
```

#### Output (success)

```json
{
  "success": true,
  "evm_address": "0x1234567890abcdef1234567890abcdef12345678"
}
```

#### Logic Flow

```rust
pub fn handle_update_mapping(req: UpdateMappingRequest) -> Result<UpdateMappingResponse> {
    // Validate EVM address format
    if !req.new_evm_address.starts_with("0x") || req.new_evm_address.len() != 42 {
        return Err(anyhow!("Invalid EVM address format"));
    }

    // Update the mapping (allows overwrite)
    update_mapping(&req.solana_pubkey, req.chain_id, &req.new_evm_address)?;

    Ok(UpdateMappingResponse {
        success: true,
        evm_address: req.new_evm_address,
    })
}
```

**Use Case:** Override the default EVM address for a specific chain (e.g., use a different address on Polygon while keeping the same address on Ethereum and Arbitrum)

---

### Output (error)

```json
{
  "error": "KV write conflict" | "CubeSigner key creation failed" | "Invalid EVM address format" | ...
}
```

### Concurrency Handling

If two requests for the same `solana_pubkey` arrive simultaneously (different or same chains):
1. Both read default address (both get `None`)
2. Both create a CubeSigner key
3. Both attempt atomic write with `IfExists::Deny` on default address
4. **First writer wins**, second gets error
5. Second retries, reads existing default address, uses it

**We accept that one CubeSigner key may be orphaned.** This is acceptable and happens rarely.

---

## 5. Backend → C2F Integration

### Authentication

**Our assumption:** Backend authenticates to C2F via one of:
- API key in `Authorization` header
- mTLS (mutual TLS)
- JWT from Cubist identity provider

### Error Handling

| Scenario | Expected Behavior |
|----------|-------------------|
| Idempotent request (mapping exists) | Return cached address (200 OK) |
| First request for Solana address | Create key, store default, return (200 OK) |
| Request for new chain (default exists) | Reuse default address, return (200 OK) |
| Update mapping | Override chain-specific mapping, return (200 OK) |
| Concurrent requests (race) | First wins, second retries and succeeds (200 OK) |
| CubeSigner API failure | Return error (500), client should retry |
| KV unavailable | Return error (503), client should retry |
| Invalid EVM address in update | Return error (400), client fixes input |

---

## 6. Deployment & Environment

### C2F Deployment

- **Runtime:** Cubist-managed (WASM? serverless?)
- **Code:** We provide `src/lib.rs` compiled to C2F-compatible format
- **Dependencies:** Cubist C2F SDK (KV, CubeSigner client)

### Backend Deployment

- **Runtime:** Node.js on our infrastructure
- **Dependencies:** `@solana/web3.js`, `tweetnacl`
- **Secrets:** C2F API key/credentials (stored in our secret manager)


## 7. Security Considerations

### Solana Signature Verification (Backend)

- Nonces are single-use, time-limited (5 min TTL)
- Ed25519 verification via `tweetnacl.sign.detached.verify`
- No private keys on backend — only signature verification

### C2F Isolation

- Backend never touches CubeSigner credentials
- C2F is the only component with CubeSigner access
- KV is only accessible from C2F (not from public internet)

### Key Immutability & Flexibility

- Once a default EVM address is created for a Solana pubkey, it remains the default
- Chain-specific mappings can be updated to override the default
- Lost Solana key = lost access to provisioned EVM wallets (by design)
- **Verified by tests:** `test_wallet_address_immutability`, `test_atomicity_prevents_overwrites`, `test_same_address_across_chains_by_default`, `test_update_mapping_for_specific_chain`

### Race Condition Handling

- Concurrent provisions may create multiple CubeSigner keys (acceptable)
- Only first write to KV succeeds (atomic `IfExists::Deny`)
- Losing requests retry and receive the winning address
- System eventually converges to single mapping per (solana_pubkey, chain_id)
- **Verified by tests:** `test_concurrent_provisions_first_writer_wins`, `test_retry_after_race_condition`

---

## 8. Testing & Validation

### Test Coverage

Comprehensive tests validate the following critical properties:

#### **Atomicity & Immutability Tests**

| Test | Property Validated |
|------|-------------------|
| `test_atomicity_prevents_overwrites` | Once a mapping is stored, it cannot be overwritten (IfExists::Deny works) |
| `test_cannot_delete_and_recreate_mapping` | Mappings cannot be deleted - storage is immutable |
| `test_atomicity_with_delete_attempt_before_write` | Attempting to delete then recreate fails - original mapping preserved |
| `test_wallet_address_immutability` | Multiple provisions for the same (solana_pubkey, chain_id) always return the same EVM address |
| `test_concurrent_provisions_first_writer_wins` | Concurrent requests for the same mapping result in one winner, all get same address |
| `test_retry_after_race_condition` | Lost race conditions (orphaned keys) are handled correctly on retry |

#### **Default Address Behavior Tests**

| Test | Behavior Validated |
|------|-------------------|
| `test_same_address_across_chains_by_default` | Same Solana key provisioned on multiple chains gets the same EVM address |
| `test_update_mapping_for_specific_chain` | Can override default address for specific chains |
| `test_update_mapping_validates_address_format` | Update function validates EVM address format |

#### **Functional Tests**

| Test | Behavior Validated |
|------|-------------------|
| `test_first_provision_creates_mapping` | Initial provision creates new EVM address |
| `test_second_provision_returns_same_address` | Idempotent behavior - repeated calls return cached address |
| `test_different_chains_get_different_addresses` | Same Solana key can have different addresses if updated per chain |
| `test_different_solana_keys_get_different_addresses` | Different Solana keys → different EVM addresses |
| `test_kv_key_format` | KV key format is correct |

### Critical Guarantees

**✅ Default Address Consistency**
- One Solana address → one default EVM address (used across all chains)
- Simplifies key management and reduces CubeSigner key count
- Default address cannot be changed once created

**✅ Chain-Specific Flexibility**
- Individual chain mappings can be updated to override the default
- Useful for special cases (e.g., using a different address on one chain)
- Updates are atomic and validated

**✅ Wallet Address Behavior**
- By default: Same EVM address across all chains
- With updates: Can customize per chain
- This is enforced by:
  1. KV store's atomic operations
  2. Separate default and chain-specific keys
  3. Update function with validation
  4. **No delete operation** - mappings cannot be removed or reversed
- **Security implication:** Prevents wallet substitution attacks and revert-then-recreate attacks
- **Verified by tests:** `test_wallet_address_immutability`, `test_cannot_delete_and_recreate_mapping`, `test_atomicity_with_delete_attempt_before_write`

**✅ Atomicity Under Concurrency**
- Multiple simultaneous requests for the same mapping:
  - All create CubeSigner keys (unavoidable in distributed system)
  - Only one write succeeds (atomic KV operation)
  - Losers retry and get the winning address
  - Orphaned keys are acceptable (tradeoff for atomicity)

**✅ Idempotency**
- Calling provision with same parameters 1 time or 1000 times produces identical result
- No side effects after first successful provision
- Safe for retries at any layer (backend, C2F, KV)

### Test Execution

```bash
# Run all tests
cargo test

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test test_wallet_address_immutability
```

**Test Results:**
<img width="984" height="603" alt="image" src="https://github.com/user-attachments/assets/35318094-c1a2-44a3-8211-b5b22eee3f6d" />


✅ **All 14 tests passing** - Default address consistency, update functionality, atomicity, and concurrency guarantees validated

### Already Validated

| Component | Method |
|-----------|--------|
| Solana signing | Working end-to-end with CubeSigner CLI |
| C2F logic | Tested locally with mocked KV store |
| Concurrency | Stress-tested first-writer-wins semantics |
| Backend auth | Ed25519 verification working |

### Pending Validation

| Component | Blocker |
|-----------|---------|
| Real KV operations | Need C2F runtime access |
| CubeSigner key creation | Need API documentation |
| End-to-end flow | Need deployed C2F endpoint |

---

## 9. Open Questions Summary

**KV Store:**
1. Exact API for `IfExists::Deny` (atomicity guarantees)
2. Consistency model (strong vs eventual)
3. Bucket creation process (manual or programmatic)

**CubeSigner:**
4. API for creating Secp256k1 keys from C2F
5. Key metadata support
6. Rate limits on key creation
7. Authentication mechanism (C2F → CubeSigner)

**C2F:**
8. Deployment process and tooling
9. Staging environment availability
10. Backend → C2F authentication mechanism (API key, mTLS, JWT)
11. Retry logic (built-in or manual)
12. Function versioning/rollback support

---

## Appendix: Code References

- **C2F provisioning logic:** `src/lib.rs`
- **Backend Solana auth:** `backend-example/solana-auth.ts`
- **Working Solana signing example:** `backend-example/send_usdc_usdt_batched_with_cubist.ts`

---

## 10. Live Testing Flow (Production Validated ✅)

The complete provisioning system has been tested on production CubeSigner. Below is the step-by-step flow with commands.

### Step 1: Create EVM Key

Create a Secp256k1 EVM key via CubeSigner CLI:

```bash
cs key create --key-type secp --metadata '{"name":"EVM_TestUser123"}'
```

**Output:**
```json
{
  "keys": [{
    "metadata": { "name": "EVM_TestUser123" },
    "key_id": "Key#0xcb373e47d769b06dee02f05c86dd8790e0358aee",
    "key_type": "SecpEthAddr",
    "material_id": "0xcb373e47d769b06dee02f05c86dd8790e0358aee",
    "purpose": "Evm"
  }]
}
```

<img width="2940" height="874" alt="image" src="https://github.com/user-attachments/assets/8ec95c40-d537-4274-b2b4-a74becadda62" />



---

### Step 2: Store Mapping

Invoke the policy to store mappings for multiple chains:

```bash
cs policy invoke --name "skate_wallet_provisioner" --key-id "Key#0x7404906e09deb5de2cf22b1693337f9ba6c36237" \
  '{"action":"store","solana_pubkey":"TestUser123","chain_ids":[1,137,42161],"evm_address":"0xcb373e47d769b06dee02f05c86dd8790e0358aee"}'
```

**Response:**
```json
{
  "success": true,
  "evm_address": "0xcb373e47d769b06dee02f05c86dd8790e0358aee",
  "chain_mappings": {
    "1": "0xcb373e47d769b06dee02f05c86dd8790e0358aee",
    "137": "0xcb373e47d769b06dee02f05c86dd8790e0358aee",
    "42161": "0xcb373e47d769b06dee02f05c86dd8790e0358aee"
  }
}
```

<img width="2926" height="596" alt="image" src="https://github.com/user-attachments/assets/23ebd81e-ad92-4f22-b6c0-f1aaba7f6088" />

---

### Step 3: Get Mappings

Verify the stored mappings:

```bash
cs policy invoke --name "skate_wallet_provisioner" --key-id "Key#0x7404906e09deb5de2cf22b1693337f9ba6c36237" \
  '{"action":"get","solana_pubkey":"TestUser123","chain_ids":[1,137,42161]}'
```

**Response:**
```json
{
  "success": true,
  "default_address": "0xcb373e47d769b06dee02f05c86dd8790e0358aee",
  "chain_mappings": {
    "1": "0xcb373e47d769b06dee02f05c86dd8790e0358aee",
    "137": "0xcb373e47d769b06dee02f05c86dd8790e0358aee",
    "42161": "0xcb373e47d769b06dee02f05c86dd8790e0358aee"
  }
}
```

✅ All chains map to the same EVM address!

<img width="2924" height="514" alt="image" src="https://github.com/user-attachments/assets/bb902613-71a7-468a-aa4a-a004817d7809" />

---

### Step 4: Create New EVM Key (for chain update)

Create a new key specifically for updating chain 137:

```bash
cs key create --key-type secp --metadata '{"name":"EVM_7xKXtg_chain137"}'
```

**Output:**
```json
{
  "keys": [{
    "metadata": { "name": "EVM_7xKXtg_chain137" },
    "key_id": "Key#0xb29db776e2f8e38dcb2da1ee6f92dd1208874424",
    "key_type": "SecpEthAddr",
    "material_id": "0xb29db776e2f8e38dcb2da1ee6f92dd1208874424",
    "purpose": "Evm"
  }]
}
```

<img width="2926" height="800" alt="image" src="https://github.com/user-attachments/assets/95c46707-77c6-49e0-be3d-7339e6bb5771" />

---

### Step 5: Update Chain 137

Update only chain 137 with the new EVM address:

```bash
cs policy invoke --name "skate_wallet_provisioner" --key-id "Key#0x7404906e09deb5de2cf22b1693337f9ba6c36237" \
  '{"action":"update","solana_pubkey":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU","chain_id":137,"new_evm_address":"0xb29db776e2f8e38dcb2da1ee6f92dd1208874424"}'
```

**Response:**
```json
{
  "success": true,
  "new_evm_address": "0xb29db776e2f8e38dcb2da1ee6f92dd1208874424",
  "chain_id": 137
}
```

<img width="2940" height="632" alt="image" src="https://github.com/user-attachments/assets/f44f4d74-a590-4039-b2da-48b302048b36" />

---

### Step 6: Verify Update

Confirm the update worked - chain 137 should have the new address:

```bash
cs policy invoke --name "skate_wallet_provisioner" --key-id "Key#0x7404906e09deb5de2cf22b1693337f9ba6c36237" \
  '{"action":"get","solana_pubkey":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU","chain_ids":[1,137,42161]}'
```

**Response:**
```json
{
  "success": true,
  "default_address": "0x7404906e09deb5de2cf22b1693337f9ba6c36237",
  "chain_mappings": {
    "1": "0x7404906e09deb5de2cf22b1693337f9ba6c36237",
    "137": "0xb29db776e2f8e38dcb2da1ee6f92dd1208874424",
    "42161": "0x7404906e09deb5de2cf22b1693337f9ba6c36237"
  }
}
```

✅ **Chain 137 updated to `0xb29db...`, chains 1 and 42161 unchanged!**

<img width="2940" height="460" alt="image" src="https://github.com/user-attachments/assets/c8fa4de8-cfe6-4f0b-b484-756c246a7bc8" />

---
