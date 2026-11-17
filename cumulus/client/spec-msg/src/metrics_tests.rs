// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Cumulus.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

use crate::{
	fetch::{fetch_with_metrics, RequestError},
	metrics::FetchKind,
	tests::{
		ack, channel, frontier_at, import, messages, ArchiveTransport, Model, TestArchive, TestPeer,
	},
	worker::read_watermarks,
	Budget, FinalizedRoots, MemoryStore, Metrics, Receiver, SourceWants, Transport,
};
use codec::Encode;
use cumulus_primitives_core::ParaId;
use cumulus_primitives_spec_messaging::{
	ChannelMessage, FetchError, FetchResponse, InChannelId, MessagePosition, MessagesRequest,
	MmrFrontier, Register, RegisterStatus, StreamId, StreamsRoot,
};
use futures::{executor::block_on, FutureExt};
use parking_lot::{Mutex, RwLock};
use prometheus_endpoint::Registry;
use sc_network::PeerId;
use std::{collections::BTreeMap, sync::Arc};

fn metrics() -> (Registry, Metrics) {
	let registry = Registry::new();
	let metrics = Metrics::register(Some(&registry)).expect("fresh registry accepts metrics; qed");
	(registry, metrics)
}

fn sample(registry: &Registry, suffix: &str, labels: &[(&str, &str)]) -> Option<f64> {
	let name = format!("parachain_spec_msg_{suffix}");
	registry
		.gather()
		.iter()
		.find(|family| family.get_name() == name)
		.and_then(|family| {
			family
				.get_metric()
				.iter()
				.find(|metric| {
					metric.get_label().len() == labels.len() &&
						labels.iter().all(|(key, value)| {
							metric.get_label().iter().any(|label| {
								label.get_name() == *key && label.get_value() == *value
							})
						})
				})
				.map(|metric| metric.get_counter().get_value() + metric.get_gauge().get_value())
		})
}

fn delivery(registry: &Registry, sent: u64, confirmed: u64, pending: u64) {
	let labels = [("recipient", "2001")];
	assert_eq!(sample(registry, "messages_sent_total", &labels), Some(sent as f64));
	assert_eq!(sample(registry, "messages_confirmed_total", &labels), Some(confirmed as f64));
	assert_eq!(sample(registry, "messages_pending", &labels), Some(pending as f64));
}

fn no_delivery(registry: &Registry) {
	let families = registry.gather();
	for suffix in [
		"messages_sent_total",
		"messages_confirmed_total",
		"messages_pending",
		"oldest_pending_timestamp_seconds",
	] {
		let name = format!("parachain_spec_msg_{suffix}");
		assert!(!families.iter().any(|family| family.get_name() == name), "{name} is exported");
	}
}

fn data(byte: u8) -> Vec<u8> {
	ChannelMessage::Data(vec![byte]).encode()
}

fn archive(store: Arc<MemoryStore>, metrics: Metrics) -> TestArchive {
	let mut archive = TestArchive::open(store).with_metrics(metrics);
	archive
		.import_block(0, 0, 0, 0, Vec::new(), None)
		.expect("an empty archive accepts genesis; qed");
	archive.observe_delivery(&0, &BTreeMap::new(), 50);
	archive
}

#[test]
fn delivery_counts_only_data_once_across_channels_and_pruning() {
	let (registry, metrics) = metrics();
	let mut archive = archive(Arc::new(MemoryStore::default()), metrics);
	let a = channel(2001);
	let b = StreamId::Channel { recipient: 2001.into(), domain: 0, num: 1 };
	let leaves = vec![
		ChannelMessage::Hello { session: 1, version: 1 }.encode(),
		data(1),
		data(2),
		ChannelMessage::CloseChannel { session: 1 }.encode(),
	];
	import(
		&mut archive,
		&mut Model::default(),
		1,
		vec![(a, leaves), (b, vec![data(3)]), (ack(2001), vec![data(4)])],
	);
	assert_eq!(sample(&registry, "messages_sent_total", &[("recipient", "2001")]), None);
	archive.observe_delivery(&1, &BTreeMap::new(), 100);
	delivery(&registry, 3, 0, 3);
	assert_eq!(
		sample(&registry, "oldest_pending_timestamp_seconds", &[("recipient", "2001")]),
		Some(100.0),
	);

	// Hello and the first Data are confirmed. Retention deletes their payloads afterwards.
	archive
		.retain(1, &BTreeMap::from([(a, 2)]), 0)
		.expect("retention succeeds; qed");
	delivery(&registry, 3, 1, 2);
	for up_to in [2, 1, 2] {
		archive.observe_delivery(&1, &BTreeMap::from([(a, up_to)]), 120);
		delivery(&registry, 3, 1, 2);
	}
	// A watermark past the head is clamped; no synthetic messages enter the numerator.
	archive
		.retain(1, &BTreeMap::from([(a, 99), (b, 1)]), 0)
		.expect("retention succeeds; qed");
	delivery(&registry, 3, 3, 0);
	assert_eq!(
		sample(&registry, "oldest_pending_timestamp_seconds", &[("recipient", "2001")]),
		Some(0.0),
	);
}

