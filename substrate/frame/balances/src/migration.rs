// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Substrate.
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

use super::*;
use frame_support::{
	pallet_prelude::*,
	traits::{OnRuntimeUpgrade, PalletInfoAccess},
	weights::Weight,
};

fn migrate_v0_to_v1<T: Config<I>, I: 'static>(accounts: &[T::AccountId]) -> Weight {
	let on_chain_version = Pallet::<T, I>::on_chain_storage_version();

	if on_chain_version == 0 {
		let total = accounts
			.iter()
			.map(|a| Pallet::<T, I>::total_balance(a))
			.fold(T::Balance::zero(), |a, e| a.saturating_add(e));
		Pallet::<T, I>::deactivate(total);

		// Remove the old `StorageVersion` type.
		frame_support::storage::unhashed::kill(&frame_support::storage::storage_prefix(
			Pallet::<T, I>::name().as_bytes(),
			"StorageVersion".as_bytes(),
		));

		// Set storage version to `1`.
		StorageVersion::new(1).put::<Pallet<T, I>>();

		log::info!(target: LOG_TARGET, "Storage to version 1");
		T::DbWeight::get().reads_writes(2 + accounts.len() as u64, 3)
	} else {
		log::info!(
			target: LOG_TARGET,
			"Migration did not execute. This probably should be removed"
		);
		T::DbWeight::get().reads(1)
	}
}

// NOTE: This must be used alongside the account whose balance is expected to be inactive.
// Generally this will be used for the XCM teleport checking account.
pub struct MigrateToTrackInactive<T, A, I = ()>(PhantomData<(T, A, I)>);
impl<T: Config<I>, A: Get<T::AccountId>, I: 'static> OnRuntimeUpgrade
	for MigrateToTrackInactive<T, A, I>
{
	fn on_runtime_upgrade() -> Weight {
		migrate_v0_to_v1::<T, I>(&[A::get()])
	}
}

// NOTE: This must be used alongside the accounts whose balance is expected to be inactive.
// Generally this will be used for the XCM teleport checking accounts.
pub struct MigrateManyToTrackInactive<T, A, I = ()>(PhantomData<(T, A, I)>);
impl<T: Config<I>, A: Get<Vec<T::AccountId>>, I: 'static> OnRuntimeUpgrade
	for MigrateManyToTrackInactive<T, A, I>
{
	fn on_runtime_upgrade() -> Weight {
		migrate_v0_to_v1::<T, I>(&A::get())
	}
}

pub struct ResetInactive<T, I = ()>(PhantomData<(T, I)>);
impl<T: Config<I>, I: 'static> OnRuntimeUpgrade for ResetInactive<T, I> {
	fn on_runtime_upgrade() -> Weight {
		let on_chain_version = Pallet::<T, I>::on_chain_storage_version();

		if on_chain_version == 1 {
			// Remove the old `StorageVersion` type.
			frame_support::storage::unhashed::kill(&frame_support::storage::storage_prefix(
				Pallet::<T, I>::name().as_bytes(),
				"StorageVersion".as_bytes(),
			));

			InactiveIssuance::<T, I>::kill();

			// Set storage version to `0`.
			StorageVersion::new(0).put::<Pallet<T, I>>();

			log::info!(target: LOG_TARGET, "Storage to version 0");
			T::DbWeight::get().reads_writes(1, 3)
		} else {
			log::info!(
				target: LOG_TARGET,
				"Migration did not execute. This probably should be removed"
			);
			T::DbWeight::get().reads(1)
		}
	}
}
