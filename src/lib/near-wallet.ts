/**
 * near-wallet.ts — NEAR wallet via @hot-labs/near-connect (zero-dep, sandboxed)
 */

import { NearConnector, type NearWalletBase, type Account, type EventMap } from "@hot-labs/near-connect";
import { NEAR_NETWORK_ID } from "./constants";

let connector: NearConnector | null = null;

export function getConnector(): NearConnector {
  if (!connector) {
    connector = new NearConnector({
      network: NEAR_NETWORK_ID as "testnet" | "mainnet",
      features: { signMessage: true, testnet: true, signAndSendTransaction: true },
    });
  }
  return connector;
}

export type { NearConnector, NearWalletBase, Account };