#[test]
fn unfinalized_forks_and_repeated_or_skipped_notifications_do_not_inflate_delivery() {
	let (registry, metrics) = metrics();
	let mut archive = archive(Arc::new(MemoryStore::default()), metrics);
	let a = channel(2001);
	let mut model = Model::default();
	import(&mut archive, &mut model, 1, vec![(a, vec![data(1)])]);
	archive.observe_delivery(&1, &BTreeMap::new(), 100);
	let at_fork = model.clone();
	import(&mut archive, &mut model, 2, vec![(a, vec![data(2), data(3)])]);
	delivery(&registry, 1, 0, 1);
	archive.rewind_to(1, 1).expect("block 1 is the archived ancestor; qed");
	model = at_fork;
	for (hash, parent, number, leaves) in [(12, 1, 2, vec![data(4)]), (13, 12, 3, vec![data(5)])] {
		let sends = vec![(a, leaves)];
		let root = model.send(&sends);
		archive
			.import_block(hash, parent, number, 0, sends, Some(root))
			.expect("the fork extends the current tip; qed");
	}
	// Finality jumps over block 12; all of its data still counts once.
	archive.observe_delivery(&13, &BTreeMap::from([(a, 2)]), 150);
	delivery(&registry, 3, 2, 1);
	assert_eq!(
		sample(&registry, "oldest_pending_timestamp_seconds", &[("recipient", "2001")]),
		Some(150.0),
	);
	archive.observe_delivery(&13, &BTreeMap::from([(a, 2)]), 160);
	archive.observe_delivery(&1, &BTreeMap::new(), 170);
	archive.observe_delivery(&13, &BTreeMap::from([(a, 3)]), 180);
	delivery(&registry, 3, 3, 0);
}

#[test]
fn restart_establishes_one_baseline_for_sends_and_confirmations() {
	let store = Arc::new(MemoryStore::default());
	let a = channel(2001);
	let mut model = Model::default();
	let (_, first_metrics) = metrics();
	let mut first = archive(store.clone(), first_metrics);
	import(&mut first, &mut model, 1, vec![(a, vec![data(1), data(2)])]);
	first.retain(1, &BTreeMap::from([(a, 1)]), 0).expect("retention succeeds; qed");
	drop(first);

	let (registry, metrics) = metrics();
	let mut restarted = TestArchive::open(store).with_metrics(metrics);
	no_delivery(&registry);
	restarted.observe_delivery(&1, &BTreeMap::new(), 200);
	delivery(&registry, 0, 0, 0);
	assert_eq!(
		sample(&registry, "oldest_pending_timestamp_seconds", &[("recipient", "2001")]),
		Some(0.0),
	);
	import(&mut restarted, &mut model, 2, vec![(a, vec![data(3), data(4)])]);
	restarted
		.retain(2, &BTreeMap::from([(a, 3)]), 0)
		.expect("retention succeeds; qed");
	// The old pending message is excluded from both counters, even when confirmed now.
	delivery(&registry, 2, 1, 1);
}

#[test]
fn seeded_history_and_later_backfill_do_not_enter_the_delivery_cohort() {
	let (registry, metrics) = metrics();
	let a = channel(2001);
	let old = vec![data(1), data(2)];
	let mut model = Model::default();
	model.send(&[(a, old.clone())]);
	let mut archive = TestArchive::open(Arc::new(MemoryStore::default())).with_metrics(metrics);
	archive
		.seed(10, 9, 10, 0, vec![(a, frontier_at(&old, old.len()))], None)
		.expect("valid peaks seed an empty archive; qed");
	archive.observe_delivery(&10, &BTreeMap::new(), 100);
	import(&mut archive, &mut model, 11, vec![(a, vec![data(3)])]);
	archive.observe_delivery(&11, &BTreeMap::from([(a, 2)]), 110);
	delivery(&registry, 1, 0, 1);
	archive
		.backfill(a, &MmrFrontier::new(), &old)
		.expect("backfill reproduces the seed's peaks; qed");
	archive
		.retain(11, &BTreeMap::from([(a, 3)]), 0)
		.expect("retention succeeds; qed");
	delivery(&registry, 1, 1, 0);
}

