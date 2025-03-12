// Copyright (C) Parity Technologies (UK) Ltd.
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

use crate::{
	weights, xcm_config,
	xcm_config::{
		AssetTransactors, LocationToAccountId, TrustBackedAssetsPalletLocation, UniversalLocation,
		XcmConfig,
	},
	AccountId, Assets, ForeignAssets, Runtime, RuntimeEvent,
};
use assets_common::{matching::FromSiblingParachain, AssetIdForTrustBackedAssetsConvert};
use frame_support::{
	dispatch::RawOrigin,
	parameter_types,
	traits::{ContainsPair, EitherOf, EnsureOrigin, EnsureOriginWithArg, Everything, OriginTrait},
};
use frame_system::{ensure_signed, EnsureRootWithSuccess};
use pallet_xcm::{EnsureXcm, Origin as XcmOrigin};
use parachains_common::AssetIdForTrustBackedAssets;
use sp_runtime::traits::{MaybeEquivalence, TryConvert};
use testnet_parachains_constants::westend::snowbridge::EthereumNetwork;
use xcm::prelude::{Asset, InteriorLocation, Location, PalletInstance, Parachain};
use xcm_executor::XcmExecutor;

#[cfg(not(feature = "runtime-benchmarks"))]
use crate::xcm_config::XcmRouter;
#[cfg(feature = "runtime-benchmarks")]
use benchmark_helpers::DoNothingRouter;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmark_helpers {
	use crate::RuntimeOrigin;
	use codec::Encode;
	use xcm::prelude::*;

	pub struct DoNothingRouter;
	impl SendXcm for DoNothingRouter {
		type Ticket = Xcm<()>;

		fn validate(
			_dest: &mut Option<Location>,
			xcm: &mut Option<Xcm<()>>,
		) -> SendResult<Self::Ticket> {
			Ok((xcm.clone().unwrap(), Assets::new()))
		}
		fn deliver(xcm: Xcm<()>) -> Result<XcmHash, SendError> {
			let hash = xcm.using_encoded(sp_io::hashing::blake2_256);
			Ok(hash)
		}
	}

	impl snowbridge_pallet_system_frontend::BenchmarkHelper<RuntimeOrigin> for () {
		fn make_xcm_origin(location: Location) -> RuntimeOrigin {
			RuntimeOrigin::from(pallet_xcm::Origin::Xcm(location))
		}
	}
}

parameter_types! {
	pub storage FeeAsset: Location = Location::new(
			2,
			[
				EthereumNetwork::get().into(),
			],
	);
	pub storage DeliveryFee: Asset = (Location::parent(), 80_000_000_000u128).into();
	pub BridgeHubLocation: Location = Location::new(1,[Parachain(westend_runtime_constants::system_parachain::BRIDGE_HUB_ID)]);
	pub SystemFrontendPalletLocation: InteriorLocation = [PalletInstance(80)].into();
	pub const RootLocation: Location = Location::here();
}

impl snowbridge_pallet_system_frontend::Config for Runtime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = weights::snowbridge_pallet_system_frontend::WeightInfo<Runtime>;
	#[cfg(feature = "runtime-benchmarks")]
	type Helper = ();
	type RegisterTokenOrigin = EitherOf<
		EitherOf<
			LocalAssetCreatorAsOwner<
				AssetIdForTrustBackedAssetsConvert<TrustBackedAssetsPalletLocation, Location>,
				Assets,
				AccountId,
				AssetIdForTrustBackedAssets,
				xcm_builder::AliasesIntoAccountId32<xcm_config::RelayNetwork, AccountId>,
				Location,
			>,
			ForeignAssetCreatorAsOwner<
				(
					FromSiblingParachain<parachain_info::Pallet<Runtime>, Location>,
					xcm_config::bridging::to_rococo::RococoAssetFromAssetHubRococo,
				),
				ForeignAssets,
				AccountId,
				LocationToAccountId,
				Location,
			>,
		>,
		EnsureRootWithSuccess<AccountId, RootLocation>,
	>;
	#[cfg(not(feature = "runtime-benchmarks"))]
	type XcmSender = XcmRouter;
	#[cfg(feature = "runtime-benchmarks")]
	type XcmSender = DoNothingRouter;
	type AssetTransactor = AssetTransactors;
	type EthereumLocation = FeeAsset;
	type XcmExecutor = XcmExecutor<XcmConfig>;
	type BridgeHubLocation = BridgeHubLocation;
	type UniversalLocation = UniversalLocation;
	type PalletLocation = SystemFrontendPalletLocation;
	type BackendWeightInfo = weights::snowbridge_pallet_system_backend::WeightInfo<Runtime>;
}

