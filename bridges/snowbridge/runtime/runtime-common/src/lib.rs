// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>
//! # Runtime Common
//!
//! Common traits and types shared by runtimes.
#![cfg_attr(not(feature = "std"), no_std)]

pub mod fee_handler;
pub mod register_token;

pub use fee_handler::XcmExportFeeToSibling;

pub use register_token::{ForeignAssetOwner, LocalAssetOwner};

#[cfg(test)]
mod tests;
