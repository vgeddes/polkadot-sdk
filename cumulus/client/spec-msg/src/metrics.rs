// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Cumulus.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

//! Speculative messaging Prometheus metrics, prefixed with `parachain_spec_msg_`.
//!
//! Delivery metrics are absent before initialization or, after incomplete history, until restart.
//!
//! Confirmation percentage per node and recipient, excluding idle traffic.
//! Use totals: confirmation rates can exceed send rates while draining backlog.
//!
//! ```promql
//! 100 * parachain_spec_msg_messages_confirmed_total
//!   / (parachain_spec_msg_messages_sent_total > 0)
//! ```
//!
//! Oldest pending age in seconds:
//!
//! ```promql
//! clamp_min(time() - (parachain_spec_msg_oldest_pending_timestamp_seconds > 0), 0)
//! ```
//!
//! Fetch success percentage, excluding expected unwritten-stream polls and idle windows:
//!
//! ```promql
//! 100 * sum without(result) (
//!   rate(parachain_spec_msg_fetch_requests_total{result="success"}[5m]))
//! / (sum without(result) (
//!   rate(parachain_spec_msg_fetch_requests_total{result!="unknown_stream_pending"}[5m])) > 0)
//! ```

use crate::LOG_TARGET;
use codec::DecodeAll;
use cumulus_primitives_core::ParaId;
use cumulus_primitives_spec_messaging::{ChannelMessage, StreamId};
use prometheus_endpoint::{register, CounterVec, GaugeVec, Opts, PrometheusError, Registry, U64};
use std::{
	collections::{BTreeMap, VecDeque},
	sync::Arc,
};

struct Inner {
	sent: CounterVec<U64>,
	confirmed: CounterVec<U64>,
	pending: GaugeVec<U64>,
	oldest_pending: GaugeVec<U64>,
	fetch_requests: CounterVec<U64>,
	fetch_attempts: CounterVec<U64>,
}

/// Shared speculative-messaging metrics. The default is disabled.
#[derive(Clone, Default)]
pub struct Metrics(Option<Arc<Inner>>);

impl Metrics {
	/// Register in the node's registry, or disable instrumentation when it has no registry.
	pub fn register(registry: Option<&Registry>) -> Result<Self, PrometheusError> {
		let Some(registry) = registry else { return Ok(Self::default()) };
		Ok(Self(Some(Arc::new(Inner {
			sent: register(
				CounterVec::new(
					Opts::new(
						"parachain_spec_msg_messages_sent_total",
						"Finalized outbound Data messages observed after this process's baseline.",
					),
					&["recipient"],
				)?,
				registry,
			)?,
			confirmed: register(
				CounterVec::new(
					Opts::new(
						"parachain_spec_msg_messages_confirmed_total",
						"Observed Data messages covered by a finalized receiver watermark.",
					),
					&["recipient"],
				)?,
				registry,
			)?,
			pending: register(
				GaugeVec::new(
					Opts::new(
						"parachain_spec_msg_messages_pending",
						"Observed finalized Data messages awaiting a receiver confirmation.",
					),
					&["recipient"],
				)?,
				registry,
			)?,
			oldest_pending: register(
				GaugeVec::new(
					Opts::new(
						"parachain_spec_msg_oldest_pending_timestamp_seconds",
						"Unix time the oldest pending Data message was observed finalized; zero if none.",
					),
					&["recipient"],
				)?,
				registry,
			)?,
			fetch_requests: register(
				CounterVec::new(
					Opts::new(
						"parachain_spec_msg_fetch_requests_total",
						"Logical fetch outcomes after peer fallback, including cancelled fetches.",
					),
					&["source", "kind", "result"],
				)?,
				registry,
			)?,
			fetch_attempts: register(
				CounterVec::new(
					Opts::new(
						"parachain_spec_msg_fetch_attempts_total",
						"Individual peer fetch attempt outcomes after decoding and verification.",
					),
					&["source", "kind", "result"],
				)?,
				registry,
			)?,
		}))))
	}

	pub(crate) fn delivery(&self) -> Option<DeliveryMetrics> {
		self.0.as_ref().map(|metrics| DeliveryMetrics {
			metrics: metrics.clone(),
			channels: Some(BTreeMap::new()),
			initialized: false,
		})
	}

	pub(crate) fn fetch(&self, source: ParaId, kind: FetchKind) -> FetchMetrics<'_> {
		let observation = FetchMetrics {
			metrics: self.0.as_deref(),
			source: self.0.as_ref().map(|_| source.to_string()).unwrap_or_default(),
			kind: kind.label(),
			result: "cancelled",
			attempt_pending: false,
		};
		if let Some(metrics) = observation.metrics {
			// A failed first fetch must yield a zero success rate, not an absent numerator.
			metrics.fetch_requests.with_label_values(&[
				&observation.source,
				observation.kind,
				"success",
			]);
			metrics.fetch_attempts.with_label_values(&[
				&observation.source,
				observation.kind,
				"success",
			]);
		}
		observation
	}
}

/// The caller knows whether an unwritten stream is expected; the wire request alone does not.
#[derive(Clone, Copy)]
pub(crate) enum FetchKind {
	Channel { pending: bool },
	Register { pending: bool },
	Watermark { pending: bool },
	Proof,
	Gap,
	Backfill,
	Other,
}

