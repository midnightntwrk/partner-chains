//! Background task that generates ring-VRF tickets and submits them as unsigned extrinsics.
//!
//! Spawned as an essential task in the node service. Polls for epoch changes and
//! generates tickets once per epoch for each local authority key.

use pallet_safrole::{
	EpochIndex, Ticket, TicketEnvelope, KEY_TYPE, MAX_RING_SIZE,
	SafroleApi as SafroleRuntimeApi,
};
use parity_scale_codec::{Decode, Encode};
use sc_client_api::AuxStore;
use sc_transaction_pool_api::{TransactionPool, TransactionSource};
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderBackend;
use sp_core::bandersnatch;
use sp_keystore::KeystorePtr;
use sp_runtime::traits::Block as BlockT;
use std::sync::Arc;
use std::time::Duration;

/// Aux-DB key for a ticket→authority mapping.
/// Format: `safrole_ticket::{epoch}::{ticket_id_hex}`
/// Value: authority index (u32, little-endian)
fn ticket_aux_key(epoch: EpochIndex, ticket_id: &[u8; 32]) -> Vec<u8> {
	let mut key = Vec::with_capacity(64);
	key.extend_from_slice(b"safrole_ticket::");
	key.extend_from_slice(&epoch.to_le_bytes());
	key.extend_from_slice(b"::");
	key.extend_from_slice(ticket_id);
	key
}

/// Look up which authority index owns a ticket in the aux-DB.
pub fn lookup_ticket_owner<S: AuxStore>(
	aux_store: &S,
	epoch: EpochIndex,
	ticket_id: &[u8; 32],
) -> Option<u32> {
	let key = ticket_aux_key(epoch, ticket_id);
	let data = aux_store.get_aux(&key).ok()??;
	Some(u32::from_le_bytes(data.try_into().ok()?))
}

/// Store a ticket→authority mapping in the aux-DB.
fn store_ticket_owner<S: AuxStore>(
	aux_store: &S,
	epoch: EpochIndex,
	ticket_id: &[u8; 32],
	authority_index: u32,
) {
	let key = ticket_aux_key(epoch, ticket_id);
	let value = authority_index.to_le_bytes();
	let _ = aux_store.insert_aux(&[(&key[..], &value[..])], &[]);
}

/// Inherent data provider for the ring verifier key.
///
/// Computes the `RingVerifierKey` from the authority set when it changes and
/// provides it as inherent data so the block producer can include a
/// `set_ring_verifier_key` inherent.
pub struct RingVerifierKeyProvider {
	key_bytes: Option<Vec<u8>>,
}

impl RingVerifierKeyProvider {
	/// Create a provider that supplies the given verifier key bytes.
	pub fn new(key_bytes: Option<Vec<u8>>) -> Self {
		Self { key_bytes }
	}

	/// Compute the ring verifier key from authorities using the testing SRS.
	pub fn from_authorities(authorities: &[pallet_safrole::AuthorityId]) -> Self {
		if authorities.is_empty() {
			return Self::new(None);
		}
		let raw_pks: Vec<bandersnatch::Public> = authorities
			.iter()
			.map(|a| {
				let bytes: &[u8] = a.as_ref();
				bandersnatch::Public::from_raw(bytes.try_into().expect("32 bytes"))
			})
			.collect();
		let ring_ctx = bandersnatch::ring_vrf::RingContext::<MAX_RING_SIZE>::new_testing();
		let vk = ring_ctx.verifier_key(&raw_pks);
		let vk_bytes = vk.encode();
		Self::new(Some(vk_bytes))
	}
}

#[async_trait::async_trait]
impl sp_inherents::InherentDataProvider for RingVerifierKeyProvider {
	async fn provide_inherent_data(
		&self,
		inherent_data: &mut sp_inherents::InherentData,
	) -> Result<(), sp_inherents::Error> {
		if let Some(ref key_bytes) = self.key_bytes {
			inherent_data.put_data(
				pallet_safrole::RING_VERIFIER_KEY_INHERENT_ID,
				key_bytes,
			)?;
		}
		Ok(())
	}

	async fn try_handle_error(
		&self,
		_identifier: &sp_inherents::InherentIdentifier,
		_error: &[u8],
	) -> Option<Result<(), sp_inherents::Error>> {
		None
	}
}

/// Callback that converts a `TicketEnvelope` into an encoded unsigned extrinsic.
/// Provided by the service layer which knows the concrete runtime types.
pub type TicketExtrinsicBuilder = Arc<dyn Fn(TicketEnvelope) -> Vec<u8> + Send + Sync>;

