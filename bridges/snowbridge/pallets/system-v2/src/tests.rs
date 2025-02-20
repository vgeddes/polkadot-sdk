// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>
use crate::{mock::*, *};
use frame_support::assert_ok;
use sp_keyring::sr25519::Keyring;
use xcm::{latest::WESTEND_GENESIS_HASH, prelude::*};

#[test]
fn register_tokens_succeeds() {
	new_test_ext(true).execute_with(|| {
		let origin = make_xcm_origin(Location::new(1, [Parachain(1000)]));
		let versioned_location: VersionedLocation = Location::parent().into();

		assert_ok!(EthereumSystemV2::register_token(
			origin,
			Box::new(versioned_location),
			Default::default(),
			1
		));
	});
}

#[test]
fn agent_id_from_location() {
	new_test_ext(true).execute_with(|| {
		let bob: AccountId = Keyring::Bob.into();
		let origin = Location::new(
			1,
			[
				Parachain(1000),
				AccountId32 {
					network: Some(NetworkId::ByGenesis(WESTEND_GENESIS_HASH)),
					id: bob.into(),
				},
			],
		);
		let agent_id = EthereumSystemV2::location_to_message_origin(&origin).unwrap();
		let expected_agent_id =
			hex_literal::hex!("6be45a5822eca69502b722ee5130497d63f0358edba9f0dde28e94e152a74742")
				.into();
		assert_eq!(agent_id, expected_agent_id);
	});
}