impl FetchKind {
	fn label(self) -> &'static str {
		match self {
			Self::Channel { .. } => "channel",
			Self::Register { .. } => "register",
			Self::Watermark { .. } => "watermark",
			Self::Proof => "proof",
			Self::Gap => "gap",
			Self::Backfill => "backfill",
			Self::Other => "other",
		}
	}

	pub(crate) fn expects_unknown_stream(self) -> bool {
		matches!(
			self,
			Self::Channel { pending: true } |
				Self::Register { pending: true } |
				Self::Watermark { pending: true }
		)
	}
}

/// Dropping an in-progress request (e.g. the inherent's deadline) is observable too.
pub(crate) struct FetchMetrics<'a> {
	metrics: Option<&'a Inner>,
	source: String,
	kind: &'static str,
	result: &'static str,
	attempt_pending: bool,
}

impl FetchMetrics<'_> {
	pub(crate) fn start_attempt(&mut self) {
		self.attempt_pending = true;
	}

	pub(crate) fn attempt(&mut self, result: &'static str) {
		self.attempt_pending = false;
		if let Some(metrics) = self.metrics {
			metrics
				.fetch_attempts
				.with_label_values(&[&self.source, self.kind, result])
				.inc();
		}
	}

	pub(crate) fn finish(&mut self, result: &'static str) {
		self.result = result;
	}
}

impl Drop for FetchMetrics<'_> {
	fn drop(&mut self) {
		if self.attempt_pending {
			self.attempt("cancelled");
		}
		if let Some(metrics) = self.metrics {
			metrics
				.fetch_requests
				.with_label_values(&[&self.source, self.kind, self.result])
				.inc();
		}
	}
}

#[derive(Default)]
struct ChannelDelivery {
	/// The next finalized position to inspect. Set to the existing count at startup.
	next: u64,
	/// Only Data positions, with the Unix time they were first observed finalized.
	pending: VecDeque<(u64, u64)>,
}

/// Kept with the archive so payloads are classified before retention deletes them. Only pending
/// positions are retained, not payloads or an ever-growing history of delivered messages.
pub(crate) struct DeliveryMetrics {
	metrics: Arc<Inner>,
	/// None after incomplete history: withdrawn series must stay absent until restart.
	channels: Option<BTreeMap<StreamId, ChannelDelivery>>,
	initialized: bool,
}

impl DeliveryMetrics {
	pub(crate) fn metrics(&self) -> Metrics {
		Metrics(Some(self.metrics.clone()))
	}

	pub(crate) fn observe(
		&mut self,
		counts: impl IntoIterator<Item = (StreamId, u64)>,
		watermarks: &BTreeMap<StreamId, u64>,
		now: u64,
		payload: impl Fn(&StreamId, u64) -> Option<Vec<u8>>,
	) {
		let Some(channels) = &mut self.channels else { return };
		for (stream, count) in counts {
			let StreamId::Channel { recipient, .. } = stream else { continue };
			let recipient = recipient.to_string();
			let labels = [recipient.as_str()];
			let channel = channels.entry(stream).or_default();
			let mut sent = 0;
			if self.initialized {
				for position in channel.next..count {
					let message = payload(&stream, position)
						.and_then(|bytes| ChannelMessage::decode_all(&mut &bytes[..]).ok());
					match message {
						Some(ChannelMessage::Data(_)) => {
							channel.pending.push_back((position, now));
							sent += 1;
						},
						Some(_) => {},
						None => {
							// Withdraw every delivery series; partial counts must not look
							// trustworthy. Metrics never prevent pruning, and fetch metrics remain
							// available.
							self.metrics.sent.reset();
							self.metrics.confirmed.reset();
							self.metrics.pending.reset();
							self.metrics.oldest_pending.reset();
							self.channels = None;
							log::warn!(
								target: LOG_TARGET,
								"Delivery metrics withdrawn until restart: unavailable channel message at \
								 {stream:?} position {position}",
							);
							return;
						},
					}
				}
			}
			channel.next = channel.next.max(count);
			self.metrics.sent.with_label_values(&labels).inc_by(sent);

			let up_to = watermarks.get(&stream).copied().unwrap_or(0).min(count);
			let mut confirmed = 0;
			while channel.pending.front().is_some_and(|(position, _)| *position < up_to) {
				channel.pending.pop_front();
				confirmed += 1;
			}
			self.metrics.confirmed.with_label_values(&labels).inc_by(confirmed);
		}
		self.initialized = true;

		// Several channels may share one recipient. Export their aggregate, including zeros when
		// the last pending message is confirmed.
		let mut recipients = BTreeMap::<ParaId, (u64, Option<u64>)>::new();
		for (stream, channel) in channels {
			let StreamId::Channel { recipient, .. } = stream else { continue };
			let (pending, oldest) = recipients.entry(*recipient).or_default();
			*pending += channel.pending.len() as u64;
			if let Some((_, timestamp)) = channel.pending.front() {
				*oldest = Some(oldest.map_or(*timestamp, |old| old.min(*timestamp)));
			}
		}
		for (recipient, (pending, oldest)) in recipients {
			let label = recipient.to_string();
			self.metrics.pending.with_label_values(&[&label]).set(pending);
			self.metrics
				.oldest_pending
				.with_label_values(&[&label])
				.set(oldest.unwrap_or(0));
		}
	}
}
