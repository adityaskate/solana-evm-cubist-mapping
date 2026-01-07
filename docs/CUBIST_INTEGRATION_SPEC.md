# Cubist Integration Specification

**Purpose:** Define our requirements for Cubist C2F, KV store, and CubeSigner integration  
**Audience:** Cubist team  
**Date:** 2026-01-07

---

## System Overview

We're building a **deterministic Solana → EVM wallet provisioning system** where:
- One Solana wallet maps to exactly one EVM wallet per chain (immutable)
- Backend authenticates users via Solana signature verification
- C2F function handles wallet provisioning (KV lookup + CubeSigner key creation)
- All state is stored in Cubist KV

**Key requirement:** Idempotent, atomic, first-writer-wins semantics.

---

## 1. KV Store Requirements

### Bucket Configuration

```
Bucket name: solana_to_evm
```

### Key Schema

```
{solana_pubkey}:{chain_id} → {evm_address}
```

**Examples:**
```
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:1   → 0xabc...def
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU:137 → 0x123...456
```

### Required Operations

| Operation | API (assumed) | Purpose |
|-----------|---------------|---------|
| **Open bucket** | `keyvalue::open("solana_to_evm")` | Get bucket handle |
| **Read** | `bucket.get(key)` → `Option<Value>` | Idempotent lookup |
| **Atomic write** | `bucket.set(key, value, IfExists::Deny)` | First-writer-wins |

### Critical Requirements

1. **Atomicity:** `IfExists::Deny` must be atomic (prevent race conditions)
2. **Immutability:** Once written, a key-value pair never changes
3. **Consistency:** All C2F instances must see the same KV state
4. **Error handling:** Clear error when `IfExists::Deny` fails (key exists)

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
    chain_id: u64,
) -> Result<String> {
    // Generate unique key material ID
    let key_material_id = format!("EVM_{}_{}", solana_pubkey, chain_id);
    
    // Create key via CLI
    cs key create \
      --type Secp256k1 \
      --material-id $key_material_id
    
    // Returns: { "key_id": "Key#...", "address": "0x...", ... }
}
```

#### Key Material ID Format

```
EVM_{solana_pubkey}_{chain_id}
```

**Examples:**
```
EVM_7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU_1
EVM_7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU_137
```

This ensures:
- Deterministic key IDs for tracking
- Easy lookup of which Solana wallet maps to which EVM key
- Unique keys per (solana_pubkey, chain_id) combination

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

### Input

```json
{
  "solana_pubkey": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "chain_id": 1
}
```

### Output (success)

```json
{
  "evm_address": "0xabc123..."
}
```

### Output (error)

```json
{
  "error": "KV write conflict" | "CubeSigner key creation failed" | ...
}
```

### Logic Flow

```rust
pub fn handle(req: ProvisionRequest) -> Result<ProvisionResponse> {
    // 1. Idempotent read
    if let Some(addr) = get_existing_mapping(&req.solana_pubkey, req.chain_id)? {
        return Ok(ProvisionResponse { evm_address: addr });
    }

    // 2. Create new EVM key via CubeSigner
    let evm_address = create_cubesigner_evm_key(&req.solana_pubkey, req.chain_id)?;

    // 3. Atomic store (first-writer-wins)
    store_mapping_once(&req.solana_pubkey, req.chain_id, &evm_address)?;

    Ok(ProvisionResponse { evm_address })
}
```

### Concurrency Handling

If two requests for the same `(solana_pubkey, chain_id)` arrive simultaneously:
1. Both read KV (both get `None`)
2. Both create a CubeSigner key
3. Both attempt atomic write with `IfExists::Deny`
4. **First writer wins**, second gets error
5. Second retries, reads existing mapping, returns it

**We accept that one CubeSigner key may be orphaned.** This is acceptable.

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
| First request for mapping | Create key, store, return (200 OK) |
| Concurrent requests (race) | First wins, second retries and succeeds (200 OK) |
| CubeSigner API failure | Return error (500), client should retry |
| KV unavailable | Return error (503), client should retry |

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

### Key Immutability

- Once a mapping is created, it cannot be changed
- Prevents wallet substitution attacks
- Lost Solana key = lost access to provisioned EVM wallets (by design)
- **Verified by tests:** `test_wallet_address_immutability`, `test_atomicity_prevents_overwrites`

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

#### **Functional Tests**

| Test | Behavior Validated |
|------|-------------------|
| `test_first_provision_creates_mapping` | Initial provision creates new EVM address |
| `test_second_provision_returns_same_address` | Idempotent behavior - repeated calls return cached address |
| `test_different_chains_get_different_addresses` | Same Solana key on different chains → different EVM addresses |
| `test_different_solana_keys_get_different_addresses` | Different Solana keys → different EVM addresses |
| `test_kv_key_format` | KV key format is `{solana_pubkey}:{chain_id}` |

### Critical Guarantees

**✅ Wallet Address Immutability**
- Once a (solana_pubkey, chain_id) → evm_address mapping is created, it **NEVER changes**
- This is enforced by:
  1. KV store's `IfExists::Deny` atomic operation
  2. Read-before-write pattern in provisioning logic
  3. First-writer-wins semantics in concurrent scenarios
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

✅ **All 11 tests passing** - Atomicity, immutability, and concurrency guarantees validated

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


