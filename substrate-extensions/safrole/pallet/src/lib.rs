//! # Safrole Pallet
//!
//! VRF-based block production following the JAM Gray Paper's Safrole protocol.
//!
//! Replaces Aura with a ticket-based slot assignment system using Bandersnatch VRFs.
//! Validators submit anonymous ring-VRF tickets during epoch N; the sorted tickets
//! determine who produces blocks in epoch N+1. Falls back to per-slot VRF when
//! insufficient tickets are collected.
//!
//! ## Usage
//!
//! Configure the pallet in your runtime:
//!
//! ```ignore
//! impl pallet_safrole::Config for Runtime {
//!     type MaxAuthorities = ConstU32<1024>;
//!     type EpochLength = ConstU32<600>;
//!     type SlotDuration = ConstU64<6000>;
//!     type TicketsPerValidator = ConstU32<2>;
//! }
//! ```
//!
//! Other pallets can read the current slot via:
//!
//! ```ignore
//! let slot = pallet_safrole::CurrentSlot::<Runtime>::get();
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

use frame_support::pallet_prelude::*;
use frame_support::traits::OnTimestampSet;
use frame_system::pallet_prelude::*;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_consensus_slots::Slot;
use sp_core::bandersnatch;
use sp_core::crypto::KeyTypeId;
use sp_inherents::InherentIdentifier;

/// Key type identifier for Safrole Bandersnatch keys.
pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"safr");

/// Maximum ring size for ring-VRF proofs.
/// Must be >= the actual authority set size. Controlled via feature flags:
/// `size-full` = 1024, otherwise 16 (suitable for tiny/small profiles).
#[cfg(feature = "size-full")]
pub const MAX_RING_SIZE: usize = 1024;
#[cfg(not(feature = "size-full"))]
pub const MAX_RING_SIZE: usize = 16;

/// Inherent identifier for ring verifier key updates.
pub const RING_VERIFIER_KEY_INHERENT_ID: InherentIdentifier = *b"safrolek";

/// Application crypto types for Safrole (Bandersnatch-based).
/// This gives us `Public`, `Pair`, `Signature` types that implement
/// `RuntimeAppPublic`, which is required by `impl_opaque_keys!`.
pub mod app {
	use sp_application_crypto::{app_crypto, bandersnatch};
	use super::KEY_TYPE;
	app_crypto!(bandersnatch, KEY_TYPE);
}

/// Safrole authority keypair.
#[cfg(feature = "std")]
pub type AuthorityPair = app::Pair;

/// Safrole authority signature.
pub type AuthoritySignature = app::Signature;

/// Safrole authority identifier (Bandersnatch public key).
pub type AuthorityId = app::Public;

/// A VRF ticket submitted by a validator for slot assignment.
#[derive(Clone, Debug, Encode, Decode, MaxEncodedLen, TypeInfo, PartialEq, Eq)]
pub struct Ticket {
	/// The VRF output, used as the ticket identifier and for sorting.
	pub id: [u8; 32],
	/// Which attempt index produced this ticket (each validator gets multiple attempts).
	pub attempt: u8,
}

impl PartialOrd for Ticket {
	fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for Ticket {
	fn cmp(&self, other: &Self) -> core::cmp::Ordering {
		self.id.cmp(&other.id)
	}
}

/// Ring VRF proof accompanying a ticket submission.
/// 752 bytes for the ring proof + 32 bytes for the VRF pre-output.
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub struct TicketEnvelope {
	/// The ticket body (VRF output + attempt index).
	pub ticket: Ticket,
	/// Ring VRF signature proving membership in the authority set
	/// without revealing which authority submitted it.
	pub ring_signature: bandersnatch::ring_vrf::RingVrfSignature,
}

// DecodeWithMemTracking impls — needed for pallet Call parameter types.
// Manual impls because bandersnatch::ring_vrf::RingVrfSignature doesn't derive it.
impl parity_scale_codec::DecodeWithMemTracking for Ticket {}
impl parity_scale_codec::DecodeWithMemTracking for TicketEnvelope {}

/// Epoch index type.
pub type EpochIndex = u64;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// Maximum number of authorities (validators).
		#[pallet::constant]
		type MaxAuthorities: Get<u32>;

