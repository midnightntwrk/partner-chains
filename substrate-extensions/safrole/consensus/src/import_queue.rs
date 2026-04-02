//! Import queue verifier for Safrole blocks.
//!
//! Verifies Bandersnatch seals, VRF proofs in pre-digests, and delegates
//! inherent checking to the runtime. Preserves the `InherentDigest` mechanism
//! for Partner Chains mainchain hash verification.

use crate::{LOG_TARGET, to_raw_public, verify_seal};
use pallet_safrole::SafroleApi as SafroleRuntimeApi;
use log::trace;
use pallet_safrole::{SafrolePreDigest, SAFROLE_ENGINE_ID, find_pre_digest};
use sc_client_api::{BlockOf, UsageProvider, backend::AuxStore};
use sc_consensus::{
	block_import::{BlockImport, BlockImportParams, ForkChoiceStrategy},
	import_queue::{BasicQueue, DefaultImportQueue, Verifier},
};
use sc_telemetry::{CONSENSUS_TRACE, TelemetryHandle, telemetry};
use sp_api::{ApiExt, ProvideRuntimeApi};
use sp_block_builder::BlockBuilder as BlockBuilderApi;
use sp_blockchain::{HeaderBackend, HeaderMetadata};
use sp_consensus::Error as ConsensusError;
use sp_consensus_slots::Slot;
use sp_core::crypto::VrfPublic as _;
use sp_inherents::{CreateInherentDataProviders, InherentDataProvider};
use sp_partner_chains_consensus_common::{CurrentSlotProvider, InherentDigest};
use sp_runtime::{
	DigestItem,
	traits::{Block as BlockT, Header},
};
use std::{marker::PhantomData, sync::Arc};

/// Verifier for Safrole blocks.
pub struct SafroleVerifier<C, CIDP, B: BlockT, ID> {
	client: Arc<C>,
	create_inherent_data_providers: CIDP,
	telemetry: Option<TelemetryHandle>,
	_phantom: PhantomData<(B, ID)>,
}

impl<C, CIDP, B: BlockT, ID> SafroleVerifier<C, CIDP, B, ID> {
	pub fn new(
		client: Arc<C>,
		create_inherent_data_providers: CIDP,
		telemetry: Option<TelemetryHandle>,
	) -> Self {
		Self {
			client,
			create_inherent_data_providers,
			telemetry,
			_phantom: PhantomData,
		}
	}
}

/// Extract and strip the seal from a block header.
/// Returns the pre-digest, the seal digest item, and the modified header.
fn extract_seal_and_predigest<B: BlockT>(
	header: &mut B::Header,
) -> Result<(SafrolePreDigest, DigestItem), String> {
	// The seal is the last digest log.
	let seal = header
		.digest_mut()
		.pop()
		.ok_or_else(|| "Header has no digest items (missing seal)".to_string())?;

	match seal {
		DigestItem::Seal(id, _) if id == SAFROLE_ENGINE_ID => {},
		_ => return Err("Last digest item is not a Safrole seal".to_string()),
	}

	let pre_digest = find_pre_digest::<B>(header)
		.ok_or_else(|| "Missing Safrole pre-digest in header".to_string())?;

	Ok((pre_digest, seal))
}

