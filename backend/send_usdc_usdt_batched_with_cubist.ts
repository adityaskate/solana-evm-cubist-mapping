import {
  Connection,
  PublicKey,
  TransactionMessage,
  VersionedTransaction,
} from "@solana/web3.js";
import {
  getAssociatedTokenAddressSync,
  createAssociatedTokenAccountInstruction,
  createTransferInstruction,
} from "@solana/spl-token";
import { execSync } from "child_process";

const RPC = "https://api.mainnet-beta.solana.com";

// Cubist Solana wallet
const FROM = new PublicKey(
  "B4fiuy1rJgmbTrraeZpcEtGtFzmt2GVYr1XEoSY7HqqC"
);

// recipient (can be same wallet for demo)
const TO = new PublicKey(
  "GywwQw8m7YRfrTWabrS6rHMf8HHbFwYjzhYhmBAGrFDW"
);

// Token mints (mainnet)
const USDC_MINT = new PublicKey(
  "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
);

const USDT_MINT = new PublicKey(
  "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
);

// 1 cent (6 decimals)
const AMOUNT = 10_000;

(async () => {
  const connection = new Connection(RPC, "confirmed");

  const { blockhash } = await connection.getLatestBlockhash();

  // ATAs
  const fromUsdcAta = getAssociatedTokenAddressSync(USDC_MINT, FROM);
  const toUsdcAta = getAssociatedTokenAddressSync(USDC_MINT, TO);

  const fromUsdtAta = getAssociatedTokenAddressSync(USDT_MINT, FROM);
  const toUsdtAta = getAssociatedTokenAddressSync(USDT_MINT, TO);

  const instructions = [];

  // Check both sender and recipient ATAs
  const accounts = await connection.getMultipleAccountsInfo([
    fromUsdtAta,  // sender's USDT (might not exist)
    toUsdcAta,    // recipient's USDC
    toUsdtAta,    // recipient's USDT
  ]);

  // Create sender's USDT ATA if needed
  if (!accounts[0]) {
    instructions.push(
      createAssociatedTokenAccountInstruction(
        FROM,
        fromUsdtAta,
        FROM,
        USDT_MINT
      )
    );
  }

  // Create recipient's USDC ATA if needed
  if (!accounts[1]) {
    instructions.push(
      createAssociatedTokenAccountInstruction(
        FROM,
        toUsdcAta,
        TO,
        USDC_MINT
      )
    );
  }

  // Create recipient's USDT ATA if needed
  if (!accounts[2]) {
    instructions.push(
      createAssociatedTokenAccountInstruction(
        FROM,
        toUsdtAta,
        TO,
        USDT_MINT
      )
    );
  }

  // USDC transfer (1¢)
  instructions.push(
    createTransferInstruction(
      fromUsdcAta,
      toUsdcAta,
      FROM,
      AMOUNT
    )
  );

  // USDT transfer (1¢)
  instructions.push(
    createTransferInstruction(
      fromUsdtAta,
      toUsdtAta,
      FROM,
      AMOUNT
    )
  );

  // Build v0 message
  const message = new TransactionMessage({
    payerKey: FROM,
    recentBlockhash: blockhash,
    instructions,
  }).compileToV0Message();

  const messageBase64 = Buffer.from(message.serialize()).toString("base64");

  // Sign with Cubist
  const signOutput = execSync(
    `cs sign --key-id Key#Solana_B4fiuy1rJgmbTrraeZpcEtGtFzmt2GVYr1XEoSY7HqqC --message "${messageBase64}"`
  ).toString();

  const sigHex = JSON.parse(signOutput).signature.replace(/^0x/, "");
  const sigBase64 = Buffer.from(sigHex, "hex").toString("base64");

  // Attach signature + send
  const tx = new VersionedTransaction(message);
  tx.signatures[0] = Buffer.from(sigBase64, "base64");

  const txid = await connection.sendRawTransaction(tx.serialize());
  console.log("TX HASH:", txid);
})();
