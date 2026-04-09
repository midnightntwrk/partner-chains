# Aura to Safrole Migration

## Goal

Replace Aura (round-robin block production) with Safrole (VRF ticket-based, JAM Gray Paper).
GRANDPA finality stays. Fresh chain. Full spec including ring-VRF anonymity.

## Key Design Decisions

- **D1**: New `pallet-safrole` replaces `pallet-aura` (different state shape)
- **D2**: Use `sp_core::bandersnatch` types directly (already in SDK behind `bandersnatch-experimental`)
- **D3**: Tickets submitted as unsigned extrinsics with `ValidateUnsigned`
- **D4**: `InherentDigest` mechanism is consensus-agnostic, extracted to `sp-partner-chains-consensus-common`
- **D5**: Fallback mode from day one (chain liveness on first epoch)
- **D6**: Hard fork, no dual Aura/Safrole
- **D7**: Stake weighting via two mechanisms: (1) Ariadne's existing proportional seat allocation for block production, (2) `pallet-grandpa-weights` injects stake-proportional GRANDPA authority weights for finality voting. No polkadot-sdk fork — patches `PendingChange` storage before GRANDPA's `on_finalize`.

## Completed

### Phase 1: pallet-safrole

`substrate-extensions/safrole/pallet/`

- State: `CurrentSlot`, `Authorities`, `NextAuthorities`, `TicketAccumulator`, `EpochTickets`, `EpochRandomness`, `InFallbackMode`
- Epoch rotation with outside-in ticket reordering (Gray Paper section 6)
- `OnTimestampSet` impl (drop-in for pallet-aura)
- `SafrolePreDigest` enum (Ticket + Fallback variants)
- `SafroleApi` runtime API via `decl_runtime_apis!`
- Genesis config

### Phase 2: Consensus-common primitives

`substrate-extensions/consensus-common/primitives/`

- Extracted `InherentDigest`, `CurrentSlotProvider`, `PartnerChainsProposerFactory` from aura crates
- Aura primitives re-export from common for backward compat

### Phase 3: Safrole consensus engine

`substrate-extensions/safrole/consensus/`

- `SafroleWorker` implementing `SimpleSlotWorker`:
  - `claim_slot_fallback()`: deterministic blake2(randomness || slot) assignment
  - `claim_slot_ticket()`: brute-force key matching against anonymous tickets
  - Bandersnatch Schnorr seal on headers
- `SafroleVerifier` import queue: validates seals, VRF proofs, preserves InherentDigest
- `import_queue()` / `start_safrole()` factory functions

### Phase 4: Wire into demo runtime

- `pallet_safrole` in runtime + mock, `OnTimestampSet = Safrole`
- All `pallet_aura::CurrentSlot` reads replaced with `pallet_safrole::CurrentSlot`
- `SafroleApi` implemented in `impl_runtime_apis!`
- SafroleConfig in all genesis configs
- All 9 runtime tests pass

## In Progress

### Phase 5: Remove Aura, wire Safrole end-to-end

#### 5a: SessionKeys change

**Problem**: `impl_opaque_keys!` requires each key type to implement `RuntimeAppPublic`.
Bandersnatch in `sp-core` has `Pair`/`Public`/`Signature` but no `app_crypto!` wrapper yet.

**Options**:

1. Create a `safrole_app` module using `app_crypto!(bandersnatch, SAFR)` -- but `app_crypto!` may not support bandersnatch yet since it's experimental
2. Use a custom `KeyTypeId` and manually implement the needed traits
3. Check if polkadot-stable2603 has `sp_application_crypto` support for bandersnatch

**Resolution**: Created `app_crypto!(bandersnatch, KEY_TYPE)` in `pallet_safrole::app` module. Works.
Also implemented `BoundToRuntimeAppPublic` and `OneSessionHandler` on `Pallet<T>`.
SessionKeys changed to `{ safrole: Safrole, grandpa: Grandpa }` in runtime.
`pallet_aura` removed from construct_runtime, Config, AuraApi.

**Done**:
- SessionKeys: `{ safrole: Safrole, grandpa: Grandpa }` using `app_crypto!(bandersnatch, KEY_TYPE)`
- `pallet_aura` removed from `construct_runtime!`, Config impl removed, `AuraApi` removed
- `pallet_safrole` implements `BoundToRuntimeAppPublic` + `OneSessionHandler`
- Node service switched from Aura to Safrole consensus engine
- Import queue uses `SafroleVerifier` instead of `AuraVerifier`
- RPC uses `SafroleApi` bound instead of `AuraApi`
- `inherent_data.rs` fully cleaned of Aura imports, uses `SlotIDP`
- All chain specs updated (staging, testnet, template, presets)
- All test mocks updated (runtime mock, node runtime_api_mock, chain_spec test)
- Full workspace compiles, 9 runtime + 3 node + 82 toolkit lib tests pass
- **Toolkit rename done**: `AuraPublicKey` → `SafrolePublicKey`, `aura_pub_key` → `safrole_pub_key`
- `CandidateKeys` encoding simplified (no legacy Aura+Grandpa special case)
- CLI keystore: AURA key definition aliased to SAFROLE (key type "safr", scheme "bandersnatch")
- CLI mock runtime: `TestSessionKeys { safrole, grandpa }`
- 6 db-sync candidate test failures remain: test fixtures contain Plutus datum hex with old "aura" key type bytes — need fixture regeneration for fresh-chain format

