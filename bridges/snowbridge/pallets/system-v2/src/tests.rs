// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>
use crate::{mock::*, *};
use frame_support::{assert_noop, assert_ok};

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