#[async_trait::async_trait]
impl<B, C, CIDP, ID> Verifier<B> for SafroleVerifier<C, CIDP, B, ID>
where
	B: BlockT,
	C: HeaderBackend<B>
		+ HeaderMetadata<B, Error = sp_blockchain::Error>
		+ ProvideRuntimeApi<B>
		+ Send
		+ Sync
		+ AuxStore,
	C::Api: BlockBuilderApi<B> + SafroleRuntimeApi<B>,
	CIDP: CurrentSlotProvider
		+ CreateInherentDataProviders<B, (Slot, <ID as InherentDigest>::Value)>
		+ Send
		+ Sync,
	ID: InherentDigest + Send + Sync + 'static,
{
	async fn verify(
		&self,
		mut block: BlockImportParams<B>,
	) -> Result<BlockImportParams<B>, String> {
		// Skip checks for state imports (gap sync, warp sync).
		if block.with_state() || block.state_action.skip_execution_checks() {
			block.fork_choice = Some(ForkChoiceStrategy::Custom(block.with_state()));
			return Ok(block);
		}

		let hash = block.header.hash();
		let parent_hash = *block.header.parent_hash();

		// Fetch authorities from runtime.
		let authorities = self
			.client
			.runtime_api()
			.authorities(parent_hash)
			.map_err(|e| format!("Could not fetch authorities at {:?}: {}", parent_hash, e))?;

		// Extract inherent digest value before modifying the header.
		let inherent_digest_value = <ID as InherentDigest>::value_from_digest(
			block.header.digest().logs(),
		)
		.map_err(|e| {
			format!(
				"Failed to retrieve inherent digest from header at {:?}: {}",
				parent_hash, e
			)
		})?;

		// Extract seal and pre-digest.
		let (pre_digest, seal) = extract_seal_and_predigest::<B>(&mut block.header)?;

		let slot_now = self.create_inherent_data_providers.slot();
		let block_slot = pre_digest.slot();

		// Reject blocks from the future (allow 1 slot drift).
		if block_slot > slot_now + 1 {
			return Err(format!(
				"Block slot {} is too far in the future (current: {})",
				*block_slot, *slot_now
			));
		}

		// Header hash without the seal — this is what was signed.
		let pre_seal_hash = block.header.hash();

		match &pre_digest {
			SafrolePreDigest::Fallback {
				authority_index,
				vrf_signature,
				..
			} => {
				let idx = *authority_index as usize;
				let authority = authorities.get(idx).ok_or_else(|| {
					format!("Authority index {} out of bounds (len {})", idx, authorities.len())
				})?;

				// Verify the seal (Bandersnatch Schnorr signature on the header hash).
				if let DigestItem::Seal(_, ref seal_bytes) = seal {
					if !verify_seal(pre_seal_hash.as_ref(), seal_bytes, authority) {
						return Err(format!("Bad Safrole seal on block {:?}", hash));
					}
				}

				// Verify the VRF proof matches this authority for this slot.
				let epoch_randomness = self
					.client
					.runtime_api()
					.epoch_randomness(parent_hash)
					.unwrap_or([0u8; 32]);

				let vrf_input_data = [
					b"jam_fallback_seal".as_slice(),
					&epoch_randomness,
					&(*block_slot).to_le_bytes(),
				]
				.concat();

				let vrf_sign_data =
					sp_core::bandersnatch::vrf::VrfSignData::new(&vrf_input_data, b"");

				if !to_raw_public(authority).vrf_verify(&vrf_sign_data, vrf_signature) {
					return Err(format!(
						"Bad VRF proof in fallback pre-digest for block {:?}",
						hash
					));
				}

				// Verify deterministic fallback assignment.
				let mut fallback_input = [0u8; 40];
				fallback_input[..32].copy_from_slice(&epoch_randomness);
				fallback_input[32..40].copy_from_slice(&(*block_slot).to_le_bytes());
				let fallback_hash = sp_core::hashing::blake2_256(&fallback_input);
				let expected_idx = u32::from_le_bytes([
					fallback_hash[0],
					fallback_hash[1],
					fallback_hash[2],
					fallback_hash[3],
				]) as usize
					% authorities.len();

				if expected_idx != idx {
					return Err(format!(
						"Fallback authority index mismatch: expected {}, got {}",
						expected_idx, idx
					));
				}
			},
			SafrolePreDigest::Ticket {
				ticket_index,
				..
			} => {
				// Verify ticket mode: the ticket at this index must exist.
				let tickets = self
					.client
					.runtime_api()
					.epoch_tickets(parent_hash)
					.map_err(|e| format!("Could not fetch epoch tickets: {}", e))?
					.ok_or_else(|| {
						"Block claims ticket mode but chain is in fallback mode".to_string()
					})?;

				let _ticket = tickets.get(*ticket_index as usize).ok_or_else(|| {
					format!(
						"Ticket index {} out of bounds (len {})",
						ticket_index,
						tickets.len()
					)
				})?;

				// The seal reveals the author's identity. Verify the seal is valid
				// for *some* authority in the set.
				if let DigestItem::Seal(_, ref seal_bytes) = seal {
					let seal_valid = authorities
						.iter()
						.any(|a| verify_seal(pre_seal_hash.as_ref(), seal_bytes, a));
					if !seal_valid {
						return Err(format!(
							"Bad Safrole seal: no authority matches for block {:?}",
							hash
						));
					}
				}

				// TODO: Verify that the sealing authority actually owns the ticket
				// by checking VRF(authority, epoch_randomness || attempt) == ticket.id.
			},
		}

		// Verify inherents if body is present.
		if let Some(inner_body) = block.body.take() {
			let new_block = B::new(block.header.clone(), inner_body.clone());

			let inherent_data_providers = self
				.create_inherent_data_providers
				.create_inherent_data_providers(
					parent_hash,
					(block_slot, inherent_digest_value),
				)
				.await
				.map_err(|e| format!("Error creating inherent data providers: {}", e))?;

			// Check inherents if the runtime supports it.
			if self
				.client
				.runtime_api()
				.has_api_with::<dyn BlockBuilderApi<B>, _>(parent_hash, |v| v >= 2)
				.map_err(|e| e.to_string())?
			{
				let inherent_data = inherent_data_providers
					.create_inherent_data()
					.await
					.map_err(|e| format!("Error creating inherent data: {:?}", e))?;

				sp_block_builder::check_inherents_with_data(
					self.client.clone(),
					parent_hash,
					new_block.clone(),
					&inherent_data_providers,
					inherent_data,
				)
				.await
				.map_err(|e| format!("Error checking block inherents: {:?}", e))?;
			}

			let (_, inner_body) = new_block.deconstruct();
			block.body = Some(inner_body);
		}

		trace!(target: LOG_TARGET, "Verified {:?}; importing.", block.header);
		telemetry!(
			self.telemetry;
			CONSENSUS_TRACE;
			"safrole.checked_and_importing";
			"pre_header" => ?block.header,
		);

		block.post_digests.push(seal);
		block.fork_choice = Some(ForkChoiceStrategy::LongestChain);
		block.post_hash = Some(hash);

		Ok(block)
	}
}

