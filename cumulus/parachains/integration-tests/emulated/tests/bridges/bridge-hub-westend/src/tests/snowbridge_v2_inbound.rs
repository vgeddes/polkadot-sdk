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
	imports::*,
	tests::snowbridge_common::{
		erc20_token_location, eth_location, register_foreign_asset, set_up_eth_and_dot_pool,
		set_up_eth_and_dot_pool_on_penpal, snowbridge_sovereign, weth_location,
	},
};
use asset_hub_westend_runtime::ForeignAssets;
use bridge_hub_westend_runtime::{
	bridge_to_ethereum_config::{CreateAssetCall, CreateAssetDeposit, EthereumGatewayAddress},
	EthereumInboundQueueV2,
};
use codec::Encode;
use emulated_integration_tests_common::{RESERVABLE_ASSET_ID, WETH};
use hex_literal::hex;
use rococo_westend_system_emulated_network::penpal_emulated_chain::PARA_ID_B;
use snowbridge_core::{AssetMetadata, TokenIdOf};
use snowbridge_inbound_queue_primitives::{
	v2::{
		EthereumAsset::{ForeignTokenERC20, NativeTokenERC20},
		Message,
	},
	EthereumLocationsConverterFor,
};
use sp_core::{H160, H256};
use sp_runtime::MultiAddress;
use xcm::opaque::latest::AssetTransferFilter::ReserveDeposit;
use xcm_executor::traits::ConvertLocation;
use snowbridge_inbound_queue_primitives::v2::XcmCommand;

const TOKEN_AMOUNT: u128 = 100_000_000_000;

/// Calculates the XCM prologue fee for sending an XCM to AH.
const INITIAL_FUND: u128 = 5_000_000_000_000;

/// An ERC-20 token to be registered and sent.
const TOKEN_ID: [u8; 20] = hex!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
const CHAIN_ID: u64 = 11155111u64;

