// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>

extern crate alloc;

use scale_info::TypeInfo;
use sp_runtime::{
    codec::{Decode, Encode},
};
use sp_std::{fmt::Debug, marker::PhantomData};
use bp_relayers::PaymentProcedure;
use xcm::prelude::{ExecuteXcm, Junction::*, Location, SendXcm, *};
use xcm::opaque::latest::prelude::Xcm;
use sp_runtime::DispatchError;
use sp_runtime::traits::Get;
use frame_support::dispatch::GetDispatchInfo;

pub struct NoOpReward;
/// Reward payment procedure that sends a XCM to AssetHub to mint the reward (foreign asset)
/// into the provided beneficiary account.
pub struct PayAccountOnLocation<Relayer, RewardBalance, NoOpReward, EthereumLocation, AssetHubLocation, AssetHubXCMFee, InboundQueueLocation, XcmSender, XcmExecutor, Call>(
    PhantomData<(Relayer, RewardBalance, NoOpReward, EthereumLocation, AssetHubLocation, AssetHubXCMFee, InboundQueueLocation, XcmSender, XcmExecutor, Call)>,
);

impl<Relayer, RewardBalance, NoOpReward, EthereumLocation, AssetHubLocation, AssetHubXCMFee, InboundQueueLocation, XcmSender, XcmExecutor, Call>
PaymentProcedure<Relayer, NoOpReward, RewardBalance>
for PayAccountOnLocation<Relayer, RewardBalance, NoOpReward, EthereumLocation, AssetHubLocation, AssetHubXCMFee, InboundQueueLocation, XcmSender, XcmExecutor, Call>
    where
        Relayer: Clone + Debug + Decode + Encode + Eq + TypeInfo + Into<sp_runtime::AccountId32> + Into<Location>,
        EthereumLocation: Get<Location>,
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
        let reward_unit: u128 = reward.into();
        let reward_asset: Asset = (EthereumLocation::get(), reward_unit).into();
        let fee_asset: Asset = (EthereumLocation::get(), AssetHubXCMFee::get()).into();

        let xcm: Xcm<()> = alloc::vec![
            RefundSurplus,
            ReserveAssetDeposited(reward_asset.clone().into()),
            PayFees { asset: fee_asset },
            DescendOrigin(InboundQueueLocation::get().into()),
            UniversalOrigin(GlobalConsensus(Ethereum { chain_id: 11155111 })),
            DepositAsset {
                assets: AllCounted(1).into(),
                beneficiary
            },
        ]
            .into();

        let (ticket, fee) = validate_send::<XcmSender>(AssetHubLocation::get(), xcm).map_err(|_| DispatchError::Unavailable)?; // TODO fix error
        XcmExecutor::charge_fees(relayer.clone(), fee.clone()).map_err(|_| DispatchError::Unavailable)?; // TODO fix error
        XcmSender::deliver(ticket).map_err(|_| DispatchError::Unavailable)?; // TODO fix error

        Ok(())
    }
}

/// XCM asset descriptor for native ether relative to AH
pub fn ether_asset(network: NetworkId, value: u128) -> Asset {
    (
        Location::new(
            2,
            [
                GlobalConsensus(network),
            ],
        ),
        value
    ).into()
}