/// Parameters for creating the Safrole import queue.
pub struct SafroleImportQueueParams<'a, Block: BlockT, I, C, S, CIDP> {
	pub block_import: I,
	pub justification_import: Option<
		sc_consensus::BoxJustificationImport<Block>,
	>,
	pub client: Arc<C>,
	pub create_inherent_data_providers: CIDP,
	pub spawner: &'a S,
	pub registry: Option<&'a substrate_prometheus_endpoint::Registry>,
	pub telemetry: Option<TelemetryHandle>,
}

/// Create a Safrole import queue.
pub fn import_queue<Block, I, C, S, CIDP, ID>(
	SafroleImportQueueParams {
		block_import,
		justification_import,
		client,
		create_inherent_data_providers,
		spawner,
		registry,
		telemetry,
	}: SafroleImportQueueParams<'_, Block, I, C, S, CIDP>,
) -> Result<DefaultImportQueue<Block>, ConsensusError>
where
	Block: BlockT,
	C::Api: BlockBuilderApi<Block> + SafroleRuntimeApi<Block>,
	C: 'static
		+ ProvideRuntimeApi<Block>
		+ BlockOf
		+ Send
		+ Sync
		+ AuxStore
		+ UsageProvider<Block>
		+ HeaderBackend<Block>
		+ HeaderMetadata<Block, Error = sp_blockchain::Error>,
	I: BlockImport<Block, Error = ConsensusError> + Send + Sync + 'static,
	S: sp_core::traits::SpawnEssentialNamed,
	CIDP: CurrentSlotProvider
		+ CreateInherentDataProviders<Block, (Slot, <ID as InherentDigest>::Value)>
		+ Sync
		+ Send
		+ 'static,
	ID: InherentDigest + Send + Sync + 'static,
{
	let verifier = SafroleVerifier::<_, _, _, ID>::new(
		client,
		create_inherent_data_providers,
		telemetry,
	);

	Ok(BasicQueue::new(
		verifier,
		Box::new(block_import),
		justification_import,
		spawner,
		registry,
	))
}