#[test]
fn register_token_old_v2() {
	let relayer = BridgeHubWestendSender::get();
	let receiver = AssetHubWestendReceiver::get();
	BridgeHubWestend::fund_accounts(vec![(relayer.clone(), INITIAL_FUND)]);
	AssetHubWestend::fund_accounts(vec![(snowbridge_sovereign(), INITIAL_FUND)]);

	set_up_eth_and_dot_pool();

	let claimer = Location::new(0, AccountId32 { network: None, id: receiver.clone().into() });
	let claimer_bytes = claimer.encode();

	let relayer_location =
		Location::new(0, AccountId32 { network: None, id: relayer.clone().into() });

	let bridge_owner = EthereumLocationsConverterFor::<[u8; 32]>::from_chain_id(&CHAIN_ID);

	let token: H160 = TOKEN_ID.into();
	let asset_id = erc20_token_location(token.into());

	let dot_asset = Location::new(1, Here);
	let dot_fee: xcm::prelude::Asset = (dot_asset, CreateAssetDeposit::get()).into();

	let eth_asset_value = 9_000_000_000_000u128;
	let asset_deposit: xcm::prelude::Asset = (eth_location(), eth_asset_value).into();

	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		let instructions = vec![
			// Exchange eth for dot to pay the asset creation deposit
			ExchangeAsset {
				give: asset_deposit.clone().into(),
				want: dot_fee.clone().into(),
				maximal: false,
			},
			// Deposit the dot deposit into the bridge sovereign account (where the asset creation
			// fee will be deducted from)
			DepositAsset { assets: dot_fee.into(), beneficiary: bridge_owner.into() },
			// Call to create the asset.
			Transact {
				origin_kind: OriginKind::Xcm,
				fallback_max_weight: None,
				call: (
					CreateAssetCall::get(),
					asset_id,
					MultiAddress::<[u8; 32], ()>::Id(bridge_owner.into()),
					1u128,
				)
					.encode()
					.into(),
			},
			ExpectTransactStatus(MaybeErrorCode::Success),
			RefundSurplus,
			DepositAsset { assets: Wild(All), beneficiary: claimer.into() },
		];
		let xcm: Xcm<()> = instructions.into();
		let versioned_message_xcm = VersionedXcm::V5(xcm);
		let origin = EthereumGatewayAddress::get();

		let encoded_xcm = versioned_message_xcm.encode();

		let message = Message {
			gateway: origin,
			nonce: 1,
			origin,
			assets: vec![],
			xcm: XcmCommand::Raw(encoded_xcm),
			claimer: Some(claimer_bytes),
			// Used to pay the asset creation deposit.
			value: 9_000_000_000_000u128,
			execution_fee: 1_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubWestend,
			vec![
				// message processed successfully
				RuntimeEvent::MessageQueue(
					pallet_message_queue::Event::Processed { success: true, .. }
				) => {},
				// Check that the token was created as a foreign asset on AssetHub
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Created { asset_id, owner, .. }) => {
					asset_id: *asset_id == erc20_token_location(token),
					owner: *owner == snowbridge_sovereign(),
				},
				// Check that excess fees were paid to the claimer
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == eth_location(),
					owner: *owner == receiver.clone().into(),
				},
			]
		);

		let events = AssetHubWestend::events();
		// Check that no assets were trapped
		assert!(
			!events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped, should not happen."
		);
	});
}
#[test]
fn register_token_v2() {
	let relayer = BridgeHubWestendSender::get();
	let receiver = AssetHubWestendReceiver::get();
	BridgeHubWestend::fund_accounts(vec![(relayer.clone(), INITIAL_FUND)]);
	AssetHubWestend::fund_accounts(vec![(snowbridge_sovereign(), INITIAL_FUND)]);

	set_up_eth_and_dot_pool();

	let claimer = Location::new(0, AccountId32 { network: None, id: receiver.clone().into() });
	let claimer_bytes = claimer.encode();

	let bridge_owner = EthereumLocationsConverterFor::<[u8; 32]>::from_chain_id(&CHAIN_ID);

	let token: H160 = TOKEN_ID.into();
	let asset_id = erc20_token_location(token.into());

	let dot_asset = Location::new(1, Here);
	let dot_fee: xcm::prelude::Asset = (dot_asset, CreateAssetDeposit::get()).into();

	let eth_asset_value = 9_000_000_000_000u128;
	let asset_deposit: xcm::prelude::Asset = (eth_location(), eth_asset_value).into();

	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		let origin = EthereumGatewayAddress::get();

		let message = Message {
			gateway: origin,
			nonce: 1,
			origin,
			assets: vec![],
			xcm: XcmCommand::TokenRegistration { token, network: 0 },
			claimer: Some(claimer_bytes),
			// Used to pay the asset creation deposit.
			value: 9_000_000_000_000u128,
			execution_fee: 1_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubWestend,
			vec![
				// message processed successfully
				RuntimeEvent::MessageQueue(
					pallet_message_queue::Event::Processed { success: true, .. }
				) => {},
				// Check that the token was created as a foreign asset on AssetHub
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Created { asset_id, owner, .. }) => {
					asset_id: *asset_id == erc20_token_location(token),
					owner: *owner == snowbridge_sovereign(),
				},
				// Check that excess fees were paid to the claimer
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == eth_location(),
					owner: *owner == receiver.clone().into(),
				},
			]
		);

		let events = AssetHubWestend::events();
		// Check that no assets were trapped
		assert!(
			!events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped, should not happen."
		);
	});
}

