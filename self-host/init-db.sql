\getenv auth_password AUTH_DB_PASSWORD
\getenv rest_password REST_DB_PASSWORD

create role anon nologin;
create role authenticated nologin;
create role service_role nologin;
create role authenticator noinherit login password :'rest_password';
grant anon, authenticated to authenticator;

create role supabase_auth_admin noinherit login password :'auth_password';
create schema auth authorization supabase_auth_admin;
alter role supabase_auth_admin set search_path to auth, public;
create extension if not exists pgcrypto;
create extension if not exists "uuid-ossp";