		/// Number of slots per epoch.
		#[pallet::constant]
		type EpochLength: Get<u32>;

		/// Slot duration in milliseconds.
		#[pallet::constant]
		type SlotDuration: Get<u64>;

		/// Number of ticket submission attempts per validator per epoch.
		#[pallet::constant]
		type TicketsPerValidator: Get<u32>;
	}

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	// ── Storage ──────────────────────────────────────────────────────────

	/// The current slot number, updated each block.
	#[pallet::storage]
	pub type CurrentSlot<T: Config> = StorageValue<_, Slot, ValueQuery>;

	/// The current epoch index.
	#[pallet::storage]
	pub type CurrentEpoch<T: Config> = StorageValue<_, EpochIndex, ValueQuery>;

	/// The Bandersnatch public keys of the current epoch's authorities.
	#[pallet::storage]
	pub type Authorities<T: Config> =
		StorageValue<_, BoundedVec<AuthorityId, T::MaxAuthorities>, ValueQuery>;

	/// The Bandersnatch keys pending for the next epoch (set by session rotation).
	#[pallet::storage]
	pub type NextAuthorities<T: Config> =
		StorageValue<_, BoundedVec<AuthorityId, T::MaxAuthorities>, ValueQuery>;

	/// Tickets accumulating for the next epoch's slot assignment.
	/// Kept sorted by ticket id (ascending). Bounded to EpochLength since
	/// we only need as many tickets as there are slots.
	#[pallet::storage]
	pub type TicketAccumulator<T: Config> =
		StorageValue<_, BoundedVec<Ticket, T::EpochLength>, ValueQuery>;

	/// The finalized ticket-to-slot mapping for the current epoch.
	/// If `None`, the epoch is in fallback mode.
	#[pallet::storage]
	pub type EpochTickets<T: Config> =
		StorageValue<_, BoundedVec<Ticket, T::EpochLength>, OptionQuery>;

	/// Randomness seed for the current epoch, derived from accumulated VRF outputs.
	#[pallet::storage]
	pub type EpochRandomness<T: Config> = StorageValue<_, [u8; 32], ValueQuery>;

	/// Randomness being accumulated for the next epoch.
	#[pallet::storage]
	pub type NextEpochRandomness<T: Config> = StorageValue<_, [u8; 32], ValueQuery>;

	/// Whether the current epoch is in fallback mode (insufficient tickets).
	#[pallet::storage]
	pub type InFallbackMode<T: Config> = StorageValue<_, bool, ValueQuery>;

	/// The ring verifier key for the current authority set.
	/// Cached to avoid recomputation every block.
	#[pallet::storage]
	pub type RingVerifierKeyBytes<T: Config> = StorageValue<_, BoundedVec<u8, ConstU32<512>>, OptionQuery>;

	// ── Genesis ──────────────────────────────────────────────────────────

	#[pallet::genesis_config]
	#[derive(frame_support::DefaultNoBound)]
	pub struct GenesisConfig<T: Config> {
		/// Initial authorities for the first epoch.
		pub authorities: BoundedVec<AuthorityId, T::MaxAuthorities>,
	}

	#[pallet::genesis_build]
	impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
		fn build(&self) {
			Authorities::<T>::put(&self.authorities);
			NextAuthorities::<T>::put(&self.authorities);
			// First epoch starts in fallback mode (no tickets yet).
			InFallbackMode::<T>::put(true);

			// Compute and store the ring verifier key for the genesis authority set.
			#[cfg(feature = "std")]
			if !self.authorities.is_empty() {
				let raw_pks: alloc::vec::Vec<sp_core::bandersnatch::Public> = self
					.authorities
					.iter()
					.map(|a| {
						let bytes: &[u8] = a.as_ref();
						sp_core::bandersnatch::Public::from_raw(
							bytes.try_into().expect("AuthorityId is 32 bytes"),
						)
					})
					.collect();
				let ring_ctx =
					sp_core::bandersnatch::ring_vrf::RingContext::<MAX_RING_SIZE>::new_testing();
				let vk = ring_ctx.verifier_key(&raw_pks);
				let vk_bytes = parity_scale_codec::Encode::encode(&vk);
				if let Ok(bounded) = BoundedVec::try_from(vk_bytes) {
					RingVerifierKeyBytes::<T>::put(bounded);
				}
			}
		}
	}

	// ── Hooks ────────────────────────────────────────────────────────────

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
			Weight::zero()
		}

		fn on_finalize(_n: BlockNumberFor<T>) {
			// Accumulate randomness: mix in the current block's parent hash.
			// In production this should use the seal's VRF output; for now
			// the parent hash provides sufficient entropy for distinct epochs.
			let parent_hash = frame_system::Pallet::<T>::parent_hash();
			let mut next = NextEpochRandomness::<T>::get();
			let hash_bytes = parent_hash.as_ref();
			for (i, b) in hash_bytes.iter().enumerate().take(32) {
				next[i] ^= b;
			}
			NextEpochRandomness::<T>::put(next);
		}
	}

	// ── Errors ───────────────────────────────────────────────────────────

	#[pallet::error]
	pub enum Error<T> {
		/// Ticket accumulator is full (EpochLength tickets already collected).
		AccumulatorFull,
		/// Ticket attempt index exceeds TicketsPerValidator.
		InvalidAttempt,
		/// A ticket with this ID has already been submitted.
		DuplicateTicket,
		/// Ring verifier key not set (no authority set configured).
		NoRingVerifierKey,
		/// Ring-VRF proof verification failed.
		InvalidRingVrfProof,
		/// VRF output does not match the claimed ticket ID.
		TicketIdMismatch,
	}

	// ── Calls ────────────────────────────────────────────────────────────

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Submit a ring-VRF ticket for the next epoch's slot assignment.
		///
		/// This is an unsigned extrinsic — validators submit tickets anonymously
		/// via the transaction pool. The ring-VRF proof proves membership in the
		/// authority set without revealing which validator submitted it.
		#[pallet::call_index(0)]
		#[pallet::weight((Weight::from_parts(100_000_000, 0), DispatchClass::Operational))]
		pub fn submit_ticket(
			origin: OriginFor<T>,
			envelope: alloc::boxed::Box<TicketEnvelope>,
		) -> DispatchResult {
			ensure_none(origin)?;

			let mut accumulator = TicketAccumulator::<T>::get();
			ensure!(
				(accumulator.len() as u32) < T::EpochLength::get(),
				Error::<T>::AccumulatorFull
			);
			ensure!(
				(envelope.ticket.attempt as u32) < T::TicketsPerValidator::get(),
				Error::<T>::InvalidAttempt
			);
			ensure!(
				accumulator.binary_search(&envelope.ticket).is_err(),
				Error::<T>::DuplicateTicket
			);

			// Verify ring-VRF proof.
			let vk_bytes = RingVerifierKeyBytes::<T>::get()
				.ok_or(Error::<T>::NoRingVerifierKey)?;
			let verifier_key = bandersnatch::ring_vrf::RingVerifierKey::decode(
				&mut &vk_bytes[..],
			)
			.map_err(|_| Error::<T>::NoRingVerifierKey)?;
			let verifier = bandersnatch::ring_vrf::RingContext::<MAX_RING_SIZE>::verifier_no_context(
				verifier_key,
			);

			let epoch_randomness = EpochRandomness::<T>::get();
			let mut vrf_input_data = alloc::vec::Vec::with_capacity(32 + 1 + 10);
			vrf_input_data.extend_from_slice(b"jam_ticket");
			vrf_input_data.extend_from_slice(&epoch_randomness);
			vrf_input_data.push(envelope.ticket.attempt);

			let _vrf_input = bandersnatch::vrf::VrfInput::new(&vrf_input_data);
			let vrf_sign_data = bandersnatch::vrf::VrfSignData::new(&vrf_input_data, b"");

			ensure!(
				envelope.ring_signature.ring_vrf_verify(&vrf_sign_data, &verifier),
				Error::<T>::InvalidRingVrfProof
			);

			// Verify ticket ID matches VRF output.
			let output_bytes = envelope.ring_signature.pre_output.make_bytes();
			ensure!(
				output_bytes == envelope.ticket.id,
				Error::<T>::TicketIdMismatch
			);

			// Insert sorted.
			let insert_pos = accumulator
				.binary_search(&envelope.ticket)
				.unwrap_err();
			accumulator
				.try_insert(insert_pos, envelope.ticket)
				.map_err(|_| Error::<T>::AccumulatorFull)?;
			TicketAccumulator::<T>::put(accumulator);

			Ok(())
		}

		/// Set the ring verifier key for the current authority set.
		/// Called as an inherent by the block producer when authorities change.
		#[pallet::call_index(1)]
		#[pallet::weight((Weight::from_parts(10_000_000, 0), DispatchClass::Mandatory))]
		pub fn set_ring_verifier_key(
			origin: OriginFor<T>,
			key_bytes: BoundedVec<u8, ConstU32<512>>,
		) -> DispatchResult {
			ensure_none(origin)?;
			RingVerifierKeyBytes::<T>::put(key_bytes);
			Ok(())
		}
	}

	// ── ValidateUnsigned ────────────────────────────────────────────────

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T> {
		type Call = Call<T>;

		fn validate_unsigned(
			_source: TransactionSource,
			call: &Self::Call,
		) -> TransactionValidity {
			match call {
				Call::submit_ticket { envelope } => {
					// Cheap checks only — ring-VRF verification is too expensive
					// for pool validation and would be a DoS vector.
					// Full capacity + duplicate checks happen in dispatch.
					if (envelope.ticket.attempt as u32) >= T::TicketsPerValidator::get() {
						return InvalidTransaction::BadProof.into();
					}
					let accumulator = TicketAccumulator::<T>::get();
					if accumulator.binary_search(&envelope.ticket).is_ok() {
						return InvalidTransaction::Stale.into();
					}

					ValidTransaction::with_tag_prefix("SafroleTicket")
						.priority(TransactionPriority::MAX)
						.and_provides(envelope.ticket.id)
						.longevity(T::EpochLength::get() as u64)
						.propagate(true)
						.build()
				},
				Call::set_ring_verifier_key { .. } => {
					// Inherent — always valid.
					ValidTransaction::with_tag_prefix("SafroleVerifierKey")
						.priority(TransactionPriority::MAX)
						.longevity(1)
						.propagate(false)
						.build()
				},
				_ => InvalidTransaction::Call.into(),
			}
		}
	}

	// ── ProvideInherent (ring verifier key) ─────────────────────────────

	#[pallet::inherent]
	impl<T: Config> ProvideInherent for Pallet<T> {
		type Call = Call<T>;
		type Error = sp_inherents::MakeFatalError<()>;
		const INHERENT_IDENTIFIER: InherentIdentifier = RING_VERIFIER_KEY_INHERENT_ID;

		fn create_inherent(data: &InherentData) -> Option<Self::Call> {
			let key_bytes: alloc::vec::Vec<u8> =
				data.get_data(&Self::INHERENT_IDENTIFIER).ok()??;
			let bounded = BoundedVec::try_from(key_bytes).ok()?;
			Some(Call::set_ring_verifier_key { key_bytes: bounded })
		}

		fn is_inherent(call: &Self::Call) -> bool {
			matches!(call, Call::set_ring_verifier_key { .. })
		}

		fn is_inherent_required(_data: &InherentData) -> Result<Option<Self::Error>, Self::Error> {
			// Not required every block — only when authority set changes.
			Ok(None)
		}
	}

	// ── Public API ───────────────────────────────────────────────────────

	impl<T: Config> Pallet<T> {
		/// Set the current slot. Called by the consensus engine via inherent/pre-digest.
		pub fn set_slot(slot: Slot) {
			CurrentSlot::<T>::put(slot);

			let epoch_length = T::EpochLength::get() as u64;
			if epoch_length == 0 {
				return;
			}
			let new_epoch = *slot / epoch_length;
			let old_epoch = CurrentEpoch::<T>::get();

			if new_epoch > old_epoch {
				Self::rotate_epoch(new_epoch);
			}
		}

		/// Get the slot duration in milliseconds.
		pub fn slot_duration() -> u64 {
			T::SlotDuration::get()
		}

		/// Determine the expected block author for a given slot.
		/// Returns `None` if the slot mapping cannot be determined.
		pub fn slot_author(slot: Slot) -> Option<AuthorityId> {
			let authorities = Authorities::<T>::get();
			if authorities.is_empty() {
				return None;
			}

			let epoch_length = T::EpochLength::get() as u64;
			let slot_in_epoch = (*slot % epoch_length) as usize;

			if let Some(tickets) = EpochTickets::<T>::get() {
				// Ticket mode: the ticket at this slot index determines the author.
				// The actual author identity is revealed when they seal the block
				// with a Bandersnatch proof matching the ticket.
				// We can't directly map ticket -> author here (that's the anonymity
				// property), so return None in ticket mode. The consensus engine
				// handles this via VRF proofs.
				let _ = tickets.get(slot_in_epoch);
				None
			} else {
				// Fallback mode: deterministic VRF-based assignment.
				let randomness = EpochRandomness::<T>::get();
				let mut input = [0u8; 40];
				input[..32].copy_from_slice(&randomness);
				input[32..40].copy_from_slice(&slot.encode()[..]);
				let hash = sp_io::hashing::blake2_256(&input);
				let idx = u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]]) as usize;
				Some(authorities[idx % authorities.len()].clone())
			}
		}

		/// Handle epoch rotation: finalize tickets, rotate authorities, update randomness.
		fn rotate_epoch(new_epoch: EpochIndex) {
			let epoch_length = T::EpochLength::get() as u64;

			// Rotate authorities.
			let next = NextAuthorities::<T>::get();
			if !next.is_empty() {
				Authorities::<T>::put(&next);
			}

			// Finalize ticket accumulator for the new epoch.
			// Only consume the accumulator if we have enough tickets.
			// Otherwise keep accumulating across epochs until we do.
			let tickets = TicketAccumulator::<T>::get();

			if tickets.len() as u64 >= epoch_length {
				TicketAccumulator::<T>::kill();
				// Enough tickets: reorder outside-in for security (Gray Paper §6).
				// Outside-in: alternate taking from front and back of sorted list.
				let mut reordered = BoundedVec::default();
				let mut front = 0usize;
				let mut back = tickets.len().saturating_sub(1);
				let mut take_front = true;
				while front <= back && (reordered.len() as u64) < epoch_length {
					let idx = if take_front { front } else { back };
					let _ = reordered.try_push(tickets[idx].clone());
					if take_front {
						front += 1;
					} else {
						if back == 0 {
							break;
						}
						back -= 1;
					}
					take_front = !take_front;
				}
				EpochTickets::<T>::put(reordered);
				InFallbackMode::<T>::put(false);
			} else {
				// Not enough tickets: enter fallback mode.
				EpochTickets::<T>::kill();
				InFallbackMode::<T>::put(true);
				log::warn!(
					target: "safrole",
					"Epoch {}: only {} tickets for {} slots, entering fallback mode",
					new_epoch,
					tickets.len(),
					epoch_length,
				);
			}

			// Rotate randomness.
			let next_randomness = NextEpochRandomness::<T>::get();
			EpochRandomness::<T>::put(next_randomness);
			NextEpochRandomness::<T>::put([0u8; 32]);

			CurrentEpoch::<T>::put(new_epoch);
		}
	}

	/// Implement `OnTimestampSet` so pallet_timestamp drives slot updates,
	/// exactly as pallet_aura does.
	impl<T: Config> OnTimestampSet<T::Moment> for Pallet<T>
	where
		T: pallet_timestamp::Config,
	{
		fn on_timestamp_set(moment: T::Moment) {
			let slot_duration = Self::slot_duration();
			assert!(
				slot_duration != 0,
				"Safrole slot duration cannot be zero."
			);

			let timestamp_slot =
				moment / <T::Moment as sp_runtime::traits::UniqueSaturatedFrom<u64>>::unique_saturated_from(slot_duration);
			let timestamp_slot = Slot::from(
				<T::Moment as sp_runtime::traits::UniqueSaturatedInto<u64>>::unique_saturated_into(
					timestamp_slot,
				),
			);

			assert!(
				CurrentSlot::<T>::get() <= timestamp_slot,
				"Timestamp slot must not decrease: current={:?}, new={:?}",
				CurrentSlot::<T>::get(),
				timestamp_slot,
			);

			Self::set_slot(timestamp_slot);
		}
	}
}

impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Pallet<T> {
	type Public = AuthorityId;
}

impl<T: Config> frame_support::traits::OneSessionHandler<T::AccountId> for Pallet<T> {
	type Key = AuthorityId;

	fn on_genesis_session<'a, I: 'a>(validators: I)
	where
		I: Iterator<Item = (&'a T::AccountId, AuthorityId)>,
	{
		let authorities: BoundedVec<_, T::MaxAuthorities> = validators
			.map(|(_, k)| k)
			.collect::<alloc::vec::Vec<_>>()
			.try_into()
			.expect("Initial authority set too large");
		Authorities::<T>::put(&authorities);
		NextAuthorities::<T>::put(&authorities);
	}

	fn on_new_session<'a, I: 'a>(_changed: bool, validators: I, _queued: I)
	where
		I: Iterator<Item = (&'a T::AccountId, AuthorityId)>,
	{
		let next: BoundedVec<_, T::MaxAuthorities> = validators
			.map(|(_, k)| k)
			.collect::<alloc::vec::Vec<_>>()
			.try_into()
			.expect("Authority set too large");
		// Set both current and next: the session pallet already handles staging,
		// so when on_new_session fires the validators are the ones that should be
		// active *now*, matching pallet_aura's behavior.
		Authorities::<T>::put(&next);
		NextAuthorities::<T>::put(&next);
	}

	fn on_disabled(_i: u32) {
		// Safrole doesn't support disabling individual validators mid-epoch.
	}
}

/// Safrole consensus engine ID.
pub const SAFROLE_ENGINE_ID: sp_runtime::ConsensusEngineId = *b"SAFR";

/// Pre-digest data included in block headers by the Safrole consensus engine.
#[derive(Clone, Debug, Encode, Decode, TypeInfo)]
pub enum SafrolePreDigest {
	/// Block produced via ticket claim.
	Ticket {
		slot: Slot,
		/// Index into the epoch's ticket array.
		ticket_index: u32,
		/// The VRF proof linking the block author to the ticket.
		vrf_signature: bandersnatch::vrf::VrfSignature,
	},
	/// Block produced in fallback mode.
	Fallback {
		slot: Slot,
		/// Index of the authority in the authority set.
		authority_index: u32,
		/// VRF signature proving this authority is the slot leader.
		vrf_signature: bandersnatch::vrf::VrfSignature,
	},
}

impl SafrolePreDigest {
	/// Extract the slot from the pre-digest.
	pub fn slot(&self) -> Slot {
		match self {
			Self::Ticket { slot, .. } | Self::Fallback { slot, .. } => *slot,
		}
	}

	/// Extract the authority index (only available in fallback mode).
	pub fn authority_index(&self) -> Option<u32> {
		match self {
			Self::Fallback { authority_index, .. } => Some(*authority_index),
			Self::Ticket { .. } => None,
		}
	}
}

/// Find the Safrole pre-digest in a block's header digests.
pub fn find_pre_digest<B: sp_runtime::traits::Block>(
	header: &B::Header,
) -> Option<SafrolePreDigest> {
	use sp_runtime::traits::Header;
	header.digest().logs().iter().find_map(|log| {
		log.as_pre_runtime().and_then(|(id, data)| {
			if id == SAFROLE_ENGINE_ID {
				SafrolePreDigest::decode(&mut &data[..]).ok()
			} else {
				None
			}
		})
	})
}

// ── Runtime API ──────────────────────────────────────────────────────

sp_api::decl_runtime_apis! {
	/// Runtime API for the Safrole consensus engine.
	pub trait SafroleApi {
		/// Get the slot duration in milliseconds.
		fn slot_duration() -> sp_consensus_slots::SlotDuration;
		/// Get the current authority set (Bandersnatch public keys).
		fn authorities() -> alloc::vec::Vec<AuthorityId>;
		/// Get the epoch tickets for the current epoch.
		/// Returns `None` if in fallback mode.
		fn epoch_tickets() -> Option<alloc::vec::Vec<Ticket>>;
		/// Whether the chain is in fallback mode.
		fn is_fallback_mode() -> bool;
		/// The current epoch's randomness seed.
		fn epoch_randomness() -> [u8; 32];
		/// The current epoch index.
		fn current_epoch() -> EpochIndex;
		/// The encoded ring verifier key for the current authority set.
		fn ring_verifier_key() -> Option<alloc::vec::Vec<u8>>;
	}
}
