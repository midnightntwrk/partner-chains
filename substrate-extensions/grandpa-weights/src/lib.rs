//! # Grandpa Weights Pallet
//!
//! Injects stake-proportional authority weights into `pallet_grandpa`.
//!
//! `pallet_grandpa` hardcodes all authority weights to 1 in its `OneSessionHandler`.
//! This pallet stores a weight map (GrandpaId -> stake) and patches the weights
//! in `on_finalize`, before `pallet_grandpa::on_finalize` emits the `ScheduledChange`
//! digest log to the GRANDPA consensus client.
//!
//! **Ordering requirement**: This pallet MUST be declared AFTER `pallet_grandpa` in
//! `construct_runtime!` so that its `on_finalize` runs first (reverse declaration order).

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use sp_consensus_grandpa::{AuthorityId, AuthorityWeight};
	use sp_std::vec::Vec;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + pallet_grandpa::Config {}

	/// Map from GRANDPA authority ID to its stake-proportional weight.
	/// Permissioned authorities have weight 1; registered authorities
	/// have weight equal to their `StakeDelegation` (Lovelace).
	#[pallet::storage]
	pub type AuthorityWeights<T: Config> =
		StorageMap<_, Twox64Concat, AuthorityId, AuthorityWeight, OptionQuery>;

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_finalize(_n: BlockNumberFor<T>) {
			Self::patch_pending_change_weights();
		}
	}

	impl<T: Config> Pallet<T> {
		/// Replace the weight map with new values.
		/// Called from the runtime's `select_authorities` when a new committee is selected.
		pub fn set_weights(weights: impl IntoIterator<Item = (AuthorityId, AuthorityWeight)>) {
			let _ = AuthorityWeights::<T>::clear(u32::MAX, None);
			for (id, weight) in weights {
				AuthorityWeights::<T>::insert(&id, weight);
			}
		}

		/// Returns the GRANDPA authority list with stake-proportional weights.
		/// Falls back to weight=1 for authorities not in the weight map.
		pub fn weighted_grandpa_authorities() -> Vec<(AuthorityId, AuthorityWeight)> {
			pallet_grandpa::Pallet::<T>::grandpa_authorities()
				.into_iter()
				.map(|(id, _)| {
					let weight = AuthorityWeights::<T>::get(&id).unwrap_or(1);
					(id, weight)
				})
				.collect()
		}

		/// Patch the weights in `pallet_grandpa::PendingChange` storage
		/// so the `ScheduledChange` digest log carries correct weights.
		fn patch_pending_change_weights() {
			if let Some(pending) = pallet_grandpa::PendingChange::<T>::get() {
				let mut authorities: Vec<_> = pending.next_authorities.into_inner();
				let mut patched = false;
				for (id, weight) in authorities.iter_mut() {
					if let Some(w) = AuthorityWeights::<T>::get(id) {
						if *weight != w {
							*weight = w;
							patched = true;
						}
					}
				}
				if patched {
					log::debug!(
						target: "grandpa-weights",
						"Patched PendingChange with stake-proportional weights",
					);
					let next_authorities =
						frame_support::WeakBoundedVec::force_from(authorities, None);
					pallet_grandpa::PendingChange::<T>::put(pallet_grandpa::StoredPendingChange {
						scheduled_at: pending.scheduled_at,
						delay: pending.delay,
						next_authorities,
						forced: pending.forced,
					});
				}
			}
		}
	}
}