#[test]
fn send_token_v2() {
	let relayer = BridgeHubWestendSender::get();

	let token: H160 = TOKEN_ID.into();
	let token_location = erc20_token_location(token);

	let receiver = AssetHubWestendReceiver::get();
	let claimer = Location::new(0, AccountId32 { network: None, id: receiver.clone().into() });
	let claimer_bytes = claimer.encode();

	let beneficiary_acc_id: H256 = H256::random();
	let beneficiary_acc_bytes: [u8; 32] = beneficiary_acc_id.into();
	let beneficiary =
		Location::new(0, AccountId32 { network: None, id: beneficiary_acc_id.into() });

	register_foreign_asset(token_location.clone());

	let token_transfer_value = 2_000_000_000_000u128;

	let assets = vec![
		// the token being transferred
		NativeTokenERC20 { token_id: token.into(), value: token_transfer_value },
	];

	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		let instructions = vec![DepositAsset {
			assets: Wild(AllOf {
				id: AssetId(token_location.clone()),
				fun: WildFungibility::Fungible,
			}),
			beneficiary,
		}];
		let xcm: Xcm<()> = instructions.into();
		let versioned_message_xcm = VersionedXcm::V5(xcm);
		let origin = H160::random();

		let message = Message {
			gateway: EthereumGatewayAddress::get(),
			nonce: 1,
			origin,
			assets,
			xcm: XcmCommand::Raw(versioned_message_xcm.encode()),
			claimer: Some(claimer_bytes),
			value: 1_500_000_000_000u128,
			execution_fee: 1_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubWestend,
			vec![
				// message processed successfully
				RuntimeEvent::MessageQueue(
					pallet_message_queue::Event::Processed { success: true, .. }
				) => {},
				// Check that the token was received and issued as a foreign asset on AssetHub
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == token_location,
					owner: *owner == beneficiary_acc_bytes.into(),
				},
				// Check that excess fees were paid to the claimer
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == eth_location(),
					owner: *owner == receiver.clone().into(),
				},
			]
		);

		// Beneficiary received the token transfer value
		assert_eq!(
			ForeignAssets::balance(token_location, AccountId::from(beneficiary_acc_bytes)),
			token_transfer_value
		);
		// Claimer received eth refund for fees paid
		assert!(ForeignAssets::balance(eth_location(), receiver) > 0);

		let events = AssetHubWestend::events();
		// Check that no assets were trapped
		assert!(
			!events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped, should not happen."
		);
	});
}

#[test]
fn send_weth_v2() {
	let relayer = BridgeHubWestendSender::get();

	let beneficiary_acc_id: H256 = H256::random();
	let beneficiary_acc_bytes: [u8; 32] = beneficiary_acc_id.into();
	let beneficiary =
		Location::new(0, AccountId32 { network: None, id: beneficiary_acc_id.into() });

	let claimer_acc_id = H256::random();
	let claimer_acc_id_bytes: [u8; 32] = claimer_acc_id.into();
	let claimer = Location::new(0, AccountId32 { network: None, id: claimer_acc_id.into() });
	let claimer_bytes = claimer.encode();

	let token_transfer_value = 2_000_000_000_000u128;

	let assets = vec![
		// the token being transferred
		NativeTokenERC20 { token_id: WETH.into(), value: token_transfer_value },
	];

	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		let instructions = vec![DepositAsset {
			assets: Wild(AllOf {
				id: AssetId(weth_location().clone()),
				fun: WildFungibility::Fungible,
			}),
			beneficiary,
		}];
		let xcm: Xcm<()> = instructions.into();
		let versioned_message_xcm = VersionedXcm::V5(xcm);
		let origin = EthereumGatewayAddress::get();

		let message = Message {
			gateway: origin,
			nonce: 1,
			origin,
			assets,
			xcm: XcmCommand::Raw(versioned_message_xcm.encode()),
			claimer: Some(claimer_bytes),
			value: 3_500_000_000_000u128,
			execution_fee: 1_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubWestend,
			vec![
				// message processed successfully
				RuntimeEvent::MessageQueue(
					pallet_message_queue::Event::Processed { success: true, .. }
				) => {},
				// Check that the token was received and issued as a foreign asset on AssetHub
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == weth_location(),
					owner: *owner == beneficiary_acc_bytes.into(),
				},
				// Check that excess fees were paid to the claimer
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == eth_location(),
					owner: *owner == claimer_acc_id_bytes.clone().into(),
				},
			]
		);

		// Beneficiary received the token transfer value
		assert_eq!(
			ForeignAssets::balance(weth_location(), AccountId::from(beneficiary_acc_bytes)),
			token_transfer_value
		);

		// Claimer received eth refund for fees paid
		assert!(ForeignAssets::balance(eth_location(), AccountId::from(claimer_acc_id_bytes)) > 0);

		let events = AssetHubWestend::events();
		// Check that no assets were trapped
		assert!(
			!events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped, should not happen."
		);
	});
}

