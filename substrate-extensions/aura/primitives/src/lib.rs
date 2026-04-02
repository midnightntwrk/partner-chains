// Re-export from consensus-common for backward compatibility.
// New code should depend on sp-partner-chains-consensus-common directly.
pub use sp_partner_chains_consensus_common::block_proposal;
pub use sp_partner_chains_consensus_common::inherent_digest;

pub use sp_partner_chains_consensus_common::inherent_digest::InherentDigest;
pub use sp_partner_chains_consensus_common::CurrentSlotProvider;
