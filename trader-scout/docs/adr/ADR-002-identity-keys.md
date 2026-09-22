# ADR-002: Identity keys (ChainKey, WalletKey, AssetKey, EventKey)

Status: Accepted
Date: 2026-09-22

## Context

ARCHITECTURE.md §2 mandates that network identity is part of every key: same address bytes on two
different chains are two different records; same ticker on different chains is not one asset.
AGENTS.md invariant #3 restates this as non-negotiable. ACCEPTANCE A01-A04 give concrete test cases:
ambiguous EVM chain resolution, bare EVM wallet without chain, Solana mainnet/devnet byte collision,
same symbol on two token addresses.

## Decision

### ChainKey

```rust
pub struct ChainKey {
    pub family: ChainFamily,       // Solana | Evm
    pub network_id: NetworkId,     // EVM chain_id (u64) | Solana cluster tag
    pub genesis_identity: GenesisIdentity, // hash/fingerprint, not just a string alias
}
```

`network_id` alone is not sufficient identity proof — ARCHITECTURE.md explicitly requires checking
genesis hash for Solana, not trusting a textual `mainnet` alias. `GenesisIdentity` is an opaque
verified fingerprint obtained once via preflight (P2.1) and cached; a `ChainProfile` config entry
without a verified `GenesisIdentity` cannot be used for live scanning (only for `documented`-level P0
matrix entries).

### WalletKey / AssetKey / EventKey

```rust
pub struct WalletKey { pub chain: ChainKey, pub address: AddressBytes }
pub enum AssetKey { Native(ChainKey), Token(ChainKey, AddressBytes) }
pub struct EventKey { pub chain: ChainKey, pub location: CanonicalLocation }
```

`AddressBytes` is a fixed-capacity byte buffer (20 bytes EVM, 32 bytes Solana) tagged by
`ChainFamily` — never a bare `String`. Equality and `Ord` are derived from `(family, network_id,
genesis_identity, bytes)` as a tuple; two keys with equal bytes but different `ChainKey` are never
equal. This directly satisfies A01/A03/A04.

`CanonicalLocation` encodes the chain-appropriate total order (ADR needed for full ledger ordering is
tracked in the ledger crate, not here): EVM = `(block_number, tx_index, action_path)`, Solana =
`(slot, block_tx_index, instruction_path)`. Lexicographic signature/hash ordering is explicitly
rejected per ARCHITECTURE.md §6.

### Canonicalization at input time

Text-form addresses are canonicalized once at input parsing (scout-app), before entering any
`WalletKey`/`AssetKey`:

- EVM: parse hex, validate 20 bytes, store as raw bytes; EIP-55 mixed-case is a *display* concern
  applied only when formatting output, never part of the stored key or its `Eq`/`Hash`/`Ord`.
- Solana: `bs58` decode, validate exactly 32 bytes; reject non-canonical/invalid base58 before any
  network call (satisfies A04's "Invalid address → error with line number before live fetch").

Duplicate textual forms of the same canonical identity (e.g. mixed-case vs lowercase EVM address)
collapse to one `WalletKey`/`AssetKey` with a duplicate count reported to stderr/manifest — this is
A04's "Дубликат входного адреса в другой текстовой форме → одна canonical identity".

### Ambiguous chain resolution

For a bare EVM address with no explicit chain, resolution against enabled network profiles yields:

```rust
pub enum ChainResolution {
    Resolved(ChainKey),
    Ambiguous(Vec<ChainKey>),   // -> AMBIGUOUS_CHAIN, never picks the first responder
    NotFoundOrUnobserved,       // -> not proof of absence
}
```

`--evm-scope all` bypasses resolution entirely and fans out into one `WalletKey` per enabled EVM
`ChainKey` explicitly — never a single ambiguous guess (A02).

## Consequences

- `scout-core` has zero string-keyed `HashMap<String, _>` for chain/wallet/asset identity anywhere;
  every such map is keyed by the strongly-typed structs above.
- Any code comparing raw address bytes across chains without going through `WalletKey`/`AssetKey`
  equality is a design smell and should be flagged in review.
- Genesis-identity verification (P2.1 preflight) is a hard prerequisite for enabling live scanning on
  a `ChainProfile`; the P0 capability matrix records profiles that lack verified genesis identity as
  `unknown`/`unsupported` for live use, not silently assumed.

## Alternatives considered

- Treating `network_id` alone as sufficient chain identity: rejected — ARCHITECTURE.md explicitly
  calls out that Solana cluster identity requires genesis hash + config, not just an alias, precisely
  to avoid silently mixing mainnet/devnet data (A03).
- Storing addresses as `String` with case-insensitive comparison: rejected — loses the "canonical
  bytes are the identity, display case is cosmetic" separation, and risks a Hash/Eq mismatch bug class
  (two textually different but byte-equal addresses hashing differently).