#[test]
fn incomplete_delivery_is_withdrawn_until_restart_without_disabling_fetch_metrics() {
	for payload in [None, Some(vec![0xff])] {
		let (registry, instrumentation) = metrics();
		let mut tracker =
			instrumentation.delivery().expect("registered metrics enable delivery; qed");
		let (a, b, c) = (channel(2001), channel(2002), channel(2003));
		let record_fetch = || {
			let mut fetch = instrumentation.fetch(2000.into(), FetchKind::Proof);
			fetch.start_attempt();
			fetch.attempt("success");
			fetch.finish("success");
		};
		no_delivery(&registry);
		tracker.observe([], &BTreeMap::new(), 0, |_, _| None);
		tracker.observe([(a, 2), (b, 1)], &BTreeMap::from([(a, 1)]), 100, |_, _| Some(data(1)));
		delivery(&registry, 2, 1, 1);
		assert_eq!(sample(&registry, "messages_pending", &[("recipient", "2002")]), Some(1.0));
		record_fetch();

		// A failure on the second recipient must also withdraw the first recipient's statistics,
		// including any counters already updated during this observation.
		tracker.observe([(a, 3), (b, 2)], &BTreeMap::from([(a, 2)]), 110, |stream, _| {
			if *stream == b {
				payload.clone()
			} else {
				Some(data(1))
			}
		});
		no_delivery(&registry);
		tracker.observe([(a, 4), (b, 3), (c, 1)], &BTreeMap::new(), 120, |_, _| {
			panic!("withdrawn delivery tracking must not read further payloads")
		});
		no_delivery(&registry);
		record_fetch();
		for metric in ["fetch_requests_total", "fetch_attempts_total"] {
			assert_eq!(fetch_sample(&registry, metric, "proof", "success"), Some(2.0));
		}

		// A restarted process can establish a fresh baseline and count new traffic again.
		let (restarted_registry, restarted_metrics) = metrics();
		let mut restarted =
			restarted_metrics.delivery().expect("registered metrics enable delivery; qed");
		no_delivery(&restarted_registry);
		restarted.observe([(a, 4), (b, 3), (c, 1)], &BTreeMap::new(), 130, |_, _| {
			panic!("establishing a baseline must not read historical payloads")
		});
		delivery(&restarted_registry, 0, 0, 0);
		restarted.observe([(a, 5)], &BTreeMap::from([(a, 5)]), 140, |_, _| Some(data(2)));
		delivery(&restarted_registry, 1, 1, 0);
	}
}

#[test]
fn withdrawn_delivery_metrics_do_not_prevent_retention() {
	let (registry, metrics) = metrics();
	let mut archive = archive(Arc::new(MemoryStore::default()), metrics);
	let stream = channel(2001);
	let mut model = Model::default();
	import(&mut archive, &mut model, 1, vec![(stream, vec![data(1)])]);
	archive.retain(1, &BTreeMap::new(), 0).expect("retention succeeds; qed");
	delivery(&registry, 1, 0, 1);
	for (hash, payload) in [(2, vec![0xff]), (3, data(2))] {
		import(&mut archive, &mut model, hash, vec![(stream, vec![payload])]);
		archive
			.retain(hash, &BTreeMap::from([(stream, hash)]), 0)
			.expect("incomplete metrics must not block retention; qed");
		no_delivery(&registry);
		assert!(archive.unconfirmed_channels(&hash).is_empty());
	}
}

#[test]
fn disabled_metrics_have_no_delivery_tracker() {
	assert!(Metrics::register(None)
		.expect("disabled registration cannot fail; qed")
		.delivery()
		.is_none());
}

struct Replies(Vec<(PeerId, Option<Result<Vec<u8>, String>>)>);

impl Replies {
	fn new(replies: impl IntoIterator<Item = Option<Result<Vec<u8>, String>>>) -> Self {
		Self(replies.into_iter().map(|reply| (PeerId::random(), reply)).collect())
	}
}

#[async_trait::async_trait]
impl Transport for Replies {
	fn peers(&self, _: ParaId) -> Vec<PeerId> {
		self.0.iter().map(|(peer, _)| *peer).collect()
	}