#[test]
fn register_and_send_multiple_tokens_v2() {
	let relayer = BridgeHubWestendSender::get();

	let token: H160 = TOKEN_ID.into();
	let token_location = erc20_token_location(token);

	let bridge_owner = EthereumLocationsConverterFor::<[u8; 32]>::from_chain_id(&CHAIN_ID);

	let beneficiary_acc_id: H256 = H256::random();
	let beneficiary_acc_bytes: [u8; 32] = beneficiary_acc_id.into();
	let beneficiary =
		Location::new(0, AccountId32 { network: None, id: beneficiary_acc_id.clone().into() });

	// To satisfy ED
	AssetHubWestend::fund_accounts(vec![(
		sp_runtime::AccountId32::from(beneficiary_acc_bytes),
		3_000_000_000_000,
	)]);

	let claimer_acc_id = H256::random();
	let claimer_acc_id_bytes: [u8; 32] = claimer_acc_id.into();
	let claimer = Location::new(0, AccountId32 { network: None, id: claimer_acc_id.into() });
	let claimer_bytes = claimer.encode();

	set_up_eth_and_dot_pool();

	let token_transfer_value = 2_000_000_000_000u128;
	let weth_transfer_value = 2_500_000_000_000u128;

	let dot_asset = Location::new(1, Here);
	let dot_fee: xcm::prelude::Asset = (dot_asset, CreateAssetDeposit::get()).into();

	// Used to pay the asset creation deposit.
	let eth_asset_value = 9_000_000_000_000u128;
	let asset_deposit: xcm::prelude::Asset = (eth_location(), eth_asset_value).into();

	let assets = vec![
		NativeTokenERC20 { token_id: WETH.into(), value: 2_800_000_000_000u128 },
		NativeTokenERC20 { token_id: token.into(), value: token_transfer_value },
	];

	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		let instructions = vec![
			ExchangeAsset {
				give: asset_deposit.clone().into(),
				want: dot_fee.clone().into(),
				maximal: false,
			},
			DepositAsset { assets: dot_fee.into(), beneficiary: bridge_owner.into() },
			// register new token
			Transact {
				origin_kind: OriginKind::Xcm,
				fallback_max_weight: None,
				call: (
					CreateAssetCall::get(),
					token_location.clone(),
					MultiAddress::<[u8; 32], ()>::Id(bridge_owner.into()),
					1u128,
				)
					.encode()
					.into(),
			},
			ExpectTransactStatus(MaybeErrorCode::Success),
			// deposit new token to beneficiary
			DepositAsset {
				assets: Wild(AllOf {
					id: AssetId(token_location.clone()),
					fun: WildFungibility::Fungible,
				}),
				beneficiary: beneficiary.clone(),
			},
			// deposit weth to beneficiary
			DepositAsset {
				assets: Wild(AllOf {
					id: AssetId(weth_location()),
					fun: WildFungibility::Fungible,
				}),
				beneficiary: beneficiary.clone(),
			},
		];
		let xcm: Xcm<()> = instructions.into();
		let versioned_message_xcm = VersionedXcm::V5(xcm);
		let origin = EthereumGatewayAddress::get();

		let message = Message {
			gateway: origin,
			nonce: 1,
			origin,
			assets,
			xcm: XcmCommand::Raw(versioned_message_xcm.encode()),
			claimer: Some(claimer_bytes),
			value: 3_500_000_000_000u128,
			execution_fee: 1_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubWestend,
			vec![
				// message processed successfully
				RuntimeEvent::MessageQueue(
					pallet_message_queue::Event::Processed { success: true, .. }
				) => {},
				// Check that the token was created as a foreign asset on AssetHub
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Created { asset_id, owner, .. }) => {
					asset_id: *asset_id == token_location.clone(),
					owner: *owner == snowbridge_sovereign().into(),
				},
				// Check that the token was received and issued as a foreign asset on AssetHub
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == token_location,
					owner: *owner == beneficiary_acc_bytes.into(),
				},
				// Check that excess fees were paid to the claimer
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == eth_location(),
					owner: *owner == claimer_acc_id_bytes.clone().into(),
				},
			]
		);

		// Beneficiary received the token transfer value
		assert_eq!(
			ForeignAssets::balance(token_location, AccountId::from(beneficiary_acc_bytes)),
			token_transfer_value
		);

		// Beneficiary received the weth transfer value
		assert!(
			ForeignAssets::balance(weth_location(), AccountId::from(beneficiary_acc_bytes)) >
				weth_transfer_value
		);

		let events = AssetHubWestend::events();
		// Check that no assets were trapped
		assert!(
			!events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped, should not happen."
		);

		// Claimer received eth refund for fees paid
		assert!(ForeignAssets::balance(eth_location(), AccountId::from(claimer_acc_id_bytes)) > 0);
	});
}

