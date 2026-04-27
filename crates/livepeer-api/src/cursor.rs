//! Opaque cursor for `(block_number, log_index)` pagination per SPEC §14.3.1.
//!
//! Format: `"B<block>:<log_index>"`. Opaque from the consumer's perspective —
//! they should treat it as a token to round-trip — but readable for humans
//! debugging the API.

use crate::error::ApiError;

#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub block_number: i64,
    pub log_index: i32,
}

impl Cursor {
    pub fn encode(&self) -> String {
        format!("B{}:{}", self.block_number, self.log_index)
    }
    pub fn decode(s: &str) -> Result<Self, ApiError> {
        let stripped = s.strip_prefix('B').ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        let (block, log_index) = stripped
            .split_once(':')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        let block_number: i64 = block.parse().map_err(|_| ApiError::bad_request("invalid cursor block"))?;
        let log_index: i32 = log_index.parse().map_err(|_| ApiError::bad_request("invalid cursor log_index"))?;
        Ok(Self { block_number, log_index })
    }
}
