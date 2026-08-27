//! NEP-141 Fungible Token receiver and balance tracking.
//!
//! Allows the multisig contract to receive, hold, and send FTs (USDC, USDT, etc.)
//! per wallet. Each wallet tracks its own FT balances independently.
//! Supports token allowlist to prevent griefing.

use crate::*;

/// FT balance key: wallet_name + token_account_id
pub(crate) fn ft_balance_key(wallet: &str, token: &str) -> String {
    format!("{}:ft:{}", wallet, token)
}

/// Check if a token is in the wallet's allowlist.
/// Empty allowlist = accept all.
pub(crate) fn is_token_allowed(wallet: &Wallet, token: &AccountId) -> bool {
    wallet.allowed_tokens.is_empty() || wallet.allowed_tokens.contains(token)
}

/// Storage cost per FT entry (key ~70 bytes + u128 value 16 bytes + overhead)
const FT_ENTRY_STORAGE_BYTES: u64 = 100;
/// YoctoNEAR per byte (NEAR's standard storage rate)
const STORAGE_COST_PER_BYTE_YOCTO: u128 = 10_000_000_000_000; // 10^13

#[near_bindgen]
impl Contract {
    /// NEP-141 `ft_on_transfer` callback.
    /// Callers transfer FTs to this contract with `msg` = wallet name.
    /// Returns unused amount (always "0" — we accept all tokens).
    ///
    /// Enforces:
    /// - Token must be on the wallet's allowlist (or allowlist is empty = open)
    /// - Sufficient storage deposit to track a new token
    pub fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        self.assert_not_paused();
        let token_account = env::predecessor_account_id();
        let wallet_name = if msg.is_empty() {
            sender_id.as_str().to_string()
        } else {
            msg
        };

        let mut wallet = self.wallet_get(&wallet_name)
            .unwrap_or_else(|| env::panic_str(&format!("ERR_WALLET_NOT_FOUND: {}", wallet_name)));

        // Check token allowlist
        assert!(
            is_token_allowed(&wallet, &token_account),
            "ERR_TOKEN_NOT_ALLOWED: {} not in allowlist for '{}'",
            token_account, wallet_name
        );

        // Check if this is a new token — charge storage if so
        let bkey = ft_balance_key(&wallet_name, token_account.as_str());
        let is_new_token = env::storage_read(bkey.as_bytes()).is_none();

        if is_new_token {
            let storage_cost = FT_ENTRY_STORAGE_BYTES as u128 * STORAGE_COST_PER_BYTE_YOCTO;
            assert!(
                wallet.storage_deposit >= storage_cost,
                "ERR_INSUFFICIENT_STORAGE: need {} yocto for new token, have {} deposited",
                storage_cost, wallet.storage_deposit
            );
            wallet.storage_deposit -= storage_cost;
            wallet.ft_token_count += 1;
        }

        // Credit the balance
        let current: u128 = if is_new_token {
            0
        } else {
            let bytes = env::storage_read(bkey.as_bytes()).unwrap();
            let arr: [u8; 16] = bytes.try_into().unwrap_or([0u8; 16]);
            u128::from_le_bytes(arr)
        };

        let new_balance = current + amount.0;
        env::storage_write(&bkey.as_bytes(), &new_balance.to_le_bytes());

        // Update wallet (storage deposit may have changed)
        self.wallet_insert(&wallet_name, &wallet);

        self.emit_event("ft_received", serde_json::json!({
            "wallet": wallet_name,
            "token": token_account.to_string(),
            "amount": amount.0.to_string(),
            "sender": sender_id.to_string(),
            "new_balance": new_balance.to_string(),
            "is_new_token": is_new_token,
        }));

        log!(
            "FT received: {} tokens from {} for wallet '{}' (balance: {})",
            amount.0, sender_id, wallet_name, new_balance
        );

        PromiseOrValue::Value(U128(0))
    }

    // ── FT Balance Views ───────────────────────────────────────────────

    /// Get FT balance for a specific token in a wallet.
    pub fn get_ft_balance(&self, wallet_name: String, token: AccountId) -> U128 {
        assert!(self.wallet_get_readonly(&wallet_name).is_some(), "ERR_WALLET_NOT_FOUND");
        let bkey = ft_balance_key(&wallet_name, token.as_str());
        let balance: u128 = env::storage_read(bkey.as_bytes())
            .map(|bytes| {
                let arr: [u8; 16] = bytes.try_into().unwrap_or([0u8; 16]);
                u128::from_le_bytes(arr)
            })
            .unwrap_or(0);
        U128(balance)
    }

    /// Get NEAR balance held by the contract for a specific wallet.
    pub fn get_wallet_near_balance(&self, wallet_name: String) -> U128 {
        assert!(self.wallet_get_readonly(&wallet_name).is_some(), "ERR_WALLET_NOT_FOUND");
        let bkey = format!("{}:near", wallet_name);
        let balance: u128 = env::storage_read(bkey.as_bytes())
            .map(|bytes| {
                let arr: [u8; 16] = bytes.try_into().unwrap_or([0u8; 16]);
                u128::from_le_bytes(arr)
            })
            .unwrap_or(0);
        U128(balance)
    }
}