#[test]
fn send_token_to_penpal_v2() {
	let relayer = BridgeHubWestendSender::get();

	let token: H160 = TOKEN_ID.into();
	let token_location = erc20_token_location(token);

	let beneficiary_acc_id: H256 = H256::random();
	let beneficiary_acc_bytes: [u8; 32] = beneficiary_acc_id.into();
	let beneficiary =
		Location::new(0, AccountId32 { network: None, id: beneficiary_acc_id.into() });

	let claimer_acc_id = H256::random();
	let claimer = AccountId32 { network: None, id: claimer_acc_id.into() };
	let claimer_bytes = claimer.encode();

	// To pay fees on Penpal.
	let eth_fee_penpal_ah: xcm::prelude::Asset = (eth_location(), 3_000_000_000_000u128).into();

	register_foreign_asset(token_location.clone());

	// To satisfy ED
	PenpalB::fund_accounts(vec![(
		sp_runtime::AccountId32::from(beneficiary_acc_bytes),
		3_000_000_000_000,
	)]);

	let snowbridge_sovereign = snowbridge_sovereign();
	PenpalB::execute_with(|| {
		type RuntimeOrigin = <PenpalB as Chain>::RuntimeOrigin;

		// Register token on Penpal
		assert_ok!(<PenpalB as PenpalBPallet>::ForeignAssets::force_create(
			RuntimeOrigin::root(),
			token_location.clone().try_into().unwrap(),
			snowbridge_sovereign.clone().into(),
			true,
			1000,
		));

		assert!(<PenpalB as PenpalBPallet>::ForeignAssets::asset_exists(
			token_location.clone().try_into().unwrap(),
		));

		// Register eth on Penpal
		assert_ok!(<PenpalB as PenpalBPallet>::ForeignAssets::force_create(
			RuntimeOrigin::root(),
			eth_location().try_into().unwrap(),
			snowbridge_sovereign.clone().into(),
			true,
			1000,
		));

		assert!(<PenpalB as PenpalBPallet>::ForeignAssets::asset_exists(
			eth_location().try_into().unwrap(),
		));

		assert_ok!(<PenpalB as Chain>::System::set_storage(
			<PenpalB as Chain>::RuntimeOrigin::root(),
			vec![(
				PenpalCustomizableAssetFromSystemAssetHub::key().to_vec(),
				Location::new(2, [GlobalConsensus(Ethereum { chain_id: CHAIN_ID })]).encode(),
			)],
		));
	});

	set_up_eth_and_dot_pool();
	set_up_eth_and_dot_pool_on_penpal();

	let token_transfer_value = 2_000_000_000_000u128;

	let assets = vec![
		// the token being transferred
		NativeTokenERC20 { token_id: token.into(), value: token_transfer_value },
	];

	let token_asset_ah: xcm::prelude::Asset = (token_location.clone(), token_transfer_value).into();
	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		let instructions = vec![
			// Send message to Penpal
			InitiateTransfer {
				// Penpal
				destination: Location::new(1, [Parachain(PARA_ID_B)]),
				remote_fees: Some(ReserveDeposit(Definite(vec![eth_fee_penpal_ah.clone()].into()))),
				preserve_origin: true,
				assets: vec![ReserveDeposit(Definite(vec![token_asset_ah.clone()].into()))],
				remote_xcm: vec![
					// Refund unspent fees
					RefundSurplus,
					// Deposit assets to beneficiary.
					DepositAsset { assets: Wild(AllCounted(2)), beneficiary: beneficiary.clone() },
					SetTopic(H256::random().into()),
				]
				.into(),
			},
		];
		let xcm: Xcm<()> = instructions.into();
		let versioned_message_xcm = VersionedXcm::V5(xcm);
		let origin = EthereumGatewayAddress::get();

		let message = Message {
			gateway: origin,
			nonce: 1,
			origin,
			assets,
			xcm: XcmCommand::Raw(versioned_message_xcm.encode()),
			claimer: Some(claimer_bytes),
			value: 3_500_000_000_000u128,
			execution_fee: 1_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;
		// Check that the assets were issued on AssetHub
		assert_expected_events!(
			AssetHubWestend,
			vec![
				// Message processed successfully
				RuntimeEvent::MessageQueue(
					pallet_message_queue::Event::Processed { success: true, .. }
				) => {},
				// Token was issued to beneficiary
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == token_location,
					owner: *owner == beneficiary_acc_bytes.into(),
				},
				RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},
			]
		);

		let events = AssetHubWestend::events();
		// Check that no assets were trapped
		assert!(
			!events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped, should not happen."
		);
	});

	PenpalB::execute_with(|| {
		type RuntimeEvent = <PenpalB as Chain>::RuntimeEvent;

		assert_expected_events!(
			PenpalB,
			vec![
				// Message processed successfully
				RuntimeEvent::MessageQueue(
					pallet_message_queue::Event::Processed { success: true, .. }
				) => {},
				// Token was issued to beneficiary
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == token_location,
					owner: *owner == beneficiary_acc_bytes.into(),
				},
				// Leftover fees was deposited to beneficiary
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == eth_location(),
					owner: *owner == beneficiary_acc_bytes.into(),
				},
			]
		);

		// Beneficiary received the token transfer value
		assert_eq!(
			ForeignAssets::balance(token_location, AccountId::from(beneficiary_acc_bytes)),
			token_transfer_value
		);

		let events = PenpalB::events();
		// Check that no assets were trapped
		assert!(
			!events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped on Penpal, should not happen."
		);
	});
}

