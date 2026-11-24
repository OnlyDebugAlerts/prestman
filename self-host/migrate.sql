begin;
select pg_advisory_xact_lock(hashtext('prestman schema migrations'));

create schema if not exists prestman_install;
create table if not exists prestman_install.migrations (version text primary key);

-- PostgREST v14 carries JWT claims in request.jwt.claims.
create or replace function auth.uid() returns uuid
language sql stable set search_path = '' as $$
    select coalesce(
        nullif(current_setting('request.jwt.claim.sub', true), ''),
        nullif(current_setting('request.jwt.claims', true), '')::jsonb->>'sub'
    )::uuid;
$$;

select not exists (
    select from prestman_install.migrations where version = '202611200001'
) as apply_team_workspaces \gset
\if :apply_team_workspaces
\ir /migrations/202611200001_team_workspaces.sql
insert into prestman_install.migrations values ('202611200001');
\endif

commit;
