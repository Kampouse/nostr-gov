import { providers, KeyPair, transactions } from 'near-api-js';

const TREASURY = 'benchv5.vault.kampy.testnet';
const WATCHER_ID = 'cmsig.watcher.kampy.testnet';
const DEPOSIT = '500000000000000000000000';
const RPC = 'https://rpc.testnet.fastnear.com';

const signerKeys = JSON.parse(process.argv[2]);
const privRaw = Buffer.from(signerKeys.private_key.replace('ed25519:', ''), 'base64');
const signerKeyPair = KeyPair.fromEd25519Pair(privRaw);

const provider = new providers.JsonRpcProvider(RPC);

// Get signer nonce
const accessKey = (await provider.query({
  request_type: 'view_access_key',
  finality: 'final',
  account_id: signerKeys.account,
  public_key: signerKeyPair.getPublicKey().toString(),
}));

const block = await provider.query({ finality: 'final' });

// Create watcher keypair
const watcherKeyPair = KeyPair.fromRandom('ed25519');

// Build signed tx: create account + fund + add key
const { createTransaction, signTransaction } = await import('near-api-js');
const tx = createTransaction(
  signerKeys.account,
  signerKeyPair.getPublicKey(),
  WATCHER_ID,
  accessKey.nonce + 1n,
  [
    transactions.createAccount(),
    transactions.transfer(DEPOSIT),
    transactions.addKey(
      watcherKeyPair.getPublicKey(),
      transactions.functionCallAccessKey(
        ['approve_with_event', 'cancel_vote_with_event'],
        '500000000000000000000000'
      )
    ),
  ],
  block.header.hash
);

const signedTx = signTransaction(tx, signerKeyPair, signerKeys.account, provider);

const result = await provider.sendTransaction(signedTx);
console.log(JSON.stringify(result, null, 2));

const privHex = Buffer.from(watcherKeyPair.secretKey.replace('ed25519:', ''), 'base64').toString('hex');
console.log(`\n=== WATCHER CREDENTIALS ===`);
console.log(`ACCOUNT: ${WATCHER_ID}`);
console.log(`PUBLIC_KEY: ${watcherKeyPair.publicKey.toString()}`);
console.log(`PRIVATE_KEY_HEX: ${privHex}`);
console.log(`\n  wrangler secret put NEAR_SIGNER_KEY <<< '${privHex}'`);
console.log(`  wrangler secret put TREASURY_CONTRACT_ID <<< '${TREASURY}'`);