#[test]
fn send_foreign_erc20_token_back_to_polkadot() {
	let relayer = BridgeHubWestendSender::get();

	let claimer = AccountId32 { network: None, id: H256::random().into() };
	let claimer_bytes = claimer.encode();
	let beneficiary =
		Location::new(0, AccountId32 { network: None, id: AssetHubWestendReceiver::get().into() });

	let asset_id: Location =
		[PalletInstance(ASSETS_PALLET_ID), GeneralIndex(RESERVABLE_ASSET_ID.into())].into();

	let asset_id_in_bh: Location = Location::new(
		1,
		[
			Parachain(AssetHubWestend::para_id().into()),
			PalletInstance(ASSETS_PALLET_ID),
			GeneralIndex(RESERVABLE_ASSET_ID.into()),
		],
	);

	let asset_id_after_reanchored = Location::new(
		1,
		[
			GlobalConsensus(ByGenesis(WESTEND_GENESIS_HASH)),
			Parachain(AssetHubWestend::para_id().into()),
		],
	)
	.appended_with(asset_id.clone().interior)
	.unwrap();

	let ethereum_destination = Location::new(2, [GlobalConsensus(Ethereum { chain_id: CHAIN_ID })]);

	// Register token
	BridgeHubWestend::execute_with(|| {
		type RuntimeOrigin = <BridgeHubWestend as Chain>::RuntimeOrigin;

		assert_ok!(<BridgeHubWestend as BridgeHubWestendPallet>::EthereumSystem::register_token(
			RuntimeOrigin::root(),
			Box::new(VersionedLocation::from(asset_id_in_bh.clone())),
			AssetMetadata {
				name: "ah_asset".as_bytes().to_vec().try_into().unwrap(),
				symbol: "ah_asset".as_bytes().to_vec().try_into().unwrap(),
				decimals: 12,
			},
		));
	});

	let ethereum_sovereign: AccountId =
		EthereumLocationsConverterFor::<[u8; 32]>::convert_location(&ethereum_destination)
			.unwrap()
			.into();
	AssetHubWestend::fund_accounts(vec![(ethereum_sovereign.clone(), INITIAL_FUND)]);

	// Mint the asset into the bridge sovereign account, to mimic locked funds
	AssetHubWestend::mint_asset(
		<AssetHubWestend as Chain>::RuntimeOrigin::signed(AssetHubWestendAssetOwner::get()),
		RESERVABLE_ASSET_ID,
		ethereum_sovereign.clone(),
		TOKEN_AMOUNT,
	);

	let token_id = TokenIdOf::convert_location(&asset_id_after_reanchored).unwrap();

	let assets = vec![
		// the token being transferred
		ForeignTokenERC20 { token_id: token_id.into(), value: TOKEN_AMOUNT },
	];

	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		let instructions =
			vec![RefundSurplus, DepositAsset { assets: Wild(AllCounted(2)), beneficiary }];
		let xcm: Xcm<()> = instructions.into();
		let versioned_message_xcm = VersionedXcm::V5(xcm);
		let origin = EthereumGatewayAddress::get();

		let message = Message {
			gateway: origin,
			nonce: 1,
			origin,
			assets,
			xcm: XcmCommand::Raw(versioned_message_xcm.encode()),
			claimer: Some(claimer_bytes),
			value: 1_500_000_000_000u128,
			execution_fee: 3_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubWestend,
			vec![RuntimeEvent::Assets(pallet_assets::Event::Burned{..}) => {},]
		);

		assert_expected_events!(
			AssetHubWestend,
			vec![
				// Message processed successfully
				RuntimeEvent::MessageQueue(
					pallet_message_queue::Event::Processed { success: true, .. }
				) => {},
				// Check that the native token burnt from some reserved account
				RuntimeEvent::Assets(pallet_assets::Event::Burned { owner, .. }) => {
					owner: *owner == snowbridge_sovereign().into(),
				},
				// Check that the token was minted to beneficiary
				RuntimeEvent::Assets(pallet_assets::Event::Issued { owner, .. }) => {
					owner: *owner == AssetHubWestendReceiver::get(),
				},
			]
		);

		let events = AssetHubWestend::events();
		// Check that no assets were trapped
		assert!(
			!events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped, should not happen."
		);
	});
}

