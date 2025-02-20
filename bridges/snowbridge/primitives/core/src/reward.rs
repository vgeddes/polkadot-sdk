// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>

extern crate alloc;

use crate::reward::RewardPaymentError::{ChargeFeesFailure, XcmDeliveryFailure, XcmSendFailure};
use bp_relayers::PaymentProcedure;
use frame_support::dispatch::GetDispatchInfo;
use scale_info::TypeInfo;
use sp_runtime::{
	codec::{Decode, Encode},
	traits::Get,
	DispatchError,
};
use sp_std::{fmt::Debug, marker::PhantomData};
use xcm::{
	opaque::latest::prelude::Xcm,
	prelude::{ExecuteXcm, Junction::*, Location, SendXcm, *},
};

#[derive(Debug, Encode, Decode)]
pub enum RewardPaymentError {
	XcmSendFailure,
	ChargeFeesFailure,
	XcmDeliveryFailure,
}

impl From<RewardPaymentError> for DispatchError {
	fn from(e: RewardPaymentError) -> DispatchError {
		match e {
			RewardPaymentError::XcmSendFailure => DispatchError::Other("xcm send failure"),
			RewardPaymentError::ChargeFeesFailure => DispatchError::Other("charge fees error"),
			RewardPaymentError::XcmDeliveryFailure => DispatchError::Other("xcm delivery failure"),
		}
	}
}

pub struct NoOpReward;
/// Reward payment procedure that sends a XCM to AssetHub to mint the reward (foreign asset)
/// into the provided beneficiary account.
pub struct PayAccountOnLocation<
	Relayer,
	RewardBalance,
	NoOpReward,
	EthereumNetwork,
	AssetHubLocation,
	AssetHubXCMFee,
	InboundQueueLocation,
	XcmSender,
	XcmExecutor,
	Call,
>(
	PhantomData<(
		Relayer,
		RewardBalance,
		NoOpReward,
		EthereumNetwork,
		AssetHubLocation,
		AssetHubXCMFee,
		InboundQueueLocation,
		XcmSender,
		XcmExecutor,
		Call,
	)>,
);

impl<
		Relayer,
		RewardBalance,
		NoOpReward,
		EthereumNetwork,
		AssetHubLocation,
		AssetHubXCMFee,
		InboundQueueLocation,
		XcmSender,
		XcmExecutor,
		Call,
	> PaymentProcedure<Relayer, NoOpReward, RewardBalance>
	for PayAccountOnLocation<
		Relayer,
		RewardBalance,
		NoOpReward,
		EthereumNetwork,
		AssetHubLocation,
		AssetHubXCMFee,
		InboundQueueLocation,
		XcmSender,
		XcmExecutor,
		Call,
	>
where
	Relayer: Clone
		+ Debug
		+ Decode
		+ Encode
		+ Eq
		+ TypeInfo
		+ Into<sp_runtime::AccountId32>
		+ Into<Location>,
	EthereumNetwork: Get<NetworkId>,
	InboundQueueLocation: Get<InteriorLocation>,
	AssetHubLocation: Get<Location>,
	AssetHubXCMFee: Get<u128>,
	XcmSender: SendXcm,
	RewardBalance: Into<u128>,
	XcmExecutor: ExecuteXcm<Call>,
	Call: Decode + GetDispatchInfo,
{
	type Error = DispatchError;
	type Beneficiary = Location;

	fn pay_reward(
		relayer: &Relayer,
		_reward_kind: NoOpReward,
		reward: RewardBalance,
		beneficiary: Self::Beneficiary,
	) -> Result<(), Self::Error> {
		let ethereum_location = Location::new(2, [GlobalConsensus(EthereumNetwork::get())]);

		let reward_asset: Asset = (ethereum_location.clone(), reward.into()).into();
		let fee_asset: Asset = (ethereum_location, AssetHubXCMFee::get()).into();

		let xcm: Xcm<()> = alloc::vec![
			RefundSurplus,
			ReserveAssetDeposited(reward_asset.clone().into()),
			PayFees { asset: fee_asset },
			DescendOrigin(InboundQueueLocation::get().into()),
			UniversalOrigin(GlobalConsensus(EthereumNetwork::get())),
			DepositAsset { assets: AllCounted(1).into(), beneficiary },
		]
		.into();

		let (ticket, fee) =
			validate_send::<XcmSender>(AssetHubLocation::get(), xcm).map_err(|_| XcmSendFailure)?;
		XcmExecutor::charge_fees(relayer.clone(), fee.clone()).map_err(|_| ChargeFeesFailure)?;
		XcmSender::deliver(ticket).map_err(|_| XcmDeliveryFailure)?;

		Ok(())
	}
}

/// XCM asset descriptor for native ether relative to AH
pub fn ether_asset(network: NetworkId, value: u128) -> Asset {
	(Location::new(2, [GlobalConsensus(network)]), value).into()
}
