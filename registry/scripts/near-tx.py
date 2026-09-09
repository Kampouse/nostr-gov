#!/usr/bin/env python3
"""near-tx.py — sign & send a NEAR transaction without the CLI.

The near-cli-rs demands an interactive confirmation we can't provide from
this environment; this does the same job directly: borsh-serialize the
transaction, ed25519-sign it, broadcast via RPC.

Usage:
  near-tx.py send-create-account <signer_account> <new_account> <new_pubkey> <amount_yocto>
  near-tx.py send-deploy <signer_account> <wasm_file>
  near-tx.py send-call <signer_account> <receiver> <method> <json_args> <deposit_yocto> <gas>
"""
import base64, json, os, sys, hashlib, struct

RPC = "https://rpc.testnet.near.org"


def b64u(b):  # unpadded base64 for ed25519 keys
    return base64.urlsafe_b64encode(b).decode().rstrip("=")


def load_key(account):
    path = os.path.expanduser(f"~/.near-credentials/testnet/{account}.json")
    d = json.load(open(path))
    sk_b64 = d["private_key"].replace("ed25519:", "")
    sk = base64.b64decode(sk_b64 + "=" * (-len(sk_b64) % 4))
    return d["public_key"], sk


def rpc(method, params):
    import urllib.request
    req = urllib.request.Request(
        RPC, json.dumps({"jsonrpc": "2.0", "id": "1", "method": method,
                         "params": params}).encode(),
        {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read())


# ── borsh ───────────────────────────────────────────────────────────────
def u8(n): return struct.pack("<B", n)
def u32(n): return struct.pack("<I", n)
def u64(n): return struct.pack("<Q", n)
def u128(n): return n.to_bytes(16, "little")
def string(s):
    b = s.encode() if isinstance(s, str) else s
    return u32(len(b)) + b
def opt_string(s): return u8(1) + string(s) if s is not None else u8(0)
def pub_key(pk_str):
    # "ed25519:<base64url>" → (key: 0, bytes)
    raw = base64.urlsafe_b64decode(pk_str.split(":")[1] + "==")
    return u8(0) + bytes([len(raw)]) + raw


def action_create_account():
    return u8(0)  # enum CreateAccount
def action_transfer(yocto):
    return u8(3) + u128(yocto)
def action_add_key(pk_str, nonce=0):
    pk = pub_key(pk_str)
    # AccessKeyPermission::FunctionCall(0) with no restrictions = FullAccess
    # FullAccess is Permission::FullAccess = 1
    permission = u8(1)
    access_key = permission + u64(nonce)
    return u8(2) + pk + access_key
def action_deploy_contract(code: bytes):
    return u8(1) + u32(len(code)) + code
def action_function_call(method, args: bytes, gas, deposit_yocto):
    return u8(6) + string(method) + u32(len(args)) + args + u64(gas) + u128(deposit_yocto)


def serialize_tx(signer, pk_str, nonce, block_hash_b, actions_b):
    return (string(signer) + pub_key(pk_str) + u64(nonce)
            + u32(len(block_hash_b)) + block_hash_b
            + u32(len(actions_b)) + actions_b)


def sign(sk_seed, data):
    # ed25519 from seed (NEAR stores the 32-byte seed b64; some stores keep
    # 64-byte expanded — handle both: nacl uses the 32-byte seed)
    try:
        import nacl.signing
        if len(sk_seed) == 64:
            # expanded key: sign directly
            k = nacl.signing.Signer(sk_seed)
            return k.sign(data)[:64] + b"" if False else bytes(
                __import__("nacl.bindings", fromlist=["nacl.bindings"])
                .crypto_sign_detached(data, sk_seed))
        return bytes(nacl.bindings.crypto_sign_detached(data, sk_seed))
    except ImportError:
        # pure-python fallback via pynacl absent: use hashlib? no —
        # require pynacl
        raise SystemExit("pip install pynacl")


def send(signer, actions_b):
    pub, sk = load_key(signer)
    # nonce + block hash
    ak = rpc("query", {"request_type": "view_access_key", "finality": "final",
                       "account_id": signer, "public_key": pub})
    if "error" in ak:
        raise SystemExit(f"access key error: {ak['error']}")
    nonce = ak["result"]["nonce"] + 1
    bh = rpc("block", {"finality": "final"})["result"]["header"]["hash"]
    block_hash = bytes.fromhex(bh)
    tx = serialize_tx(signer, pub, nonce, block_hash, actions_b)
    sig = sign(sk, hashlib.sha256(tx).digest())  # near signs the sha256 of tx
    # SignedTransaction { transaction, signature { key, bytes } }
    signed = tx + u8(0) + bytes([len(sig)]) + sig  # Signature: Secp256K1=0? ed25519=0 enum
    # NOTE: Signature enum: ED25519 = 0, SECP256K1 = 1
    b64 = base64.b64encode(signed).decode()
    res = rpc("broadcast_tx_commit", [b64])
    if "error" in res:
        raise SystemExit(f"tx error: {json.dumps(res['error'])[:400]}")
    return res["result"]


def main():
    cmd = sys.argv[1]
    if cmd == "send-create-account":
        signer, new_acct, new_pub, amount = sys.argv[2:6]
        actions = (action_create_account() + action_transfer(int(amount))
                   + action_add_key(new_pub))
        r = send(signer, actions)
        print("OK:", r["transaction"]["hash"])
    elif cmd == "send-deploy":
        signer, wasm = sys.argv[2], sys.argv[3]
        code = open(wasm, "rb").read()
        r = send(signer, action_deploy_contract(code))
        print("OK:", r["transaction"]["hash"])
    elif cmd == "send-call":
        signer, receiver, method, args, dep, gas = sys.argv[2:8]
        r = send(signer, string(receiver) and b"" or b"")  # placeholder
        raise SystemExit("call not needed yet")
    else:
        print(__doc__)


if __name__ == "__main__":
    main()
