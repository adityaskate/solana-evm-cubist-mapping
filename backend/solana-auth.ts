/**
 * Backend: Solana Signature Verification + C2F Provisioning Call
 *
 * This file shows the realistic shape of:
 * 1. Generating a nonce for Solana wallet authentication
 * 2. Verifying a Solana signature
 * 3. Calling the Cubist C2F function after successful auth
 *
 * The backend does NOT interact with CubeSigner directly.
 * All key management happens inside C2F.
 */

import { PublicKey } from "@solana/web3.js";
import nacl from "tweetnacl";
import { randomBytes } from "crypto";

interface NonceRecord {
  nonce: string;
  createdAt: number;
  expiresAt: number;
}

interface ProvisionRequest {
  solana_pubkey: string;
  chain_id: number;
}

interface ProvisionResponse {
  evm_address: string;
}

const nonceStore = new Map<string, NonceRecord>();

const NONCE_TTL_MS = 5 * 60 * 1000; // 5 minutes

/**
 * Generate a random nonce for a Solana wallet to sign.
 * Returns a hex-encoded 32-byte nonce.
 */
export function generateNonce(solanaPubkey: string): string {
  const nonce = randomBytes(32).toString("hex");
  const now = Date.now();

  nonceStore.set(solanaPubkey, {
    nonce,
    createdAt: now,
    expiresAt: now + NONCE_TTL_MS,
  });

  return nonce;
}

/**
 * Retrieve and invalidate a nonce (single-use).
 */
function consumeNonce(solanaPubkey: string): string | null {
  const record = nonceStore.get(solanaPubkey);
  if (!record) return null;

  nonceStore.delete(solanaPubkey); // single-use

  if (Date.now() > record.expiresAt) {
    return null; // expired
  }

  return record.nonce;
}

/**
 * Verify that the signature was produced by the claimed Solana wallet
 * over the expected nonce message.
 *
 * @param solanaPubkey - Base58-encoded Solana public key
 * @param signature    - Base64-encoded Ed25519 signature
 * @param message      - The exact message that was signed (nonce)
 */
export function verifySolanaSignature(
  solanaPubkey: string,
  signature: string,
  message: string
): boolean {
  try {
    const pubkeyBytes = new PublicKey(solanaPubkey).toBytes();
    const signatureBytes = Buffer.from(signature, "base64");
    const messageBytes = Buffer.from(message, "utf-8");

    return nacl.sign.detached.verify(
      messageBytes,
      signatureBytes,
      pubkeyBytes
    );
  } catch (err) {
    console.error("Signature verification failed:", err);
    return false;
  }
}

// --------------------------------------------------
// C2F Integration (HTTP call to Cubist C2F endpoint)
// --------------------------------------------------

const C2F_ENDPOINT = process.env.C2F_ENDPOINT||"http://localhost:8080/provision-wallet";

/**
 * Call the Cubist C2F function to provision/fetch an EVM wallet.
 *
 * The backend does NOT talk to CubeSigner directly.
 * C2F handles:
 *   - KV lookup (idempotent read)
 *   - CubeSigner key creation (if needed)
 *   - KV storage (atomic write)
 *
 * Authentication to C2F is handled via service credentials (not shown).
 */
async function callC2FProvision(
  req: ProvisionRequest
): Promise<ProvisionResponse> {
  const response = await fetch(C2F_ENDPOINT, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      // "Authorization": `Bearer ${process.env.C2F_API_KEY}`,
    },
    body: JSON.stringify(req),
  });

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`C2F call failed: ${response.status} - ${body}`);
  }

  return response.json() as Promise<ProvisionResponse>;
}

// --------------------------------------------------
// Main Auth + Provision Flow
// --------------------------------------------------

interface AuthAndProvisionRequest {
  solanaPubkey: string;
  signature: string; // base64
  chainId: number;
}

interface AuthAndProvisionResponse {
  success: boolean;
  evmAddress?: string;
  error?: string;
}

/**
 * Full flow:
 * 1. Verify Solana signature against stored nonce
 * 2. If valid, call C2F to provision/fetch EVM wallet
 * 3. Return EVM address
 */
export async function authenticateAndProvision(
  req: AuthAndProvisionRequest
): Promise<AuthAndProvisionResponse> {
  const { solanaPubkey, signature, chainId } = req;

  // 1. Consume the nonce (single-use)
  const nonce = consumeNonce(solanaPubkey);
  if (!nonce) {
    return {
      success: false,
      error: "Nonce not found or expired. Request a new nonce.",
    };
  }

  // 2. Verify signature
  const isValid = verifySolanaSignature(solanaPubkey, signature, nonce);
  if (!isValid) {
    return {
      success: false,
      error: "Invalid Solana signature.",
    };
  }

  // 3. Call C2F to provision EVM wallet
  try {
    const result = await callC2FProvision({
      solana_pubkey: solanaPubkey,
      chain_id: chainId,
    });

    return {
      success: true,
      evmAddress: result.evm_address,
    };
  } catch (err) {
    console.error("C2F provisioning failed:", err);
    return {
      success: false,
      error: "Wallet provisioning failed. Please retry.",
    };
  }
}
