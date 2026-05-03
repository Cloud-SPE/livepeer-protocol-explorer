ALTER TABLE event_valuations
    DROP CONSTRAINT event_valuations_status_check;

DELETE FROM event_valuations
 WHERE status IN (
    'failed_missing_pool',
    'failed_missing_oracle',
    'failed_sequencer_outage'
 );

ALTER TABLE event_valuations
    ALTER COLUMN native_usd_price SET NOT NULL,
    ALTER COLUMN amount_usd SET NOT NULL;

ALTER TABLE event_valuations
    ADD CONSTRAINT event_valuations_status_check
    CHECK (status IN ('priced', 'priced_with_warning'));
