//! Constants and types used by the token bridge

use cardano_serialization_lib::{JsError, TransactionMetadatum};

/// Arbitrary key, used as top-level metadatum key 6500973 = 0x63326d ~= 'c2n'
pub const TOKEN_TRANSFER_METADATUM_KEY: u64 = 6500973;

/// Creates TransactionMetadatum for token transfer to specified address
pub fn transfer_to_address_transaction_metadatum(
	address_bytes: &[u8],
) -> Result<TransactionMetadatum, JsError> {
	TransactionMetadatum::new_bytes(address_bytes.to_vec())
}

/// Creates TransactionMetadatum of reserve transfer
pub fn transfer_to_reserve_metadatum() -> TransactionMetadatum {
	TransactionMetadatum::new_text("reserve".to_string())
		.expect("'reserve' is correct metadatum text")
}
