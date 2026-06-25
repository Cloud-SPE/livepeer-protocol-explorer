-- 046_orchestrator_ens_avatar_cache
-- TD-033 local avatar caching. The enricher now resolves the raw ENS
-- `avatar` text record (which may be an http(s) URL, ipfs:// URI, data:
-- URI, or an eip155 NFT reference) down to actual image bytes and stores
-- them on a shared volume at <AVATAR_STORE_DIR>/<address>.<ext>. This
-- column records the extension of the stored file so the API can both
-- (a) know an avatar is cached locally and (b) locate the file to serve.
--
-- NULL means "no locally-cached avatar" (never resolved, resolution
-- failed, or the orchestrator has no avatar record). `ens_avatar_url`
-- continues to hold the raw, unresolved text record for debugging and as
-- a passthrough fallback for already-browser-loadable http(s) URLs.
ALTER TABLE orchestrator_ens
    ADD COLUMN ens_avatar_stored_ext TEXT;
