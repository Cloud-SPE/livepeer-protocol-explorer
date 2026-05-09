-- 028_create_broadcaster_classifications
-- Phase 0 TD-017 foundation table for operator-managed broadcaster kind overlays.
-- Seeded from the legacy livepeer-backend-rs AI broadcaster list
-- (src/lib.rs + config.toml, verified 2026-05-05).

CREATE TABLE broadcaster_classifications (
    chain_id    BIGINT NOT NULL,
    address     TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('ai', 'transcoding')),
    source      TEXT NOT NULL,
    notes       TEXT,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (chain_id, address)
);

INSERT INTO broadcaster_classifications (
    chain_id,
    address,
    kind,
    source,
    notes
)
VALUES
    (42161, '0x62878278bfa59224f72ed94e9e33f289650d7fb3', 'ai', 'seed', 'flipguard.'),
    (42161, '0x2752513f699fd713b8f4ce8538e560be12b416b0', 'ai', 'seed', 'PapaBear AI'),
    (42161, '0x012345de92b630c065dfc0cabe4eb34f74f7fc85', 'ai', 'seed', 'AI SPE'),
    (42161, '0x5f51c8eae3c97364613c48b42824be47aeb47ad0', 'ai', 'seed', 'AI SPE'),
    (42161, '0x5ae4e42db3671370a0c25aff451e7482aaec3d0b', 'ai', 'seed', 'Cloud SPE (AI)'),
    (42161, '0x87d4396204035736422c2c6dfce423bba6daa776', 'ai', 'seed', '0x87d439…daa776'),
    (42161, '0x491f5f5664f11a1e0ba6902a8ca37c09150be0db', 'ai', 'seed', '0x491f5f…0be0db'),
    (42161, '0x5be44e23041e93cdf9bcd5a0968524e104e38ae1', 'ai', 'seed', '0x5be44e…e38ae1'),
    (42161, '0xca3331d67e87816adb30d9562a6e8c0623fb7fef', 'ai', 'seed', 'Livepeer, Inc (Realtime AI Video)'),
    (42161, '0x8a8053c21696f27ed305a03bd1efc5d068d91d0e', 'ai', 'seed', 'Embody Network');