	async fn request(&self, peer: PeerId, _: Vec<u8>) -> Result<Vec<u8>, String> {
		match self.0.iter().find(|(id, _)| *id == peer) {
			Some((_, Some(reply))) => reply.clone(),
			Some((_, None)) => futures::future::pending().await,
			None => Err("unknown test peer".into()),
		}
	}
}

fn request_fixture() -> (MessagesRequest, TestArchive) {
	let mut archive = TestArchive::open(Arc::new(MemoryStore::default()));
	let root = import(&mut archive, &mut Model::default(), 1, vec![(channel(2001), vec![data(1)])]);
	(messages(channel(2001), 0, root, u32::MAX), archive)
}

fn fetch_sample(registry: &Registry, metric: &str, kind: &str, result: &str) -> Option<f64> {
	sample(registry, metric, &[("source", "2000"), ("kind", kind), ("result", result)])
}

#[test]
fn peer_failures_followed_by_success_are_one_successful_logical_fetch() {
	let (registry, metrics) = metrics();
	let (request, archive) = request_fixture();
	let response = archive
		.serve_messages(&request, u32::MAX)
		.expect("fixture serves its root; qed");
	let mut forged = response.clone();
	forged.payloads[0][0] ^= 0xff;
	let replies = Replies::new([
		Some(Err("timeout".into())),
		Some(Ok(vec![0xff])),
		Some(Ok(FetchResponse::Ok(forged).encode())),
		Some(Ok(FetchResponse::Err(FetchError::UnknownRoot).encode())),
		Some(Ok(FetchResponse::Ok(response).encode())),
	]);
	assert!(block_on(fetch_with_metrics(
		&replies,
		2000.into(),
		&request,
		None,
		&metrics,
		FetchKind::Channel { pending: false },
	))
	.is_ok());
	for result in ["transport_error", "decode_error", "invalid_proof", "unknown_root", "success"] {
		assert_eq!(fetch_sample(&registry, "fetch_attempts_total", "channel", result), Some(1.0));
	}
	assert_eq!(fetch_sample(&registry, "fetch_requests_total", "channel", "success"), Some(1.0));
	assert_eq!(fetch_sample(&registry, "fetch_requests_total", "channel", "unserved"), None);
	assert_eq!(sample(&registry, "messages_confirmed_total", &[("recipient", "2001")]), None);
}

#[test]
fn expected_unwritten_streams_and_typed_refusals_remain_distinct() {
	let (registry, metrics) = metrics();
	let (request, _) = request_fixture();
	for (error, result) in [
		(FetchError::UnknownStream, "unknown_stream"),
		(FetchError::UnknownRoot, "unknown_root"),
		(FetchError::OutsideHorizon { lowest: MessagePosition(1) }, "outside_horizon"),
		(FetchError::BeyondHead, "beyond_head"),
	] {
		let replies = Replies::new([Some(Ok(FetchResponse::Err(error).encode()))]);
		assert!(block_on(fetch_with_metrics(
			&replies,
			2000.into(),
			&request,
			None,
			&metrics,
			FetchKind::Channel { pending: false },
		))
		.is_err());
		assert_eq!(fetch_sample(&registry, "fetch_attempts_total", "channel", result), Some(1.0));
	}
	assert_eq!(fetch_sample(&registry, "fetch_requests_total", "channel", "unserved"), Some(4.0));
	assert_eq!(fetch_sample(&registry, "fetch_requests_total", "channel", "success"), Some(0.0));
	let replies = Replies::new([Some(Ok(FetchResponse::Err(FetchError::UnknownStream).encode()))]);
	for kind in [
		FetchKind::Channel { pending: true },
		FetchKind::Register { pending: true },
		FetchKind::Watermark { pending: true },
	] {
		assert!(block_on(fetch_with_metrics(
			&replies,
			2000.into(),
			&request,
			None,
			&metrics,
			kind,
		))
		.is_err());
	}
	for kind in ["channel", "register", "watermark"] {
		assert_eq!(
			fetch_sample(&registry, "fetch_requests_total", kind, "unknown_stream_pending"),
			Some(1.0),
		);
	}
}

#[test]
fn no_peers_and_cancelled_requests_are_visible_without_becoming_deliveries() {
	let (registry, metrics) = metrics();
	let (request, _) = request_fixture();
	assert_eq!(
		block_on(fetch_with_metrics(
			&Replies::new([]),
			2000.into(),
			&request,
			None,
			&metrics,
			FetchKind::Proof,
		)),
		Err(RequestError::NoPeers)
	);
	let replies = Replies::new([None]);
	assert!(fetch_with_metrics(&replies, 2000.into(), &request, None, &metrics, FetchKind::Proof,)
		.now_or_never()
		.is_none());
	assert_eq!(fetch_sample(&registry, "fetch_requests_total", "proof", "no_peers"), Some(1.0));
	assert_eq!(fetch_sample(&registry, "fetch_requests_total", "proof", "cancelled"), Some(1.0));
	assert_eq!(fetch_sample(&registry, "fetch_attempts_total", "proof", "cancelled"), Some(1.0));
	assert_eq!(fetch_sample(&registry, "fetch_attempts_total", "proof", "success"), Some(0.0));
}

