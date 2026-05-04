CREATE INDEX idx_valuations_chain_block_event_asset
ON event_valuations (chain_id, block_number, event_id, asset);

CREATE INDEX idx_events_api_agg_event_time
ON raw_protocol_events (chain_id, event_name, block_timestamp)
INCLUDE (id, asset)
WHERE is_valuable = TRUE AND is_canonical = TRUE;
