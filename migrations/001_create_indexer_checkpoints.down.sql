-- DEVELOPMENT ONLY. Per SPEC §11.1 (rule 2), down migrations never run in production.

DROP TABLE IF EXISTS indexer_checkpoints;