#[test]
fn proof_only_success_and_proven_fork_divergence_have_their_own_outcomes() {
	let (registry, metrics) = metrics();
	let (mut request, archive) = request_fixture();
	request.max_bytes = 0;
	request.start = Some(MessagePosition(1));
	let response = archive.serve_messages(&request, 0).expect("fixture serves a proof; qed");
	assert!(response.payloads.is_empty());
	let replies = Replies::new([Some(Ok(FetchResponse::Ok(response).encode()))]);
	assert!(block_on(fetch_with_metrics(
		&replies,
		2000.into(),
		&request,
		None,
		&metrics,
		FetchKind::Proof,
	))
	.is_ok());
	let other = frontier_at(&[data(99)], 1);
	assert_eq!(
		block_on(fetch_with_metrics(
			&replies,
			2000.into(),
			&request,
			Some(&other),
			&metrics,
			FetchKind::Proof,
		)),
		Err(RequestError::Diverged)
	);
	assert_eq!(fetch_sample(&registry, "fetch_requests_total", "proof", "success"), Some(1.0));
	assert_eq!(fetch_sample(&registry, "fetch_requests_total", "proof", "diverged"), Some(1.0));
}

struct FinalizedRoot(StreamsRoot);

#[async_trait::async_trait]
impl FinalizedRoots for FinalizedRoot {
	async fn finalized_included_root(&self, _: ParaId) -> Option<StreamsRoot> {
		Some(self.0)
	}
}

#[test]
fn receiver_fetch_and_verified_watermark_update_their_nodes_metrics() {
	let (sender_registry, sender_metrics) = metrics();
	let store = Arc::new(MemoryStore::default());
	let mut sender = archive(store.clone(), sender_metrics);
	let root = import(
		&mut sender,
		&mut Model::default(),
		1,
		vec![(
			channel(2001),
			vec![ChannelMessage::Hello { session: 1, version: 1 }.encode(), data(1)],
		)],
	);
	sender.observe_delivery(&1, &BTreeMap::new(), 100);
	let (receiver_registry, receiver_metrics) = metrics();
	let peer = |archive| (PeerId::random(), TestPeer { archive: Arc::new(archive), tamper: false });
	let receiver = Receiver::<u64, _>::new(
		2001.into(),
		Budget::default(),
		Arc::new(ArchiveTransport(vec![peer(TestArchive::open(store))])),
	)
	.with_metrics(receiver_metrics);
	let wants = SourceWants {
		channels: BTreeMap::from([(
			InChannelId { sender: 2000.into(), domain: 0, num: 0 },
			MmrFrontier::new(),
		)]),
		..Default::default()
	};
	block_on(receiver.on_root(2000.into(), root, &wants));
	assert_eq!(
		fetch_sample(&receiver_registry, "fetch_requests_total", "channel", "success"),
		Some(1.0),
	);
	// Fetching alone has not confirmed sender delivery.
	delivery(&sender_registry, 1, 0, 1);
	let mut recipient = TestArchive::open(Arc::new(MemoryStore::default()));
	let register = Register { up_to: MessagePosition(2), status: RegisterStatus::Opened };
	let root = import(
		&mut recipient,
		&mut Model::default(),
		1,
		vec![(ack(2000), vec![register.encode()])],
	);
	let transport = ArchiveTransport(vec![peer(recipient)]);
	let sender = RwLock::new(sender);
	let marks = Mutex::new(BTreeMap::new());
	let metrics = sender.read().metrics();
	block_on(read_watermarks(
		&sender,
		&1,
		&marks,
		2000.into(),
		&transport,
		&FinalizedRoot(root),
		&metrics,
	));
	sender.write().retain(1, &marks.lock(), 0).expect("retention succeeds; qed");
	delivery(&sender_registry, 1, 1, 0);
	assert_eq!(
		sample(
			&sender_registry,
			"fetch_requests_total",
			&[("source", "2001"), ("kind", "watermark"), ("result", "success")],
		),
		Some(1.0)
	);
}
