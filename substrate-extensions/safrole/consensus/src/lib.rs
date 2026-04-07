//! Safrole consensus engine for Partner Chains.
//!
//! Implements VRF-based block production following the JAM Gray Paper's Safrole protocol.
//! Provides both the block authoring worker (`SafroleWorker`) and the import queue verifier
//! (`SafroleVerifier`). Preserves the Partner Chains `InherentDigest` mechanism for embedding
//! mainchain hash data in block headers.

pub mod import_queue;
pub mod ticket_worker;

use futures::prelude::*;
use log::{debug, warn};
use pallet_safrole::{
	AuthorityId, SafrolePreDigest, Ticket, SAFROLE_ENGINE_ID, find_pre_digest,
};
use parity_scale_codec::Encode;
use sc_client_api::{BlockOf, backend::AuxStore};
use sc_consensus::block_import::BlockImport;
use sc_consensus::{BlockImportParams, ForkChoiceStrategy, StateAction};
use sc_consensus_slots::{
	BackoffAuthoringBlocksStrategy, InherentDataProviderExt, SimpleSlotWorkerToSlotWorker,
	SlotInfo, SlotProportion, StorageChanges,
};
use sc_telemetry::TelemetryHandle;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_consensus::{
	BlockOrigin, Environment, Error as ConsensusError, Proposer, SelectChain, SyncOracle,
};
use sp_consensus_slots::Slot;
use sp_core::bandersnatch;
use sp_core::crypto::ByteArray;
use sp_inherents::CreateInherentDataProviders;
use sp_keystore::KeystorePtr;
use sp_partner_chains_consensus_common::InherentDigest;
use sp_runtime::traits::{Block as BlockT, Header, NumberFor};
use sp_runtime::DigestItem;
use std::{marker::PhantomData, pin::Pin, sync::Arc};

const LOG_TARGET: &str = "safrole";

/// Re-export key type from the pallet.
pub const KEY_TYPE: sp_core::crypto::KeyTypeId = pallet_safrole::KEY_TYPE;

/// Convert an `app::Public` (the RuntimeAppPublic wrapper) to the raw
/// `bandersnatch::Public` needed by keystore methods.
pub fn to_raw_public(app_pub: &AuthorityId) -> bandersnatch::Public {
	bandersnatch::Public::from_raw(<[u8; 32]>::try_from(app_pub.as_ref()).expect("bandersnatch public key is 32 bytes"))
}

/// Runtime API that the Safrole consensus engine requires.
///
/// Must be implemented by the runtime (via `impl_runtime_apis!`).
pub trait SafroleApi<Block: BlockT> {
	/// Get the slot duration in milliseconds.
	fn slot_duration(&self, at: Block::Hash) -> Result<u64, sp_api::ApiError>;
	/// Get the current authority set (Bandersnatch public keys).
	fn authorities(&self, at: Block::Hash) -> Result<Vec<AuthorityId>, sp_api::ApiError>;
	/// Get the epoch tickets for the current epoch.
	/// Returns `None` if in fallback mode.
	fn epoch_tickets(&self, at: Block::Hash) -> Result<Option<Vec<Ticket>>, sp_api::ApiError>;
	/// Get whether the chain is in fallback mode.
	fn is_fallback_mode(&self, at: Block::Hash) -> Result<bool, sp_api::ApiError>;
	/// Get the epoch randomness.
	fn epoch_randomness(&self, at: Block::Hash) -> Result<[u8; 32], sp_api::ApiError>;
}

/// Aux data passed from `aux_data` to `claim_slot` and `block_import_params`.
#[derive(Debug)]
pub struct SafroleAuxData {
	pub authorities: Vec<AuthorityId>,
	pub epoch_tickets: Option<Vec<Ticket>>,
	pub epoch_randomness: [u8; 32],
	pub fallback_mode: bool,
}

/// What we're claiming: our public key and the pre-digest to embed.
#[derive(Debug)]
pub struct SafroleClaim {
	pub public: AuthorityId,
	pub pre_digest: SafrolePreDigest,
}