/// `EnsureOriginWithArg` impl for `ForeignAssetCreatorAsOwner` that
/// a. allows only XCM origins that are locations containing the class location.
/// b. check the asset already exists
/// c. only the owner of the asset can create
pub struct ForeignAssetCreatorAsOwner<
	IsForeign,
	AssetInspect,
	AccountId,
	LocationToAccountId,
	L = Location,
>(core::marker::PhantomData<(IsForeign, AssetInspect, AccountId, LocationToAccountId, L)>);
impl<
		IsForeign: ContainsPair<L, L>,
		AssetInspect: frame_support::traits::fungibles::roles::Inspect<AccountId>,
		AccountId: Eq + Clone,
		LocationToAccountId: xcm_executor::traits::ConvertLocation<AccountId>,
		RuntimeOrigin: From<XcmOrigin> + OriginTrait + Clone,
		L: From<Location> + Into<Location> + Clone,
	> EnsureOriginWithArg<RuntimeOrigin, L>
	for ForeignAssetCreatorAsOwner<IsForeign, AssetInspect, AccountId, LocationToAccountId, L>
where
	RuntimeOrigin::PalletsOrigin:
		From<XcmOrigin> + TryInto<XcmOrigin, Error = RuntimeOrigin::PalletsOrigin>,
	<AssetInspect as frame_support::traits::fungibles::Inspect<AccountId>>::AssetId: From<Location>,
{
	type Success = L;

	fn try_origin(
		origin: RuntimeOrigin,
		asset_location: &L,
	) -> Result<Self::Success, RuntimeOrigin> {
		let origin_location = EnsureXcm::<Everything, L>::try_origin(origin.clone())?;
		if !IsForeign::contains(asset_location, &origin_location) {
			return Err(origin)
		}
		let asset_location: Location = asset_location.clone().into();
		let owner = AssetInspect::owner(asset_location.into());
		let location: Location = origin_location.clone().into();
		let from = LocationToAccountId::convert_location(&location);
		if !owner.eq(&from) {
			return Err(origin)
		}
		let latest_location: Location =
			origin_location.clone().try_into().map_err(|_| origin.clone())?;
		Ok(latest_location.into())
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin(a: &L) -> Result<RuntimeOrigin, ()> {
		let latest_location: Location = (*a).clone().try_into().map_err(|_| ())?;
		Ok(pallet_xcm::Origin::Xcm(latest_location).into())
	}
}

/// `EnsureOriginWithArg` impl for `LocalAssetCreatorAsOwner` that
/// a. allows signed origins
/// b. check the asset already exists
/// c. only the owner of the asset can create
pub struct LocalAssetCreatorAsOwner<
	MatchAssetId,
	AssetInspect,
	AccountId,
	AssetId,
	AccountToLocation,
	L = Location,
>(
	core::marker::PhantomData<(
		MatchAssetId,
		AssetInspect,
		AccountId,
		AssetId,
		AccountToLocation,
		L,
	)>,
);
impl<
		MatchAssetId: MaybeEquivalence<L, AssetId>,
		AssetInspect: frame_support::traits::fungibles::roles::Inspect<AccountId>,
		AccountId: Eq + Clone,
		AssetId: Eq + Clone,
		AccountToLocation: for<'a> TryConvert<&'a AccountId, Location>,
		RuntimeOrigin: OriginTrait + Clone,
		L: From<Location> + Into<Location> + Clone,
	> EnsureOriginWithArg<RuntimeOrigin, L>
	for LocalAssetCreatorAsOwner<MatchAssetId, AssetInspect, AccountId, AssetId, AccountToLocation, L>
where
	RuntimeOrigin: Into<Result<RawOrigin<AccountId>, RuntimeOrigin>> + From<RawOrigin<AccountId>>,
	<AssetInspect as frame_support::traits::fungibles::Inspect<AccountId>>::AssetId: From<AssetId>,
{
	type Success = L;

	fn try_origin(
		origin: RuntimeOrigin,
		asset_location: &L,
	) -> Result<Self::Success, RuntimeOrigin> {
		let who = ensure_signed(origin.clone()).map_err(|_| origin.clone())?;
		let asset_id = MatchAssetId::convert(asset_location).ok_or(origin.clone())?;
		let owner = AssetInspect::owner(asset_id.into()).ok_or(origin.clone())?;
		if !owner.eq(&who) {
			return Err(origin)
		}
		let latest_location: Location =
			AccountToLocation::try_convert(&who).map_err(|_| origin.clone())?;
		Ok(latest_location.into())
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin(_: &L) -> Result<RuntimeOrigin, ()> {
		Ok(RawOrigin::Root.into())
	}
}