#[test]
fn invalid_xcm_traps_funds_on_ah() {
	let relayer = BridgeHubWestendSender::get();

	let token: H160 = TOKEN_ID.into();
	let claimer = AccountId32 { network: None, id: H256::random().into() };
	let claimer_bytes = claimer.encode();
	let beneficiary_acc_bytes: [u8; 32] = H256::random().into();

	AssetHubWestend::fund_accounts(vec![(
		sp_runtime::AccountId32::from(beneficiary_acc_bytes),
		3_000_000_000_000,
	)]);

	set_up_eth_and_dot_pool();

	let assets = vec![
		// to transfer assets
		NativeTokenERC20 { token_id: WETH.into(), value: 2_800_000_000_000u128 },
		// the token being transferred
		NativeTokenERC20 { token_id: token.into(), value: 2_000_000_000_000u128 },
	];

	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		// invalid xcm
		let instructions = hex!("02806c072d50e2c7cd6821d1f084cbb4");
		let origin = EthereumGatewayAddress::get();

		let message = Message {
			gateway: origin,
			nonce: 1,
			origin,
			assets,
			xcm: XcmCommand::Raw(instructions.to_vec()),
			claimer: Some(claimer_bytes),
			value: 1_500_000_000_000u128,
			execution_fee: 1_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;

		// Assets are trapped
		assert_expected_events!(
			AssetHubWestend,
			vec![RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. }) => {},]
		);
	});
}