#### 5b: Node service switch

- `service.rs`: Replace `start_aura` with `start_safrole`
- `inherent_data.rs`: Replace `AuraIDP` with generic slot-from-timestamp
- Remove `sc-consensus-aura`, `sp-consensus-aura`, `sc-partner-chains-consensus-aura` deps from demo node

#### 5c: Remove pallet-aura

- Remove from `construct_runtime!`
- Remove `pallet_aura::Config` impl
- Remove `AuraApi` from `impl_runtime_apis!`
- Remove `AuraConfig` from all genesis configs
- Update `slot_config()` to use SafroleApi instead of AuraApi

#### 5d: Update RPC

- `rpc.rs`: Remove `AuraApi` bound on client

#### 5e: Update tests

- `header_tests.rs`: Currently tests Aura sr25519 seal verification -- needs rewrite for Bandersnatch
- `mock.rs`: Remove Aura from construct_runtime, update TestSessionKeys
- `tests/chain_spec.rs`: Update authority_keys construction
- `tests/runtime_api_mock.rs`: Update SessionKeys construction

## Completed (continued)

### Phase 6: Stake-weighted GRANDPA finality

Original plan was seat expansion (duplicating validators proportional to stake).
Replaced with a cleaner approach: direct GRANDPA weight injection.

- `pallet-grandpa-weights` (`substrate-extensions/grandpa-weights/`) stores GrandpaId → stake weight
- Patches `pallet_grandpa::PendingChange` in `on_finalize` before GRANDPA emits `ScheduledChange` digest
- `GrandpaApi::grandpa_authorities()` returns weighted authorities via `weighted_grandpa_authorities()`
- `select_authorities_with_weights` extracts stake data from candidate registrations
- No polkadot-sdk fork needed; underlying `finality-grandpa` library already supports weighted voting
- Block production proportionality handled separately by ariadne_v2's existing seat allocation

### Phase 7: Ticket submission

`substrate-extensions/safrole/pallet/src/lib.rs` + `substrate-extensions/safrole/consensus/src/ticket_worker.rs`

- `submit_ticket` unsigned extrinsic with `DispatchClass::Operational` priority
- Ring-VRF proof verification on-chain via `RingContext::verifier_no_context`
- `ValidateUnsigned`: cheap pool checks (duplicate, accumulator full, valid attempt); full ring-VRF verify in dispatch
- `set_ring_verifier_key` inherent — block producer supplies verifier key at epoch boundary
- `ProvideInherent` for ring verifier key with `RingVerifierKeyProvider` inherent data provider
- Genesis computes and stores initial `RingVerifierKey` from `new_testing()` SRS
- Runtime API extensions: `current_epoch()`, `ring_verifier_key()`
- Background `run_ticket_worker` task generates tickets per epoch for local authority keys
- Ticket worker wired end-to-end: `TicketExtrinsicBuilder` callback from service.rs constructs unsigned extrinsics, pool submission via `TransactionPool::submit_one`
- Spawned as essential blocking task in `service.rs`

### Phase 8: Aux-DB ticket mapping

`substrate-extensions/safrole/consensus/src/ticket_worker.rs` + `lib.rs`

- Aux-DB key: `safrole_ticket::{epoch_le_bytes}::{ticket_id_bytes}` → `authority_index (u32 LE)`
- `store_ticket_owner()` called in ticket worker when generating tickets
- `lookup_ticket_owner()` used in `claim_slot_ticket()` for O(1) fast path
- Brute-force fallback preserved for tickets from before aux-DB was populated (e.g. node restart)
- `AuxStore` bound added to `SafroleWorker` slot claiming impl block

## Open Questions (Resolved)

- ~~Does `app_crypto!` support bandersnatch on polkadot-stable2603?~~ **Yes.** `app_crypto!(bandersnatch, KEY_TYPE)` works.
- Ring context SRS data: where does the zcash-srs-2-11-uncompressed.bin live? Gooseberry includes it but it's large. Embed (try compressing but might be uncompressible if random). **Currently using `new_testing()` deterministic SRS. Production SRS is a future concern.**
- Epoch length: currently hardcoded as `ConstU32<60>`. Should this be configurable via chain spec or derived from Cardano epoch config?
