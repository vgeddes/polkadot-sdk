// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Encode, Decode};
use scale_info::TypeInfo;
use frame_support::weights::Weight;
use snowbridge_inbound_queue_primitives::v2::PreparedMessage;
use xcm::{VersionedLocation, VersionedAssets, VersionedXcm, latest::prelude::*};

/// Intermediate parsed message
#[derive(Encode, Decode, Debug, TypeInfo)]
pub struct Message {
	/// Ethereum account that initiated this messaging operation
	pub origin: H160,
	/// The claimer in the case that funds get trapped.
	pub claimer: Option<Location>,
	/// The assets bridged from Ethereum
	pub assets: Vec<Assets>,
	/// The XCM to execute on the destination
	pub remote_xcm: Xcm<()>,
	/// Fee to cover the xcm execution on AH.
	pub execution_fee: Asset,
}

/// Effects of dry-running an inbound message
#[derive(Encode, Decode, Debug, TypeInfo)]
pub struct DryRunEffects {
	/// Execution weight
	pub execution_weight: Weight,
	/// XCM delivery fee
	pub delivery_fee: VersionedAssets,
	/// Queued xcm for sending
	pub forwarded_xcm: (VersionedLocation, VersionedXcm<()>),
}

#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, Debug, TypeInfo)]
pub enum Error {
	/// An API call is unsupported.
	Unimplemented,

	/// Converting a message into XCM failed
	ConversionFailed,

	/// The desired destination was unreachable, generally because
	/// there is a no way of routing to it.
	Unreachable,

	/// There was some other issue (i.e. not to do with routing) in sending the message.
	/// Perhaps a lack of space for buffering the message.
	SendFailure,
}

impl  From<SendError> for Error {
	fn from(e: SendError) -> Self {
		match e {
			SendError::NotApplicable => Error::Unreachable,
			_ => Error::SendFailure,
		}
	}
}

#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, Debug, TypeInfo)]
pub enum AssetTransfer {
	Ether {
		value: u128,
	},
	Token {
		id: TokenId,
		value: u128,
	}
}

#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, Debug, TypeInfo)]
pub enum TokenId {
	Local(H160),
	Foreign(H256)
}

sp_api::decl_runtime_apis! {
	pub trait InboundQueueApiV2
	{
		/// Dry-run a message to determine incurred costs and retrieve forwarded messages
		fn dry_run_submit(message: Message) -> Result<DryRunEffects, Error>;

		/// Convert a set of asset transfer instructions to XCM Assets reanchored relative to the
		/// destination parachain
		fn convert_asset_transfers(Vec<AssetTransfer>) -> Vec<Asset>;
	}
}