/// Parameters to start the Safrole worker.
pub struct StartSafroleParams<C, SC, I, PF, SO, L, CIDP, BS, N> {
	pub slot_duration: sp_consensus_slots::SlotDuration,
	pub client: Arc<C>,
	pub select_chain: SC,
	pub block_import: I,
	pub proposer_factory: PF,
	pub sync_oracle: SO,
	pub justification_sync_link: L,
	pub create_inherent_data_providers: CIDP,
	pub force_authoring: bool,
	pub backoff_authoring_blocks: Option<BS>,
	pub keystore: KeystorePtr,
	pub block_proposal_slot_portion: SlotProportion,
	pub max_block_proposal_slot_portion: Option<SlotProportion>,
	pub telemetry: Option<TelemetryHandle>,
	pub _phantom: PhantomData<N>,
}

/// Start the Safrole worker. The returned future should be run in a futures executor.
pub fn start_safrole<B, C, SC, I, PF, SO, L, CIDP, BS, Error, ID>(
	StartSafroleParams {
		slot_duration,
		client,
		select_chain,
		block_import,
		proposer_factory,
		sync_oracle,
		justification_sync_link,
		create_inherent_data_providers,
		force_authoring,
		backoff_authoring_blocks,
		keystore,
		block_proposal_slot_portion,
		max_block_proposal_slot_portion,
		telemetry,
		..
	}: StartSafroleParams<C, SC, I, PF, SO, L, CIDP, BS, NumberFor<B>>,
) -> Result<impl Future<Output = ()>, ConsensusError>
where
	B: BlockT,
	C: ProvideRuntimeApi<B> + BlockOf + AuxStore + HeaderBackend<B> + Send + Sync + 'static,
	C::Api: SafroleRuntimeApi<B> + sp_api::ApiExt<B>,
	SC: SelectChain<B>,
	I: BlockImport<B> + Send + Sync + 'static,
	PF: Environment<B, Error = Error> + Send + Sync + 'static,
	PF::Proposer: Proposer<B, Error = Error>,
	SO: SyncOracle + Send + Sync + Clone,
	L: sc_consensus::JustificationSyncLink<B>,
	CIDP: CreateInherentDataProviders<B, ()> + Send + 'static,
	CIDP::InherentDataProviders: InherentDataProviderExt + Send,
	BS: BackoffAuthoringBlocksStrategy<NumberFor<B>> + Send + Sync + 'static,
	Error: std::error::Error + Send + From<ConsensusError> + 'static,
	ID: InherentDigest + Send + Sync + 'static,
{
	let worker = SafroleWorker {
		client,
		block_import,
		env: proposer_factory,
		keystore,
		sync_oracle: sync_oracle.clone(),
		justification_sync_link,
		force_authoring,
		backoff_authoring_blocks,
		telemetry,
		block_proposal_slot_portion,
		max_block_proposal_slot_portion,
		_phantom: PhantomData::<(B, ID)>,
	};

	Ok(sc_consensus_slots::start_slot_worker(
		slot_duration,
		select_chain,
		SimpleSlotWorkerToSlotWorker(worker),
		sync_oracle,
		create_inherent_data_providers,
	))
}

struct SafroleWorker<B, C, E, I, SO, L, BS, ID> {
	client: Arc<C>,
	block_import: I,
	env: E,
	keystore: KeystorePtr,
	sync_oracle: SO,
	justification_sync_link: L,
	force_authoring: bool,
	backoff_authoring_blocks: Option<BS>,
	block_proposal_slot_portion: SlotProportion,
	max_block_proposal_slot_portion: Option<SlotProportion>,
	telemetry: Option<TelemetryHandle>,
	_phantom: PhantomData<(B, ID)>,
}

/// Re-export the runtime API trait from the pallet crate.
/// The runtime implements this via `impl_runtime_apis!`.
pub use pallet_safrole::SafroleApi as SafroleRuntimeApi;

#[async_trait::async_trait]
impl<B, C, E, I, Error, SO, L, BS, ID> sc_consensus_slots::SimpleSlotWorker<B>
	for SafroleWorker<B, C, E, I, SO, L, BS, ID>
