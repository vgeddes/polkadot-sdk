// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: 2023 Snowfork <hello@snowfork.com>

use snowbridge_core::reward::{AddTip, AddTipError};

pub struct MockOkInboundQueue;

impl AddTip for MockOkInboundQueue {
	fn add_tip(_nonce: u64, _amount: u128) -> Result<(), AddTipError> {
		Ok(())
	}
}
