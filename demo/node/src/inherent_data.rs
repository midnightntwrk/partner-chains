use authority_selection_inherents::{
	AriadneInherentDataProvider as AriadneIDP, AuthoritySelectionDataSource,
	AuthoritySelectionInputs, CommitteeMember,
};
use pallet_safrole::SafroleApi as _;
use sc_partner_chains_consensus_safrole::ticket_worker::RingVerifierKeyProvider;
use derive_new::new;
use jsonrpsee::core::async_trait;
use pallet_safrole::find_pre_digest;
use partner_chains_demo_runtime::{
	AccountId, BlockAuthor, CrossChainPublic,
	opaque::{Block, SessionKeys},
};
use sc_service::Arc;
use sidechain_domain::{
	DelegatorKey, McBlockHash, ScEpochNumber, mainchain_epoch::MainchainEpochConfig,
};
use sidechain_mc_hash::{McHashDataSource, McHashInherentDataProvider as McHashIDP};
use sidechain_slots::ScSlotConfig;
use sp_api::ProvideRuntimeApi;
use sp_block_participation::{
	BlockParticipationApi,
	inherent_data::{BlockParticipationDataSource, BlockParticipationInherentDataProvider},
};
use sp_block_production_log::{BlockAuthorInherentProvider, BlockProductionLogApi};
use sp_blockchain::HeaderBackend;
use sp_consensus_slots::{Slot, SlotDuration};

/// Minimal slot inherent data provider that satisfies `InherentDataProviderExt`
/// (first tuple element must deref to `Slot`).
/// Unlike AuraIDP, this doesn't inject an "auraslot" inherent — the slot
/// reaches pallet_safrole via pallet_timestamp's OnTimestampSet.
pub struct SlotIDP(Slot);

impl SlotIDP {
	pub fn from_timestamp_and_slot_duration(timestamp: Timestamp, slot_duration: SlotDuration) -> Self {
		Self(Slot::from_timestamp(timestamp, slot_duration))
	}
}

impl core::ops::Deref for SlotIDP {
	type Target = Slot;
	fn deref(&self) -> &Slot {
		&self.0
	}
}

#[async_trait]
impl sp_inherents::InherentDataProvider for SlotIDP {
	async fn provide_inherent_data(
		&self,
		_inherent_data: &mut sp_inherents::InherentData,
	) -> Result<(), sp_inherents::Error> {
		// No inherent data to provide — slot is set via OnTimestampSet
		Ok(())
	}

	async fn try_handle_error(
		&self,
		_: &sp_inherents::InherentIdentifier,
		_: &[u8],
	) -> Option<Result<(), sp_inherents::Error>> {
		None
	}
}
use sp_governed_map::{GovernedMapDataSource, GovernedMapIDPApi, GovernedMapInherentDataProvider};
use sp_inherents::CreateInherentDataProviders;
use sp_partner_chains_bridge::{
	TokenBridgeDataSource, TokenBridgeIDPRuntimeApi, TokenBridgeInherentDataProvider,
};
use sp_partner_chains_consensus_common::CurrentSlotProvider;
use sp_runtime::traits::{Block as BlockT, Header, Zero};
use sp_session_validator_management::SessionValidatorManagementApi;
use sp_timestamp::{InherentDataProvider as TimestampIDP, Timestamp};
use std::error::Error;
use time_source::TimeSource;

#[derive(new)]
pub struct ProposalCIDP<T> {
	config: CreateInherentDataConfig,
	client: Arc<T>,
	mc_hash_data_source: Arc<dyn McHashDataSource + Send + Sync>,
	authority_selection_data_source: Arc<dyn AuthoritySelectionDataSource + Send + Sync>,
	block_participation_data_source: Arc<dyn BlockParticipationDataSource + Send + Sync>,
	governed_map_data_source: Arc<dyn GovernedMapDataSource + Send + Sync>,
	bridge_data_source: Arc<dyn TokenBridgeDataSource<AccountId> + Send + Sync>,
}

