// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Cumulus.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// Cumulus is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Cumulus is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus. If not, see <https://www.gnu.org/licenses/>.

//! The requester side of `/spec-msg/fetch/1`: try the source's peers in turn and accept the
//! first response that verifies under the named root. Some refusals are expected, so no peer is
//! penalized.

use crate::{
	metrics::{FetchKind, Metrics},
	protocol::PROTOCOL_NAME,
	LOG_TARGET,
};
use codec::{DecodeLimit, Encode};
use cumulus_primitives_core::ParaId;
use cumulus_primitives_spec_messaging::{
	verify_channel_messages, verify_messages, FetchError, FetchResponse, MessagesRequest,
	MessagesResponse, MmrFrontier, VerifyError,
};
use sc_network::{IfDisconnected, NetworkRequest, PeerId, ProtocolName};
use std::{collections::BTreeMap, sync::Arc};

/// Decoding depth limit for responses. They hold flat vectors; this only blocks crafted nesting.
const MAX_DECODE_DEPTH: u32 = 16;

/// How a request reaches a source's collators.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
	/// The peers that serve `source`, in the order to try them.
	fn peers(&self, source: ParaId) -> Vec<PeerId>;

	/// Send one encoded request to `peer` and return the encoded response.
	async fn request(&self, peer: PeerId, request: Vec<u8>) -> Result<Vec<u8>, String>;
}

/// [`Transport`] over the node's network, to a static set of peers per source.
pub struct NetworkTransport {
	network: Arc<dyn NetworkRequest + Send + Sync>,
	peers: BTreeMap<ParaId, Vec<PeerId>>,
}

impl NetworkTransport {
	/// Requests for `source` go to `peers[source]`, in order. The network must know how to reach
	/// them, for example as reserved peers.
	pub fn new(
		network: Arc<dyn NetworkRequest + Send + Sync>,
		peers: BTreeMap<ParaId, Vec<PeerId>>,
	) -> Self {
		Self { network, peers }
	}
}

#[async_trait::async_trait]
impl Transport for NetworkTransport {
	fn peers(&self, source: ParaId) -> Vec<PeerId> {
		self.peers.get(&source).cloned().unwrap_or_default()
	}

	async fn request(&self, peer: PeerId, request: Vec<u8>) -> Result<Vec<u8>, String> {
		self.network
			.request(
				peer,
				ProtocolName::from(PROTOCOL_NAME),
				request,
				None,
				IfDisconnected::TryConnect,
			)
			.await
			.map(|(response, _)| response)
			.map_err(|e| e.to_string())
	}
}

/// Why no response was accepted.
#[derive(Debug, PartialEq, Eq)]
pub enum RequestError {
	/// No peer is known for the source.
	NoPeers,
	/// Every peer failed, sent a response that did not verify, or refused. `refusals` holds the
	/// typed refusals, so the caller can tell a pruned range from an unknown root.
	Unserved {
		/// The peers that answered with a [`FetchError`].
		refusals: Vec<(PeerId, FetchError)>,
	},
	/// A response verified under the named root, but from another frontier than the stored one
	/// at the same leaf count: the stream under that root does not run through the stored
	/// frontier. No peer can serve one that does.
	Diverged,
}

/// Send `request` to `source`'s peers until one response verifies. With `stored` (an inbound
/// channel's local frontier) the response must also start from it; one that verifies on its own
/// from another frontier at that leaf count ends the search with [`RequestError::Diverged`].
pub async fn fetch<T: Transport + ?Sized>(
	transport: &T,
	source: ParaId,
	request: &MessagesRequest,
	stored: Option<&MmrFrontier>,
) -> Result<MessagesResponse, RequestError> {
	fetch_with_metrics(transport, source, request, stored, &Metrics::default(), FetchKind::Other)
		.await
}

/// The instrumented requester. A logical fetch has one outcome even when it tries several peers.
pub(crate) async fn fetch_with_metrics<T: Transport + ?Sized>(
	transport: &T,
	source: ParaId,
	request: &MessagesRequest,
	stored: Option<&MmrFrontier>,
	metrics: &Metrics,
	kind: FetchKind,
) -> Result<MessagesResponse, RequestError> {
	let mut observation = metrics.fetch(source, kind);
	let peers = transport.peers(source);
	if peers.is_empty() {
		observation.finish("no_peers");
		return Err(RequestError::NoPeers);
	}
	let encoded = request.encode();
	let mut refusals = Vec::new();
	let mut only_pending = kind.expects_unknown_stream();
	for peer in peers {
		observation.start_attempt();
		let bytes = match transport.request(peer, encoded.clone()).await {
			Ok(bytes) => bytes,
			Err(e) => {
				observation.attempt("transport_error");
				only_pending = false;
				log::debug!(target: LOG_TARGET, "Peer {peer} of {source:?} did not serve: {e}");
				continue;
			},
		};
		let mut input = &bytes[..];
		let response = match FetchResponse::decode_with_depth_limit(MAX_DECODE_DEPTH, &mut input) {
			Ok(response) if input.is_empty() => response,
			_ => {
				observation.attempt("decode_error");
				only_pending = false;
				log::debug!(target: LOG_TARGET, "Peer {peer} of {source:?} sent junk");
				continue;
			},
		};
		let response = match response {
			Ok(response) => response,
			Err(e) => {
				let result = match e {
					FetchError::UnknownStream if kind.expects_unknown_stream() => {
						"unknown_stream_pending"
					},
					FetchError::UnknownStream => "unknown_stream",
					FetchError::UnknownRoot => "unknown_root",
					FetchError::OutsideHorizon { .. } => "outside_horizon",
					FetchError::BeyondHead => "beyond_head",
				};
				observation.attempt(result);
				only_pending &= result == "unknown_stream_pending";
				log::debug!(target: LOG_TARGET, "Peer {peer} of {source:?} refused: {e:?}");
				refusals.push((peer, e));
				continue;
			},
		};
		let verified = match stored {
			Some(stored) => verify_channel_messages(request, &response, stored),
			None => verify_messages(request, &response),
		};
		match verified {
			Ok(()) => {
				observation.attempt("success");
				observation.finish("success");
				return Ok(response);
			},
			Err(VerifyError::FrontierMismatch)
				if stored.is_some_and(|stored| {
					response.frontier.leaf_count() == stored.leaf_count()
				}) && verify_messages(request, &response).is_ok() =>
			{
				observation.attempt("diverged");
				observation.finish("diverged");
				log::debug!(
					target: LOG_TARGET,
					"Peer {peer} of {source:?} proved {:?} under {:?} does not run through the \
					 stored frontier",
					request.stream,
					request.under,
				);
				return Err(RequestError::Diverged);
			},
			Err(e) => {
				observation.attempt("invalid_proof");
				only_pending = false;
				log::debug!(
					target: LOG_TARGET,
					"Response of peer {peer} of {source:?} does not verify: {e:?}",
				);
			},
		}
	}
	observation.finish(if only_pending { "unknown_stream_pending" } else { "unserved" });
	Err(RequestError::Unserved { refusals })
}