// ── Internal FT Operations (used by execute.rs) ────────────────────────────

impl Contract {
    /// Credit NEAR to a wallet's internal balance.
    pub(crate) fn credit_near(&mut self, wallet_name: &str, amount: u128) {
        let bkey = format!("{}:near", wallet_name);
        let current: u128 = env::storage_read(bkey.as_bytes())
            .map(|bytes| {
                let arr: [u8; 16] = bytes.try_into().unwrap_or([0u8; 16]);
                u128::from_le_bytes(arr)
            })
            .unwrap_or(0);
        let new_balance = current + amount;
        env::storage_write(bkey.as_bytes(), &new_balance.to_le_bytes());
    }

    /// Debit NEAR from a wallet's internal balance. Panics if insufficient.
    pub(crate) fn debit_near(&mut self, wallet_name: &str, amount: u128) {
        let bkey = format!("{}:near", wallet_name);
        let current: u128 = env::storage_read(bkey.as_bytes())
            .map(|bytes| {
                let arr: [u8; 16] = bytes.try_into().unwrap_or([0u8; 16]);
                u128::from_le_bytes(arr)
            })
            .unwrap_or(0);
        assert!(current >= amount, "ERR_INSUFFICIENT_NEAR: have {}, need {}", current, amount);
        let new_balance = current - amount;
        env::storage_write(bkey.as_bytes(), &new_balance.to_le_bytes());
    }

    /// Debit FT from a wallet's internal balance. Panics if insufficient.
    pub(crate) fn debit_ft(&mut self, wallet_name: &str, token: &str, amount: u128) {
        let bkey = ft_balance_key(wallet_name, token);
        let current: u128 = env::storage_read(bkey.as_bytes())
            .map(|bytes| {
                let arr: [u8; 16] = bytes.try_into().unwrap_or([0u8; 16]);
                u128::from_le_bytes(arr)
            })
            .unwrap_or(0);
        assert!(current >= amount, "ERR_INSUFFICIENT_FT: have {}, need {}", current, amount);
        let new_balance = current - amount;
        env::storage_write(bkey.as_bytes(), &new_balance.to_le_bytes());
    }

    /// Credit FT to a wallet's internal balance (for refunds).
    pub(crate) fn credit_ft(&mut self, wallet_name: &str, token: &str, amount: u128) {
        let bkey = ft_balance_key(wallet_name, token);
        let current: u128 = env::storage_read(bkey.as_bytes())
            .map(|bytes| {
                let arr: [u8; 16] = bytes.try_into().unwrap_or([0u8; 16]);
                u128::from_le_bytes(arr)
            })
            .unwrap_or(0);
        let new_balance = current + amount;
        env::storage_write(bkey.as_bytes(), &new_balance.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ft_balance_key() {
        assert_eq!(ft_balance_key("treasury", "usdt.tether-token.near"), "treasury:ft:usdt.tether-token.near");
    }

    #[test]
    fn test_is_token_allowed_empty_list() {
        let wallet = Wallet {
            name: "test".to_string(),
            owner: "owner.near".parse().unwrap(),
            proposal_index: 0, intent_index: 3,
            created_at: 0, storage_deposit: 0, storage_used: 0,
            allowed_tokens: vec![],
            ft_token_count: 0,
            call_allowed_receivers: vec![], call_max_deposit: 0,
            daily_spend_limit: 0, daily_spend_reset_at: 0, daily_spend_used: 0,
            relayer_fee: 0, allowed_relayers: vec![],
        };
        // Empty list = accept all
        assert!(is_token_allowed(&wallet, &"anything.near".parse().unwrap()));
    }

    #[test]
    fn test_is_token_allowed_with_list() {
        let token: AccountId = "usdt.tether-token.near".parse().unwrap();
        let wallet = Wallet {
            name: "test".to_string(),
            owner: "owner.near".parse().unwrap(),
            proposal_index: 0, intent_index: 3,
            created_at: 0, storage_deposit: 0, storage_used: 0,
            allowed_tokens: vec![token.clone()],
            ft_token_count: 0,
            call_allowed_receivers: vec![], call_max_deposit: 0,
            daily_spend_limit: 0, daily_spend_reset_at: 0, daily_spend_used: 0,
            relayer_fee: 0, allowed_relayers: vec![],
        };
        assert!(is_token_allowed(&wallet, &token));
        assert!(!is_token_allowed(&wallet, &"evil.near".parse().unwrap()));
    }
}
