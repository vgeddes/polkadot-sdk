// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>
//! Implements the dry-run API.

use crate::{Pallet, Config, WeightInfo};
use snowbridge_inbound_queue_primitives::v2::{ConvertMessage, Message};
use snowbridge_inbound_queue_v2_runtime_api::{DryRunEffects, Error as DryRunError};
use xcm::{latest::prelude::*, VersionedXcm};

pub fn dry_run_submit<T>(message: Message) -> Result<DryRunEffects, DryRunError>
where
	T: Config,
	Location: From<<T as frame_system::Config>::AccountId>
{
	let dest = Pallet::<T>::asset_hub_location();

	let prepared_message = PreparedMessage {
		origin: message.origin,

	}

	let xcm = T::MessageConverter::convert(message).map_err(|_| DryRunError::ConversionFailed)?;

	let (_, delivery_fee) = validate_send::<T::XcmSender>(dest.clone(), xcm.clone())?;

	let versioned_dest = dest.into();
	let versioned_xcm = VersionedXcm::<()>::from(xcm);

	let effects = DryRunEffects {
		execution_weight: T::WeightInfo::submit(),
		delivery_fee: delivery_fee.into(),
		forwarded_xcm: (versioned_dest, versioned_xcm),
	};

	Ok(effects)
}
