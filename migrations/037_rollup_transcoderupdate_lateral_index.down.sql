-- Rollback: remove the rollup-targeted TranscoderUpdate lateral lookup index.

DROP INDEX IF EXISTS idx_events_transcoder_update_by_to_address;