#[async_trait]
impl<T> CreateInherentDataProviders<Block, ()> for ProposalCIDP<T>
where
	T: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	T: HeaderBackend<Block>,
	T::Api: SessionValidatorManagementApi<
			Block,
			CommitteeMember<CrossChainPublic, SessionKeys>,
			AuthoritySelectionInputs,
			ScEpochNumber,
		>,
	T::Api: BlockProductionLogApi<Block, CommitteeMember<CrossChainPublic, SessionKeys>>,
	T::Api: BlockParticipationApi<Block, BlockAuthor>,
	T::Api: GovernedMapIDPApi<Block>,
	T::Api: TokenBridgeIDPRuntimeApi<Block>,
	T::Api: pallet_safrole::SafroleApi<Block>,
{
	type InherentDataProviders = (
		SlotIDP,
		TimestampIDP,
		McHashIDP,
		AriadneIDP,
		BlockAuthorInherentProvider<BlockAuthor>,
		BlockParticipationInherentDataProvider<BlockAuthor, DelegatorKey>,
		GovernedMapInherentDataProvider,
		TokenBridgeInherentDataProvider<AccountId>,
		RingVerifierKeyProvider,
	);

	async fn create_inherent_data_providers(
		&self,
		parent_hash: <Block as BlockT>::Hash,
		_extra_args: (),
	) -> Result<Self::InherentDataProviders, Box<dyn std::error::Error + Send + Sync>> {
		let Self {
			config,
			client,
			mc_hash_data_source,
			authority_selection_data_source,
			block_participation_data_source,
			governed_map_data_source,
			bridge_data_source,
		} = self;
		let CreateInherentDataConfig { mc_epoch_config, sc_slot_config, time_source } = config;

		let timestamp = TimestampIDP::new(Timestamp::new(time_source.get_current_time_millis()));
		let slot_idp = SlotIDP::from_timestamp_and_slot_duration(*timestamp, sc_slot_config.slot_duration);
		let slot = *slot_idp;
		let parent_header = client.expect_header(parent_hash)?;
		let mc_hash = McHashIDP::new_proposal(
			parent_header,
			mc_hash_data_source.as_ref(),
			slot,
			sc_slot_config.slot_duration,
		)
		.await?;

		let ariadne_data_provider = AriadneIDP::new(
			client.as_ref(),
			sc_slot_config,
			mc_epoch_config,
			parent_hash,
			slot,
			authority_selection_data_source.as_ref(),
			mc_hash.mc_epoch(),
		)
		.await?;
		let block_producer_id_provider =
			BlockAuthorInherentProvider::new(client.as_ref(), parent_hash, slot)?;

		let payouts = BlockParticipationInherentDataProvider::new(
			client.as_ref(),
			block_participation_data_source.as_ref(),
			parent_hash,
			slot,
			mc_epoch_config,
			config.sc_slot_config.slot_duration,
		)
		.await?;

		let governed_map = GovernedMapInherentDataProvider::new(
			client.as_ref(),
			parent_hash,
			mc_hash.mc_hash(),
			mc_hash.previous_mc_hash(),
			governed_map_data_source.as_ref(),
		)
		.await?;

		let bridge = TokenBridgeInherentDataProvider::new(
			client.as_ref(),
			parent_hash,
			mc_hash.mc_hash(),
			bridge_data_source.as_ref(),
		)
		.await?;

		// Provide ring verifier key when authorities change.
		// Check if the stored key is absent (first block or authority set changed).
		let ring_vk_provider = {
			let has_key = client
				.runtime_api()
				.ring_verifier_key(parent_hash)
				.unwrap_or(None)
				.is_some();
			if has_key {
				RingVerifierKeyProvider::new(None)
			} else {
				let authorities = client.runtime_api().authorities(parent_hash).unwrap_or_default();
				RingVerifierKeyProvider::from_authorities(&authorities)
			}
		};

		Ok((
			slot_idp,
			timestamp,
			mc_hash,
			ariadne_data_provider,
			block_producer_id_provider,
			payouts,
			governed_map,
			bridge,
			ring_vk_provider,
		))
	}
}

#[derive(new)]
pub struct VerifierCIDP<T> {
	config: CreateInherentDataConfig,
	client: Arc<T>,
	mc_hash_data_source: Arc<dyn McHashDataSource + Send + Sync>,
	authority_selection_data_source: Arc<dyn AuthoritySelectionDataSource + Send + Sync>,
	block_participation_data_source: Arc<dyn BlockParticipationDataSource + Send + Sync>,
	governed_map_data_source: Arc<dyn GovernedMapDataSource + Send + Sync>,
	bridge_data_source: Arc<dyn TokenBridgeDataSource<AccountId> + Send + Sync>,
}

impl<T: Send + Sync> CurrentSlotProvider for VerifierCIDP<T> {
	fn slot(&self) -> Slot {
		let timestamp = Timestamp::new(self.config.time_source.get_current_time_millis());
		Slot::from_timestamp(timestamp, self.config.slot_duration())
	}
}

