// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>
use core::marker::PhantomData;
use snowbridge_core::operating_mode::IsHalted;
use sp_std::vec::Vec;
use xcm::{
	prelude::{Location, SendError, SendResult, SendXcm, Xcm, XcmHash},
	VersionedLocation, VersionedXcm,
};
use xcm_builder::InspectMessageQueues;

pub struct PausableExporter<Halted, InnerExporter>(PhantomData<(Halted, InnerExporter)>);

impl<Halted: IsHalted, InnerExporter: SendXcm> SendXcm for PausableExporter<Halted, InnerExporter> {
	type Ticket = InnerExporter::Ticket;

	fn validate(
		destination: &mut Option<Location>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		match Halted::is_halted() {
			true => Err(SendError::Transport("router paused")),
			false => InnerExporter::validate(destination, message),
		}
	}

	fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		match Halted::is_halted() {
			true => Err(SendError::Transport("router paused")),
			false => InnerExporter::deliver(ticket),
		}
	}
}

impl<Halted: IsHalted, InnerExporter: SendXcm> InspectMessageQueues
	for PausableExporter<Halted, InnerExporter>
{
	fn clear_messages() {}

	/// This router needs to implement `InspectMessageQueues` but doesn't have to
	/// return any messages, since it just reuses the inner router.
	fn get_messages() -> Vec<(VersionedLocation, Vec<VersionedXcm<()>>)> {
		Vec::new()
	}
}