/// Run the ticket generation background task.
///
/// Polls for epoch changes and generates ring-VRF tickets for each local
/// authority key. Tickets are submitted as unsigned extrinsics to the
/// transaction pool via the `extrinsic_builder` callback.
pub async fn run_ticket_worker<B, C, P>(
	client: Arc<C>,
	pool: Arc<P>,
	keystore: KeystorePtr,
	slot_duration_ms: u64,
	extrinsic_builder: TicketExtrinsicBuilder,
) where
	B: BlockT,
	C: HeaderBackend<B> + ProvideRuntimeApi<B> + AuxStore + Send + Sync + 'static,
	C::Api: SafroleRuntimeApi<B>,
	P: TransactionPool<Block = B> + 'static,
{
	let mut last_epoch: EpochIndex = 0;
	// Poll frequently to catch epoch changes early.
	let poll_interval = Duration::from_millis(slot_duration_ms / 6).max(Duration::from_millis(500));

	loop {
		tokio::time::sleep(poll_interval).await;

		let best = client.info().best_hash;
		let api = client.runtime_api();

		let Ok(current_epoch) = api.current_epoch(best) else { continue };
		if current_epoch == last_epoch {
			continue;
		}
		last_epoch = current_epoch;

		log::info!(target: "safrole", "Epoch {current_epoch}: generating ring-VRF tickets...");

		let Ok(authorities) = api.authorities(best) else { continue };
		let Ok(epoch_randomness) = api.epoch_randomness(best) else { continue };

		if authorities.is_empty() {
			continue;
		}

		let raw_pks: Vec<bandersnatch::Public> = authorities
			.iter()
			.map(|a| {
				let bytes: &[u8] = a.as_ref();
				bandersnatch::Public::from_raw(bytes.try_into().expect("32 bytes"))
			})
			.collect();

		// Construct ring context (expensive, once per epoch).
		let ring_ctx = bandersnatch::ring_vrf::RingContext::<MAX_RING_SIZE>::new_testing();

		let mut submitted = 0u32;
		for (idx, authority) in authorities.iter().enumerate() {
			let raw_pub: &[u8] = authority.as_ref();
			if !keystore.has_keys(&[(raw_pub.to_vec(), KEY_TYPE)]) {
				continue;
			}

			let prover = ring_ctx.prover(&raw_pks, idx);
			let raw_key =
				bandersnatch::Public::from_raw(raw_pub.try_into().expect("32 bytes"));

			// Generate tickets for each attempt.
			// TicketsPerValidator is a runtime constant; hardcode a reasonable upper bound.
			// Tickets with attempt >= TicketsPerValidator will be rejected by ValidateUnsigned.
			for attempt in 0u8..2 {
				let mut vrf_input_data = Vec::with_capacity(10 + 32 + 1);
				vrf_input_data.extend_from_slice(b"jam_ticket");
				vrf_input_data.extend_from_slice(&epoch_randomness);
				vrf_input_data.push(attempt);

				let vrf_sign_data =
					bandersnatch::vrf::VrfSignData::new(&vrf_input_data, b"");

				let Ok(Some(ring_sig)) = keystore.bandersnatch_ring_vrf_sign(
					KEY_TYPE,
					&raw_key,
					&vrf_sign_data,
					&prover,
				) else {
					log::warn!(
						target: "safrole",
						"Failed to ring-VRF sign ticket for attempt {attempt}"
					);
					continue;
				};

				let ticket_id = ring_sig.pre_output.make_bytes();

				// Store ticket→authority mapping in aux-DB for O(1) lookup
				// during block production (claim_slot_ticket).
				store_ticket_owner(client.as_ref(), current_epoch, &ticket_id, idx as u32);

				let envelope = TicketEnvelope {
					ticket: Ticket { id: ticket_id, attempt },
					ring_signature: ring_sig,
				};

				log::debug!(
					target: "safrole",
					"Generated ticket: attempt={attempt}, id={:02x}{:02x}{:02x}{:02x}...",
					ticket_id[0], ticket_id[1], ticket_id[2], ticket_id[3]
				);

				// Build unsigned extrinsic and submit to pool.
				let encoded_xt = (extrinsic_builder)(envelope);
				let opaque_xt = B::Extrinsic::decode(&mut &encoded_xt[..]);
				match opaque_xt {
					Ok(xt) => {
						match pool.submit_one(best, TransactionSource::Local, xt).await {
							Ok(_) => submitted += 1,
							Err(e) => log::warn!(
								target: "safrole",
								"Failed to submit ticket to pool: {e}"
							),
						}
					},
					Err(e) => log::warn!(
						target: "safrole",
						"Failed to decode ticket extrinsic: {e}"
					),
				}
			}
		}

		if submitted > 0 {
			log::info!(
				target: "safrole",
				"Epoch {current_epoch}: submitted {submitted} ring-VRF tickets to pool",
			);
		}
	}
}