#[test]
fn invalid_claimer_does_not_fail_the_message() {
	let relayer = BridgeHubWestendSender::get();

	let beneficiary_acc: [u8; 32] = H256::random().into();
	let beneficiary = Location::new(0, AccountId32 { network: None, id: beneficiary_acc.into() });

	let token_transfer_value = 2_000_000_000_000u128;

	let assets = vec![
		// the token being transferred
		NativeTokenERC20 { token_id: WETH.into(), value: token_transfer_value },
	];

	let origin = H160::random();

	BridgeHubWestend::execute_with(|| {
		type RuntimeEvent = <BridgeHubWestend as Chain>::RuntimeEvent;
		let instructions = vec![DepositAsset {
			assets: Wild(AllOf {
				id: AssetId(weth_location().clone()),
				fun: WildFungibility::Fungible,
			}),
			beneficiary,
		}];
		let xcm: Xcm<()> = instructions.into();
		let versioned_message_xcm = VersionedXcm::V5(xcm);

		let message = Message {
			gateway: EthereumGatewayAddress::get(),
			nonce: 1,
			origin,
			assets,
			xcm: XcmCommand::Raw(versioned_message_xcm.encode()),
			// Set an invalid claimer
			claimer: Some(hex!("2b7ce7bc7e87e4d6619da21487c7a53f").to_vec()),
			value: 1_500_000_000_000u128,
			execution_fee: 1_500_000_000_000u128,
			relayer_fee: 1_500_000_000_000u128,
		};

		EthereumInboundQueueV2::process_message(relayer, message).unwrap();

		assert_expected_events!(
			BridgeHubWestend,
			vec![RuntimeEvent::XcmpQueue(cumulus_pallet_xcmp_queue::Event::XcmpMessageSent { .. }) => {},]
		);
	});

	// Message still processes successfully
	AssetHubWestend::execute_with(|| {
		type RuntimeEvent = <AssetHubWestend as Chain>::RuntimeEvent;

		assert_expected_events!(
			AssetHubWestend,
			vec![
				// Token was issued to beneficiary
				RuntimeEvent::ForeignAssets(pallet_assets::Event::Issued { asset_id, owner, .. }) => {
					asset_id: *asset_id == weth_location(),
					owner: *owner == beneficiary_acc.into(),
				},
			]
		);

		// Beneficiary received the token transfer value
		assert_eq!(
			ForeignAssets::balance(weth_location(), AccountId::from(beneficiary_acc)),
			token_transfer_value
		);

		let events = AssetHubWestend::events();
		// Check that assets were trapped due to the invalid claimer.
		assert!(
			events.iter().any(|event| matches!(
				event,
				RuntimeEvent::PolkadotXcm(pallet_xcm::Event::AssetsTrapped { .. })
			)),
			"Assets were trapped, should not happen."
		);
	});
}