where
	B: BlockT,
	C: ProvideRuntimeApi<B> + BlockOf + AuxStore + HeaderBackend<B> + Sync + Send,
	C::Api: SafroleRuntimeApi<B> + sp_api::ApiExt<B>,
	E: Environment<B, Error = Error> + Send + Sync,
	E::Proposer: Proposer<B, Error = Error>,
	I: BlockImport<B> + Send + Sync + 'static,
	SO: SyncOracle + Send + Clone + Sync,
	L: sc_consensus::JustificationSyncLink<B>,
	BS: BackoffAuthoringBlocksStrategy<NumberFor<B>> + Send + Sync + 'static,
	Error: std::error::Error + Send + From<ConsensusError> + 'static,
	ID: InherentDigest + Send + Sync + 'static,
{
	type BlockImport = I;
	type SyncOracle = SO;
	type JustificationSyncLink = L;
	type CreateProposer =
		Pin<Box<dyn Future<Output = Result<E::Proposer, ConsensusError>> + Send + 'static>>;
	type Proposer = E::Proposer;
	type Claim = SafroleClaim;
	type AuxData = SafroleAuxData;

	fn logging_target(&self) -> &'static str {
		LOG_TARGET
	}

	fn block_import(&mut self) -> &mut Self::BlockImport {
		&mut self.block_import
	}

	fn aux_data(
		&self,
		header: &B::Header,
		_slot: Slot,
	) -> Result<Self::AuxData, ConsensusError> {
		let api = self.client.runtime_api();
		let at = header.hash();

		let authorities = api
			.authorities(at)
			.map_err(|_| ConsensusError::InvalidAuthoritiesSet)?;

		let epoch_tickets = api
			.epoch_tickets(at)
			.unwrap_or(None);

		let fallback_mode = api
			.is_fallback_mode(at)
			.unwrap_or(true);

		let epoch_randomness = api
			.epoch_randomness(at)
			.unwrap_or([0u8; 32]);

		Ok(SafroleAuxData {
			authorities,
			epoch_tickets,
			epoch_randomness,
			fallback_mode,
		})
	}

	fn authorities_len(&self, aux: &Self::AuxData) -> Option<usize> {
		Some(aux.authorities.len())
	}

	async fn claim_slot(
		&mut self,
		_header: &B::Header,
		slot: Slot,
		aux: &Self::AuxData,
	) -> Option<Self::Claim> {
		if aux.authorities.is_empty() {
			return None;
		}

		if aux.fallback_mode || aux.epoch_tickets.is_none() {
			// Fallback mode: deterministic VRF-based assignment.
			self.claim_slot_fallback(slot, aux)
		} else {
			// Ticket mode: check if we hold the ticket for this slot.
			self.claim_slot_ticket(slot, aux)
		}
	}

	fn pre_digest_data(
		&self,
		_slot: Slot,
		claim: &Self::Claim,
	) -> Vec<DigestItem> {
		vec![DigestItem::PreRuntime(
			SAFROLE_ENGINE_ID,
			claim.pre_digest.encode(),
		)]
	}

	async fn block_import_params(
		&self,
		header: B::Header,
		header_hash: &B::Hash,
		body: Vec<B::Extrinsic>,
		storage_changes: StorageChanges<B>,
		claim: Self::Claim,
		_aux: Self::AuxData,
	) -> Result<BlockImportParams<B>, ConsensusError> {
		// Seal the block with a Bandersnatch Schnorr signature.
		let raw_public = to_raw_public(&claim.public);
		let signature = self
			.keystore
			.bandersnatch_sign(KEY_TYPE, &raw_public, header_hash.as_ref())
			.map_err(|e| ConsensusError::CannotSign(e.to_string()))?
			.ok_or_else(|| {
				ConsensusError::CannotSign("Key not found in keystore".into())
			})?;

		let seal = DigestItem::Seal(SAFROLE_ENGINE_ID, signature.encode());

		let mut import_block = BlockImportParams::new(BlockOrigin::Own, header);
		import_block.post_digests.push(seal);
		import_block.body = Some(body);
		import_block.state_action =
			StateAction::ApplyChanges(sc_consensus::StorageChanges::Changes(storage_changes));
		import_block.fork_choice = Some(ForkChoiceStrategy::LongestChain);

		Ok(import_block)
	}

	fn force_authoring(&self) -> bool {
		self.force_authoring
	}

	fn should_backoff(&self, slot: Slot, chain_head: &B::Header) -> bool {
		if let Some(ref strategy) = self.backoff_authoring_blocks {
			if let Some(pre_digest) = find_pre_digest::<B>(chain_head) {
				return strategy.should_backoff(
					*chain_head.number(),
					pre_digest.slot(),
					self.client.info().finalized_number,
					slot,
					self.logging_target(),
				);
			}
		}
		false
	}

	fn sync_oracle(&mut self) -> &mut Self::SyncOracle {
		&mut self.sync_oracle
	}

	fn justification_sync_link(&mut self) -> &mut Self::JustificationSyncLink {
		&mut self.justification_sync_link
	}

	fn proposer(&mut self, block: &B::Header) -> Self::CreateProposer {
		self.env
			.init(block)
			.map_err(|e| ConsensusError::ClientImport(format!("{:?}", e)))
			.boxed()
	}

	fn telemetry(&self) -> Option<TelemetryHandle> {
		self.telemetry.clone()
	}

	fn proposing_remaining_duration(&self, slot_info: &SlotInfo<B>) -> std::time::Duration {
		let parent_slot = find_pre_digest::<B>(&slot_info.chain_head).map(|d| d.slot());

		sc_consensus_slots::proposing_remaining_duration(
			parent_slot,
			slot_info,
			&self.block_proposal_slot_portion,
			self.max_block_proposal_slot_portion.as_ref(),
			sc_consensus_slots::SlotLenienceType::Exponential,
			self.logging_target(),
		)
	}
}

