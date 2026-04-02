mod header_verification {
	use crate::Header;
	use pallet_safrole::SAFROLE_ENGINE_ID;
	use parity_scale_codec::Encode;
	use sp_core::bandersnatch;
	use sp_core::crypto::Pair;
	use sp_runtime::traits::{BlakeTwo256, Hash};
	use sp_runtime::DigestItem;

	#[test]
	fn test_header_hashing_and_bandersnatch_seal() {
		// Generate a Bandersnatch keypair for testing
		let (pair, _) = bandersnatch::Pair::generate();

		// Build a minimal header with a Safrole pre-runtime digest
		let slot = sp_consensus_slots::Slot::from(42u64);
		let pre_digest = DigestItem::PreRuntime(SAFROLE_ENGINE_ID, slot.encode());

		let mut header = Header {
			parent_hash: BlakeTwo256::hash(b"parent"),
			number: 1,
			state_root: BlakeTwo256::hash(b"state"),
			extrinsics_root: BlakeTwo256::hash(b"extrinsics"),
			digest: sp_runtime::Digest { logs: vec![pre_digest] },
		};

		// Hash the header (without the seal)
		let header_hash = BlakeTwo256::hash(&header.encode());

		// Sign the header hash with the Bandersnatch key
		let signature = pair.sign(header_hash.as_ref());

		// Append the seal to the header
		header
			.digest
			.logs
			.push(DigestItem::Seal(SAFROLE_ENGINE_ID, signature.encode()));

		// Verify the seal
		assert!(bandersnatch::Pair::verify(
			&signature,
			header_hash.as_ref(),
			&pair.public()
		));
	}
}
