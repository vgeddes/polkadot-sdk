// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>

#![cfg_attr(not(feature = "std"), no_std)]

use codec::Encode;
use core::cell::RefCell;
#[cfg(feature = "runtime-benchmarks")]
use frame_support::traits::OriginTrait;
use snowbridge_outbound_queue_primitives::{
	v1::{Fee, Message as MessageV1, SendMessage as SendMessageV1},
	v2::{Message, SendMessage},
	SendMessageFeeProvider,
};
use sp_core::H256;
use xcm::prelude::*;
use xcm_executor::{
	traits::{FeeManager, FeeReason, TransactAsset},
	AssetsInHolding,
};

pub mod xcm_origin;

thread_local! {
	pub static IS_WAIVED: RefCell<Vec<FeeReason>> = RefCell::new(vec![]);
	pub static SENDER_OVERRIDE: RefCell<Option<(
		fn(
			&mut Option<Location>,
			&mut Option<Xcm<()>>,
		) -> Result<(Xcm<()>, Assets), SendError>,
		fn(
			Xcm<()>,
		) -> Result<XcmHash, SendError>,
	)>> = RefCell::new(None);
	pub static CHARGE_FEES_OVERRIDE: RefCell<Option<
		fn(Location, Assets) -> xcm::latest::Result
	>> = RefCell::new(None);
}

#[allow(dead_code)]
pub fn set_fee_waiver(waived: Vec<FeeReason>) {
	IS_WAIVED.with(|l| l.replace(waived));
}

#[allow(dead_code)]
pub fn set_sender_override(
	validate: fn(&mut Option<Location>, &mut Option<Xcm<()>>) -> SendResult<Xcm<()>>,
	deliver: fn(Xcm<()>) -> Result<XcmHash, SendError>,
) {
	SENDER_OVERRIDE.with(|x| x.replace(Some((validate, deliver))));
}

#[allow(dead_code)]
pub fn clear_sender_override() {
	SENDER_OVERRIDE.with(|x| x.replace(None));
}

#[allow(dead_code)]
pub fn set_charge_fees_override(charge_fees: fn(Location, Assets) -> xcm::latest::Result) {
	CHARGE_FEES_OVERRIDE.with(|x| x.replace(Some(charge_fees)));
}

#[allow(dead_code)]
pub fn clear_charge_fees_override() {
	CHARGE_FEES_OVERRIDE.with(|x| x.replace(None));
}

// Mock XCM sender that always succeeds
pub struct MockXcmSender;

impl SendXcm for MockXcmSender {
	type Ticket = Xcm<()>;

	fn validate(
		dest: &mut Option<Location>,
		xcm: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		let r: SendResult<Self::Ticket> = SENDER_OVERRIDE.with(|s| {
			if let Some((ref f, _)) = &*s.borrow() {
				f(dest, xcm)
			} else {
				Ok((xcm.take().unwrap(), Assets::default()))
			}
		});
		r
	}

	fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		let r: Result<XcmHash, SendError> = SENDER_OVERRIDE.with(|s| {
			if let Some((_, ref f)) = &*s.borrow() {
				f(ticket)
			} else {
				let hash = ticket.using_encoded(sp_io::hashing::blake2_256);
				Ok(hash)
			}
		});
		r
	}
}

pub struct SuccessfulTransactor;
impl TransactAsset for SuccessfulTransactor {
	fn can_check_in(_origin: &Location, _what: &Asset, _context: &XcmContext) -> XcmResult {
		Ok(())
	}

	fn can_check_out(_dest: &Location, _what: &Asset, _context: &XcmContext) -> XcmResult {
		Ok(())
	}

	fn deposit_asset(_what: &Asset, _who: &Location, _context: Option<&XcmContext>) -> XcmResult {
		Ok(())
	}

	fn withdraw_asset(
		_what: &Asset,
		_who: &Location,
		_context: Option<&XcmContext>,
	) -> Result<AssetsInHolding, XcmError> {
		Ok(AssetsInHolding::default())
	}

	fn internal_transfer_asset(
		_what: &Asset,
		_from: &Location,
		_to: &Location,
		_context: &XcmContext,
	) -> Result<AssetsInHolding, XcmError> {
		Ok(AssetsInHolding::default())
	}
}

pub enum Weightless {}
impl PreparedMessage for Weightless {
	fn weight_of(&self) -> Weight {
		unreachable!();
	}
}

pub struct MockXcmExecutor;
impl<C> ExecuteXcm<C> for MockXcmExecutor {
	type Prepared = Weightless;
	fn prepare(_: Xcm<C>) -> Result<Self::Prepared, Xcm<C>> {
		unreachable!()
	}
	fn execute(_: impl Into<Location>, _: Self::Prepared, _: &mut XcmHash, _: Weight) -> Outcome {
		unreachable!()
	}
	fn charge_fees(location: impl Into<Location>, assets: Assets) -> xcm::latest::Result {
		let r: xcm::latest::Result = CHARGE_FEES_OVERRIDE.with(|s| {
			if let Some(ref f) = &*s.borrow() {
				f(location.into(), assets)
			} else {
				Ok(())
			}
		});
		r
	}
}

impl FeeManager for MockXcmExecutor {
	fn is_waived(_: Option<&Location>, r: FeeReason) -> bool {
		IS_WAIVED.with(|l| l.borrow().contains(&r))
	}

	fn handle_fee(_: Assets, _: Option<&XcmContext>, _: FeeReason) {}
}

pub struct MockOkOutboundQueue;
impl SendMessage for MockOkOutboundQueue {
	type Ticket = ();

	type Balance = u128;

	fn validate(
		_: &Message,
	) -> Result<(Self::Ticket, Self::Balance), snowbridge_outbound_queue_primitives::SendError> {
		Ok(((), 0))
	}

	fn deliver(_: Self::Ticket) -> Result<H256, snowbridge_outbound_queue_primitives::SendError> {
		Ok(H256::zero())
	}
}

impl SendMessageFeeProvider for MockOkOutboundQueue {
	type Balance = u128;

	fn local_fee() -> Self::Balance {
		0
	}
}

pub struct MockOkOutboundQueueV1;
impl SendMessageV1 for MockOkOutboundQueueV1 {
	type Ticket = ();

	fn validate(
		_: &MessageV1,
	) -> Result<
		(Self::Ticket, Fee<<Self as SendMessageFeeProvider>::Balance>),
		snowbridge_outbound_queue_primitives::SendError,
	> {
		Ok(((), Fee::from((0, 0))))
	}

	fn deliver(_: Self::Ticket) -> Result<H256, snowbridge_outbound_queue_primitives::SendError> {
		Ok(H256::zero())
	}
}

impl SendMessageFeeProvider for MockOkOutboundQueueV1 {
	type Balance = u128;

	fn local_fee() -> Self::Balance {
		0
	}
}

#[cfg(feature = "runtime-benchmarks")]
pub trait BenchmarkHelper<O>
where
	O: OriginTrait,
{
	fn make_xcm_origin(location: Location) -> O;
}