// ── Slot claiming logic ──────────────────────────────────────────────────

impl<B, C, E, I, SO, L, BS, ID> SafroleWorker<B, C, E, I, SO, L, BS, ID>
where
	B: BlockT,
	C: AuxStore,
{
	/// Claim a slot in fallback mode (no tickets).
	/// Uses deterministic VRF-based assignment: hash(randomness || slot) selects an authority.
	fn claim_slot_fallback(
		&self,
		slot: Slot,
		aux: &SafroleAuxData,
	) -> Option<SafroleClaim> {
		let randomness = &aux.epoch_randomness;
		let mut input = [0u8; 40];
		input[..32].copy_from_slice(randomness);
		input[32..40].copy_from_slice(&(*slot).to_le_bytes());
		let hash = sp_core::hashing::blake2_256(&input);
		let idx =
			u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]]) as usize;
		let expected_idx = idx % aux.authorities.len();
		let expected_authority = &aux.authorities[expected_idx];

		// Check if we hold this authority's key in our keystore.
		let raw_expected = to_raw_public(expected_authority);
		if self
			.keystore
			.has_keys(&[(raw_expected.to_raw_vec(), KEY_TYPE)])
		{
			// Create VRF signature for this slot.
			let vrf_input_data = [
				b"jam_fallback_seal".as_slice(),
				randomness,
				&(*slot).to_le_bytes(),
			]
			.concat();

			let vrf_sign_data =
				bandersnatch::vrf::VrfSignData::new(&vrf_input_data, b"");

			let vrf_signature = self
				.keystore
				.bandersnatch_vrf_sign(KEY_TYPE, &raw_expected, &vrf_sign_data)
				.ok()
				.flatten()?;

			Some(SafroleClaim {
				public: expected_authority.clone(),
				pre_digest: SafrolePreDigest::Fallback {
					slot,
					authority_index: expected_idx as u32,
					vrf_signature,
				},
			})
		} else {
			None
		}
	}

	/// Claim a slot in ticket mode.
	/// Uses aux-DB for O(1) lookup of ticket ownership, falling back to
	/// brute-force if no mapping is found.
	fn claim_slot_ticket(
		&self,
		slot: Slot,
		aux: &SafroleAuxData,
	) -> Option<SafroleClaim> {
		let tickets = aux.epoch_tickets.as_ref()?;

		let epoch_length = tickets.len();
		if epoch_length == 0 {
			return None;
		}
		let slot_in_epoch = (*slot as usize) % epoch_length;
		let ticket = tickets.get(slot_in_epoch)?;

		// O(1) lookup: check aux-DB for ticket→authority mapping.
		// Try fast path: aux-DB lookup populated by ticket worker.
		// Epoch index = slot / epoch_length (same formula as pallet_safrole::set_slot).
		let epoch_index = *slot / epoch_length as u64;
		if let Some(auth_idx) = ticket_worker::lookup_ticket_owner(
			self.client.as_ref(),
			epoch_index,
			&ticket.id,
		) {
			if let Some(authority) = aux.authorities.get(auth_idx as usize) {
				let raw_auth = to_raw_public(authority);
				if self.keystore.has_keys(&[(raw_auth.to_raw_vec(), KEY_TYPE)]) {
					// Create seal VRF signature.
					let vrf_input_data = [
						b"jam_ticket_seal".as_slice(),
						&aux.epoch_randomness,
						&(*slot).to_le_bytes(),
					]
					.concat();
					let vrf_sign_data =
						bandersnatch::vrf::VrfSignData::new(&vrf_input_data, b"");

					if let Ok(Some(vrf_signature)) =
						self.keystore.bandersnatch_vrf_sign(KEY_TYPE, &raw_auth, &vrf_sign_data)
					{
						debug!(
							target: LOG_TARGET,
							"Claimed slot {} via ticket (aux-DB fast path, authority {})",
							*slot, auth_idx,
						);
						return Some(SafroleClaim {
							public: authority.clone(),
							pre_digest: SafrolePreDigest::Ticket {
								slot,
								ticket_index: slot_in_epoch as u32,
								vrf_signature,
							},
						});
					}
				}
			}
		}

		// Slow path: brute-force over local keys (fallback for tickets
		// generated before aux-DB was populated, e.g. after node restart).
		for authority in &aux.authorities {
			let raw_auth = to_raw_public(authority);
			if !self.keystore.has_keys(&[(raw_auth.to_raw_vec(), KEY_TYPE)]) {
				continue;
			}

			let vrf_input_data = [
				b"jam_ticket_seal".as_slice(),
				&aux.epoch_randomness,
				&(*slot).to_le_bytes(),
			]
			.concat();
			let vrf_sign_data = bandersnatch::vrf::VrfSignData::new(&vrf_input_data, b"");
			let vrf_signature = match self
				.keystore
				.bandersnatch_vrf_sign(KEY_TYPE, &raw_auth, &vrf_sign_data)
			{
				Ok(Some(sig)) => sig,
				_ => continue,
			};

			for attempt in 0..=ticket.attempt {
				let ticket_input_data = [
					b"jam_ticket".as_slice(),
					&aux.epoch_randomness,
					&[attempt],
				]
				.concat();
				let ticket_vrf_data =
					bandersnatch::vrf::VrfSignData::new(&ticket_input_data, b"");

				if let Ok(Some(ticket_sig)) = self
					.keystore
					.bandersnatch_vrf_sign(KEY_TYPE, &raw_auth, &ticket_vrf_data)
				{
					let output_bytes = ticket_sig.pre_output.make_bytes();
					if output_bytes == ticket.id {
						debug!(
							target: LOG_TARGET,
							"Claimed slot {} via ticket (brute-force fallback, attempt {})",
							*slot, attempt,
						);
						return Some(SafroleClaim {
							public: authority.clone(),
							pre_digest: SafrolePreDigest::Ticket {
								slot,
								ticket_index: slot_in_epoch as u32,
								vrf_signature,
							},
						});
					}
				}
			}
		}

		warn!(
			target: LOG_TARGET,
			"Slot {} has ticket but no local key matches",
			*slot,
		);
		None
	}
}

/// Verify a Bandersnatch Schnorr seal on a block header.
pub fn verify_seal(
	header_hash: &[u8],
	seal_bytes: &[u8],
	authority: &AuthorityId,
) -> bool {
	let Ok(signature) = bandersnatch::Signature::from_slice(seal_bytes) else {
		return false;
	};
	let raw = to_raw_public(authority);
	<bandersnatch::Pair as sp_core::crypto::Pair>::verify(&signature, header_hash, &raw)
}
