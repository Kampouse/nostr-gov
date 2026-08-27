/**
 * useNearWallet.tsx — NEAR wallet context via @hot-labs/near-connect
 */

import { createContext, useContext, useState, useEffect, useCallback, useRef, type ReactNode } from "react";
import { getConnector, type Account, type NearWalletBase } from "../lib/near-wallet";
import { NEAR_NETWORK_ID } from "../lib/constants";

interface NearState {
  accountId: string;
  publicKey: string | null;
  connecting: boolean;
  wallet: NearWalletBase | null;
  connect: () => Promise<void>;
  disconnect: () => Promise<void>;
  signAndSendTransaction: (params: any) => Promise<any>;
}

const NearContext = createContext<NearState | null>(null);

export function useNear(): NearState {
  const ctx = useContext(NearContext);
  if (!ctx) throw new Error("useNear must be used within NearProvider");
  return ctx;
}

export function NearProvider({ children }: { children: ReactNode }) {
  const [accountId, setAccountId] = useState("");
  const [publicKey, setPublicKey] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [wallet, setWallet] = useState<NearWalletBase | null>(null);
  const connectorRef = useRef(getConnector());

  useEffect(() => {
    const c = connectorRef.current;

    c.on("wallet:signIn", async (t) => {
      const source = t.source;
      const acc: Account = t.accounts[0];
      setAccountId(acc.accountId);
      setPublicKey(acc.publicKey ?? null);
      setConnecting(false);

      // store wallet ref
      const w = await c.wallet();
      setWallet(w);
    });

    c.on("wallet:signOut", () => {
      setAccountId("");
      setPublicKey(null);
      setWallet(null);
      setConnecting(false);
    });

    // Auto-restore session
    c.getConnectedWallet().then(({ wallet: w, accounts }) => {
      if (accounts.length > 0) {
        setAccountId(accounts[0].accountId);
        setPublicKey(accounts[0].publicKey ?? null);
        setWallet(w);
      }
    });

    return () => {
      c.removeAllListeners();
    };
  }, []);

  const connect = useCallback(async () => {
    setConnecting(true);
    try {
      await connectorRef.current.connect();
    } catch {
      setConnecting(false);
    }
  }, []);

  const disconnect = useCallback(async () => {
    await connectorRef.current.disconnect();
  }, []);

  const signAndSendTransaction = useCallback(async (params: any) => {
    const w = wallet ?? await connectorRef.current.wallet();
    return w.signAndSendTransaction(params);
  }, [wallet]);

  return (
    <NearContext.Provider value={{ accountId, publicKey, connecting, wallet, connect, disconnect, signAndSendTransaction }}>
      {children}
    </NearContext.Provider>
  );
}
