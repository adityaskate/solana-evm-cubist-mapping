# Cubist Integration Specification

**Purpose:** Define our requirements for Cubist C2F, KV store, and CubeSigner integration  
**Audience:** Cubist team  
**Date:** 2026-01-07

---

## System Overview

We're building a **deterministic Solana → EVM wallet provisioning system** where:
- **By default:** One Solana wallet maps to the same EVM address across all chains
- **Optionally:** Can override the default EVM address for specific chains
- Backend creates EVM keys via CubeSigner CLI
- WASM policy handles KV storage operations (store, get, update)
- All state is stored in Cubist KV

**Architecture:**
```
Backend                          CubeSigner
   │                                 │
   ├── 1. cs key create ────────────►│ (create EVM key)
   │◄── 2. returns 0xABC... ─────────┤
   │                                 │
   ├── 3. invoke policy ────────────►│ (store mapping)
   │      {action: "store",          │
   │       solana_pubkey: "...",     │
   │       chain_ids: [...],         │
   │       evm_address: "0xABC"}     │
   │◄── 4. success ──────────────────┤
```

**Key requirements:** 
- Idempotent, atomic provisioning
- Same EVM address across chains by default (reduces key management complexity)
- Ability to update mappings for specific chains when needed (admin only)

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

We create **Secp256k1 EVM keys** via CubeSigner CLI in the **backend** (not in the policy).

#### Backend Implementation

```bash
# Create EVM key
cs key create --key-type secp --metadata '{"name":"EVM_<user_identifier>"}'
```

**Response:**
```json
{
  "keys": [{
    "key_id": "Key#0xcb373e47d769b06dee02f05c86dd8790e0358aee",
    "key_type": "SecpEthAddr",
    "material_id": "0xcb373e47d769b06dee02f05c86dd8790e0358aee",
    "metadata": { "name": "EVM_TestUser123" },
    "purpose": "Evm"
  }]
}
```

**Key Points:**
- Keys are created **before** invoking the policy
- Backend extracts `material_id` from response
- Policy receives `evm_address` as input parameter
- One EVM key per Solana address by default (chain-agnostic)

#### Key Lifecycle

- **Creation:** Backend creates key when user provisions wallet
- **Storage:** Policy stores mapping in KV
- **Usage:** Signing EVM transactions via separate backend calls
- **Deletion:** Never (keys are permanent)

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

## 4. WASM Policy Specification

The policy runs as WASM on CubeSigner and handles three actions: **store**, **get**, and **update**.

### Policy Deployment

```bash
cd policy
cargo build --release --target wasm32-wasip2
cs policy update --name "skate_wallet_provisioner" target/wasm32-wasip2/release/skate_provisioner.wasm
```

---

### Action 1: Store Mappings

Store mappings for a Solana address across multiple chains (called **after** backend creates EVM key).

#### Input

```json
{
  "action": "store",
  "solana_pubkey": "TestUser123",
  "chain_ids": [1, 137, 42161],
  "evm_address": "0xcb373e47d769b06dee02f05c86dd8790e0358aee"
}
```

#### Output (success)

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

**Behavior:**
- Stores `default:{solana_pubkey}` → `evm_address` (with `IfExists::Deny`)
- Stores `{solana_pubkey}:{chain_id}` → `evm_address` for each chain (with `IfExists::Deny`)
- Idempotent: if mappings exist, returns existing values
- All chains get the same address by default

---

### Action 2: Get Mappings

Retrieve existing mappings for verification.

#### Input

```json
{
  "action": "get",
  "solana_pubkey": "TestUser123",
  "chain_ids": [1, 137, 42161]
}
```

#### Output (success)

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

---

### Action 3: Update Chain Mapping (Admin Only)

Override the EVM address for a specific chain.

#### Input

```json
{
  "action": "update",
  "solana_pubkey": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "chain_id": 137,
  "new_evm_address": "0xb29db776e2f8e38dcb2da1ee6f92dd1208874424"
}
```

#### Output (success)

```json
{
  "success": true,
  "new_evm_address": "0xb29db776e2f8e38dcb2da1ee6f92dd1208874424",
  "chain_id": 137
}
```

**Behavior:**
- Validates EVM address format (0x + 40 hex chars)
- Verifies Solana address has been provisioned (default exists)
- Updates `{solana_pubkey}:{chain_id}` mapping with `IfExists::Overwrite`
- Other chains remain unchanged

---

### Error Responses

```json
{
  "success": false,
  "error": "<error message>"
}
```

**Common errors:**
- `"chain_ids cannot be empty"` (store action)
- `"Invalid EVM address format: <address>"` (store/update actions)
- `"Solana address <pubkey> not provisioned"` (update action)
- `"KV write error: ..."` (storage failures)

---

## 5. Security Considerations

### Solana Signature Verification (Backend)

- Nonces are single-use, time-limited (5 min TTL)
- Ed25519 verification via `tweetnacl.sign.detached.verify`
- No private keys on backend — only signature verification

### Policy Isolation

- Backend creates keys via CubeSigner CLI
- Policy only handles KV operations (store/get/update)
- KV is only accessible from policy (not from public internet)
- Admin update action should have additional authorization checks

### Key Immutability & Flexibility

- Once a default EVM address is created for a Solana pubkey, it remains the default
- Chain-specific mappings can be updated to override the default
- Lost Solana key = lost access to provisioned EVM wallets (by design)
- **Verified by tests:** `test_wallet_address_immutability`, `test_atomicity_prevents_overwrites`, `test_same_address_across_chains_by_default`, `test_update_mapping_for_specific_chain`

### Race Condition Handling

- Atomic `IfExists::Deny` prevents duplicate mappings
- First write wins for default address and chain mappings
- System converges to single mapping per (solana_pubkey, chain_id)
- **Verified by tests:** `test_concurrent_provisions_first_writer_wins`, `test_atomicity_prevents_overwrites_on_provision`

---

## 6. Testing & Validation

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


✅ **All 16 tests passing** - Default address consistency, update functionality, atomicity, and concurrency guarantees validated

### Production Validated ✅

| Component | Status |
|-----------|--------|
| Solana signing | Working with CubeSigner CLI |
| Policy logic | Tested with mock KV + deployed to production |
| Store action | Verified in production |
| Get action | Verified in production |
| Update action | Verified in production |
| Backend auth | Ed25519 verification working |

---

## 7. Code References

- **WASM Policy:** `policy/src/main.rs` (deployed to CubeSigner)
- **Type definitions:** `src/lib.rs` (used by tests)
- **Tests:** `tests/atomicity_tests.rs` (16 tests, all passing)
- **Backend Solana auth:** `backend/solana-auth.ts`
- **Solana signing example:** `backend/send_usdc_usdt_batched_with_cubist.ts`

---

## 8. Live Testing Flow (Production Validated ✅)

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
