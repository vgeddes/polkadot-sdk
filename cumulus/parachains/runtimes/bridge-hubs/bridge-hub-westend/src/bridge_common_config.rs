// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Cumulus.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Bridge definitions that can be used by multiple BridgeHub flavors.
//! All configurations here should be dedicated to a single chain; in other words, we don't need two
//! chains for a single pallet configuration.
//!
//! For example, the messaging pallet needs to know the sending and receiving chains, but the
//! GRANDPA tracking pallet only needs to be aware of one chain.

use super::{weights, AccountId, Balance, Balances, BlockNumber, Runtime, RuntimeEvent};
use bp_messages::LegacyLaneId;
use frame_support::parameter_types;
use scale_info::TypeInfo;
use xcm::opaque::latest::Location;
use snowbridge_core::reward::NoOpReward;
use crate::bridge_to_ethereum_config::EthereumGlobalLocation;
use crate::bridge_to_ethereum_config::AssetHubXCMFee;
use crate::xcm_config::XcmConfig;
use xcm_executor::XcmExecutor;
use crate::RuntimeCall;
use crate::XcmRouter;
use crate::bridge_to_ethereum_config::InboundQueueLocation;
use testnet_parachains_constants::westend::snowbridge::EthereumLocation;
use crate::bridge_to_ethereum_config::AssetHubLocation;

parameter_types! {
	pub storage RequiredStakeForStakeAndSlash: Balance = 1_000_000;
	pub const RelayerStakeLease: u32 = 8;
	pub const RelayerStakeReserveId: [u8; 8] = *b"brdgrlrs";

impl From<RewardsAccountParams<LegacyLaneId>> for BridgeReward {
	fn from(value: RewardsAccountParams<LegacyLaneId>) -> Self {
		Self::RococoWestend(value)
	}
}

/// Implementation of `bp_relayers::PaymentProcedure` as a pay/claim rewards scheme.
pub struct BridgeRewardPayer;
impl bp_relayers::PaymentProcedure<AccountId, BridgeReward, u128> for BridgeRewardPayer {
	type Error = sp_runtime::DispatchError;
	type AlternativeBeneficiary = Location;

	fn pay_reward(
		relayer: &AccountId,
		reward_kind: BridgeReward,
		reward: u128,
		alternative_beneficiary: Option<Self::AlternativeBeneficiary>,
	) -> Result<(), Self::Error> {
		match reward_kind {
			BridgeReward::RococoWestend(lane_params) => {
				frame_support::ensure!(
					alternative_beneficiary.is_none(),
					Self::Error::Other("`alternative_beneficiary` is not supported for `RococoWestend` rewards!")
				);
				bp_relayers::PayRewardFromAccount::<
					Balances,
					AccountId,
					LegacyLaneId,
					u128,
				>::pay_reward(
					relayer, lane_params, reward, None,
				)
			},
			BridgeReward::Snowbridge => {
				frame_support::ensure!(
					alternative_beneficiary.is_some(),
					Self::Error::Other("`alternative_beneficiary` should be specified for `Snowbridge` rewards!")
				);
				snowbridge_core::reward::PayAccountOnLocation::<
					AccountId,
					u128,
					NoOpReward,
					EthereumLocation,
					//EthereumGlobalLocation,
					AssetHubLocation,
					AssetHubXCMFee,
					InboundQueueLocation,
					XcmRouter,
					XcmExecutor<XcmConfig>,
					RuntimeCall
				>::pay_reward(
					relayer, NoOpReward, reward, alternative_beneficiary
				)
			} //Relayer, RewardBalance, NoOpReward, EthereumLocation, AssetHubLocation, AssetHubXCMFee, InboundQueueLocation, XcmSender, XcmExecutor, Call
		}
	}
	pub storage DeliveryRewardInBalance: u64 = 1_000_000;
}

/// Allows collect and claim rewards for relayers
pub type RelayersForLegacyLaneIdsMessagesInstance = ();
impl pallet_bridge_relayers::Config<RelayersForLegacyLaneIdsMessagesInstance> for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type Reward = Balance;
	type PaymentProcedure = bp_relayers::PayRewardFromAccount<
		pallet_balances::Pallet<Runtime>,
		AccountId,
		Self::LaneId,
	>;
	type StakeAndSlash = pallet_bridge_relayers::StakeAndSlashNamed<
		AccountId,
		BlockNumber,
		Balances,
		RelayerStakeReserveId,
		RequiredStakeForStakeAndSlash,
		RelayerStakeLease,
	>;
	type WeightInfo = weights::pallet_bridge_relayers::WeightInfo<Runtime>;
	type LaneId = LegacyLaneId;
}