#[async_trait]
impl<T> CreateInherentDataProviders<Block, (Slot, McBlockHash)> for VerifierCIDP<T>
where
	T: ProvideRuntimeApi<Block> + Send + Sync + HeaderBackend<Block>,
	T::Api: SessionValidatorManagementApi<
			Block,
			CommitteeMember<CrossChainPublic, SessionKeys>,
			AuthoritySelectionInputs,
			ScEpochNumber,
		>,
	T::Api: BlockProductionLogApi<Block, CommitteeMember<CrossChainPublic, SessionKeys>>,
	T::Api: BlockParticipationApi<Block, BlockAuthor>,
	T::Api: GovernedMapIDPApi<Block>,
	T::Api: TokenBridgeIDPRuntimeApi<Block>,
{
	type InherentDataProviders = (
		TimestampIDP,
		AriadneIDP,
		BlockAuthorInherentProvider<BlockAuthor>,
		BlockParticipationInherentDataProvider<BlockAuthor, DelegatorKey>,
		GovernedMapInherentDataProvider,
		TokenBridgeInherentDataProvider<AccountId>,
	);

	async fn create_inherent_data_providers(
		&self,
		parent_hash: <Block as BlockT>::Hash,
		(verified_block_slot, mc_hash): (Slot, McBlockHash),
	) -> Result<Self::InherentDataProviders, Box<dyn Error + Send + Sync>> {
		let Self {
			config,
			client,
			mc_hash_data_source,
			authority_selection_data_source,
			block_participation_data_source,
			governed_map_data_source,
			bridge_data_source,
		} = self;
		let CreateInherentDataConfig { mc_epoch_config, sc_slot_config, time_source, .. } = config;

		let timestamp = TimestampIDP::new(Timestamp::new(time_source.get_current_time_millis()));
		let parent_header = client.expect_header(parent_hash)?;
		let parent_slot = slot_from_predigest(&parent_header)?;
		let mc_state_reference = McHashIDP::new_verification(
			parent_header,
			parent_slot,
			verified_block_slot,
			mc_hash.clone(),
			config.slot_duration(),
			mc_hash_data_source.as_ref(),
		)
		.await?;

		let ariadne_data_provider = AriadneIDP::new(
			client.as_ref(),
			sc_slot_config,
			mc_epoch_config,
			parent_hash,
			verified_block_slot,
			authority_selection_data_source.as_ref(),
			mc_state_reference.epoch,
		)
		.await?;

		let block_producer_id_provider =
			BlockAuthorInherentProvider::new(client.as_ref(), parent_hash, verified_block_slot)?;

		let payouts = BlockParticipationInherentDataProvider::new(
			client.as_ref(),
			block_participation_data_source.as_ref(),
			parent_hash,
			verified_block_slot,
			mc_epoch_config,
			config.sc_slot_config.slot_duration,
		)
		.await?;

		let governed_map = GovernedMapInherentDataProvider::new(
			client.as_ref(),
			parent_hash,
			mc_hash.clone(),
			mc_state_reference.previous_mc_hash(),
			governed_map_data_source.as_ref(),
		)
		.await?;

		let bridge = TokenBridgeInherentDataProvider::new(
			client.as_ref(),
			parent_hash,
			mc_hash,
			bridge_data_source.as_ref(),
		)
		.await?;

		Ok((
			timestamp,
			ariadne_data_provider,
			block_producer_id_provider,
			payouts,
			governed_map,
			bridge,
		))
	}
}

/// Extract the slot from a parent header's Safrole pre-digest.
pub fn slot_from_predigest(
	header: &<Block as BlockT>::Header,
) -> Result<Option<Slot>, Box<dyn Error + Send + Sync>> {
	if header.number().is_zero() {
		Ok(None)
	} else {
		Ok(find_pre_digest::<Block>(header).map(|d| d.slot()))
	}
}

#[derive(new, Clone)]
pub(crate) struct CreateInherentDataConfig {
	pub mc_epoch_config: MainchainEpochConfig,
	// TODO ETCM-4079 make sure that this struct can be instantiated only if sidechain epoch duration is divisible by slot_duration
	pub sc_slot_config: ScSlotConfig,
	pub time_source: Arc<dyn TimeSource + Send + Sync>,
}

impl CreateInherentDataConfig {
	pub fn slot_duration(&self) -> SlotDuration {
		self.sc_slot_config.slot_duration
	}
}
