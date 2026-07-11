-- diag_ro_role.sql — provision the read-only role used by livepeer-mcp-diag.
--
-- Run ONCE by a DB admin against the application database. This is the real
-- read-only boundary for the diagnostics MCP server (the server's own session
-- guards + SQL string checks are only defense-in-depth). Provisioning is
-- intentionally NOT a migration: it manages a role/grants, not schema, and it
-- needs a password that must not live in the migrations history.
--
-- Usage (from the postgres host). Pass the password RAW (no surrounding
-- quotes) — the script quotes it via psql's :'var' syntax:
--   PGPASSWORD=... psql -h localhost -U <admin> -d <appdb> \
--     -v diag_password=choose-a-strong-password -f scripts/diag_ro_role.sql
--
-- Then set, for livepeer-mcp-diag:
--   DIAG_DATABASE_URL=postgres://diag_ro:<password>@livepeer-valuation-postgres:5432/<appdb>
--
-- Note: psql does not substitute :'diag_password' inside dollar-quoted blocks,
-- so this script uses the \gexec pattern instead of DO blocks.

\set ON_ERROR_STOP on

-- 1. Role: create if missing (idempotent — the SELECT returns zero rows when
--    the role already exists, so \gexec runs nothing), then (re)set password.
SELECT 'CREATE ROLE diag_ro LOGIN'
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'diag_ro')
\gexec
ALTER ROLE diag_ro LOGIN PASSWORD :'diag_password';

-- 2. Force read-only + resource limits at the role level. Even a query that
--    tries to write fails; long/idle queries are cut off.
ALTER ROLE diag_ro SET default_transaction_read_only = on;
ALTER ROLE diag_ro SET statement_timeout = '15s';
ALTER ROLE diag_ro SET idle_in_transaction_session_timeout = '30s';

-- 3. Grants: connect + read the public schema only. No CREATE.
SELECT format('GRANT CONNECT ON DATABASE %I TO diag_ro', current_database())
\gexec
GRANT USAGE ON SCHEMA public TO diag_ro;
REVOKE CREATE ON SCHEMA public FROM diag_ro;

GRANT SELECT ON ALL TABLES IN SCHEMA public TO diag_ro;
GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO diag_ro;

-- 4. Future tables created by migrations should also be readable.
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO diag_ro;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON SEQUENCES TO diag_ro;

-- Verify: diag_ro should have rolcanlogin = t and no superuser/createdb.
-- SELECT rolname, rolsuper, rolcreatedb, rolcanlogin FROM pg_roles WHERE rolname='diag_ro';
