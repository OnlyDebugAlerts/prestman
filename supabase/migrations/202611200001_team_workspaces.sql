create schema prestman_private;
revoke all on schema prestman_private from public, anon, authenticated;

create table prestman_private.workspaces (
    id uuid primary key default gen_random_uuid(),
    name text not null check (char_length(btrim(name)) between 1 and 100),
    revision bigint not null default 0,
    tree jsonb not null default '{}'::jsonb
);

create table prestman_private.members (
    workspace_id uuid references prestman_private.workspaces on delete cascade,
    user_id uuid references auth.users on delete cascade,
    role text not null check (role in ('owner', 'editor', 'viewer')),
    primary key (workspace_id, user_id)
);

create table prestman_private.invites (
    code uuid primary key default gen_random_uuid(),
    workspace_id uuid not null references prestman_private.workspaces on delete cascade,
    email text not null,
    role text not null check (role in ('editor', 'viewer')),
    expires_at timestamptz not null default (now() + interval '7 days')
);

create index prestman_member_users on prestman_private.members (user_id);

create function prestman_private.require_role(p_workspace uuid, p_roles text[])
returns text language plpgsql stable set search_path = '' as $$
declare
    membership text;
begin
    select role into membership from prestman_private.members
    where workspace_id = p_workspace and user_id = auth.uid();
    if membership is null or not (membership = any(p_roles)) then
        raise exception using errcode = '42501', message = 'You do not have access to this workspace action.';
    end if;
    return membership;
end;
$$;

create function public.prestman_workspaces()
returns jsonb language sql stable security definer set search_path = '' as $$
    select coalesce(jsonb_agg(jsonb_build_object('id', w.id, 'name', w.name, 'role', m.role)
                             order by w.name, w.id), '[]'::jsonb)
    from prestman_private.workspaces w
    join prestman_private.members m on m.workspace_id = w.id
    where m.user_id = auth.uid();
$$;

create function public.prestman_create_workspace(p_name text)
returns jsonb language plpgsql security definer set search_path = '' as $$
declare
    workspace prestman_private.workspaces;
begin
    if auth.uid() is null then
        raise exception using errcode = '42501', message = 'Sign in to create a workspace.';
    end if;
    insert into prestman_private.workspaces (name) values (btrim(p_name)) returning * into workspace;
    insert into prestman_private.members values (workspace.id, auth.uid(), 'owner');
    return jsonb_build_object('id', workspace.id, 'name', workspace.name, 'role', 'owner');
end;
$$;

create function public.prestman_invite(p_workspace uuid, p_email text, p_role text)
returns uuid language plpgsql security definer set search_path = '' as $$
declare
    invite_code uuid;
    email_address text := lower(btrim(p_email));
begin
    perform 1 from prestman_private.workspaces where id = p_workspace for update;
    perform prestman_private.require_role(p_workspace, array['owner']);
    if email_address is null or char_length(email_address) > 320 or position('@' in email_address) < 2 then
        raise exception using errcode = '22023', message = 'Enter the recipient email address.';
    end if;
    delete from prestman_private.invites where workspace_id = p_workspace
        and (email = email_address or expires_at <= now());
    insert into prestman_private.invites (workspace_id, email, role)
    values (p_workspace, email_address, p_role) returning code into invite_code;
    return invite_code;
end;
$$;

create function public.prestman_accept_invite(p_code uuid)
returns jsonb language plpgsql security definer set search_path = '' as $$
declare
    invitation prestman_private.invites;
    workspace prestman_private.workspaces;
    membership text;
    email_address text;
begin
    select lower(email) into email_address from auth.users
    where id = auth.uid() and email_confirmed_at is not null;
    select w.* into workspace from prestman_private.workspaces w
    join prestman_private.invites i on i.workspace_id = w.id
    where i.code = p_code and i.email = email_address and i.expires_at > now()
    for update of w;
    delete from prestman_private.invites
    where code = p_code and workspace_id = workspace.id and email = email_address and expires_at > now()
    returning * into invitation;
    if not found then
        raise exception using errcode = '42501', message = 'The invitation is invalid, expired, or was sent to another email.';
    end if;
    insert into prestman_private.members values (workspace.id, auth.uid(), invitation.role)
        on conflict (workspace_id, user_id) do nothing;
    select role into membership from prestman_private.members
    where workspace_id = workspace.id and user_id = auth.uid();
    return jsonb_build_object('id', workspace.id, 'name', workspace.name, 'role', membership);
end;
$$;

create function public.prestman_members(p_workspace uuid)
returns jsonb language plpgsql stable security definer set search_path = '' as $$
begin
    perform prestman_private.require_role(p_workspace, array['owner']);
    return (
        select coalesce(jsonb_agg(jsonb_build_object('id', u.id, 'email', u.email, 'role', m.role)
                                 order by u.email, u.id), '[]'::jsonb)
        from prestman_private.members m join auth.users u on u.id = m.user_id
        where m.workspace_id = p_workspace
    );
end;
$$;

create function public.prestman_set_member_role(p_workspace uuid, p_member uuid, p_role text)
returns jsonb language plpgsql security definer set search_path = '' as $$
begin
    perform 1 from prestman_private.workspaces where id = p_workspace for update;
    perform prestman_private.require_role(p_workspace, array['owner']);
    if p_member = auth.uid() or (p_role is not null and p_role not in ('editor', 'viewer')) then
        raise exception using errcode = '22023', message = 'Choose an editor or viewer.';
    end if;
    if p_role is null then
        delete from prestman_private.invites
        where workspace_id = p_workspace
          and email = (select lower(email) from auth.users where id = p_member);
        delete from prestman_private.members where workspace_id = p_workspace and user_id = p_member;
    else
        update prestman_private.members set role = p_role
        where workspace_id = p_workspace and user_id = p_member;
    end if;
    return public.prestman_members(p_workspace);
end;
$$;

create function prestman_private.validate_tree(p_tree jsonb)
returns void language plpgsql set search_path = '' as $$
begin
    if octet_length(p_tree::text) > 8388608 or (select count(*) from jsonb_each(p_tree)) > 2000 then
        raise exception using errcode = '22023', message = 'This workspace exceeds the collection storage limit.';
    end if;
    if exists (
        select 1 from jsonb_each(p_tree) as item(id, node)
        where id !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
            or node->'content'->>'kind' is null
            or node->'content'->>'kind' not in ('group', 'request')
            or (node->'content'->>'parent' is not null
                and (p_tree->(node->'content'->>'parent')->'content'->>'kind') is distinct from 'group')
            or (node->'content'->>'kind' = 'request' and (
                jsonb_typeof(node->'content'->'request') is distinct from 'object'
                or jsonb_typeof(node->'content'->'request'->'name') is distinct from 'string'
                or jsonb_typeof(node->'content'->'request'->'method') is distinct from 'string'
                or jsonb_typeof(node->'content'->'request'->'url') is distinct from 'string'
                or jsonb_typeof(node->'content'->'request'->'body') is distinct from 'string'
            ))
            or (node->'content'->>'kind' = 'group'
                and (jsonb_typeof(node->'content'->'name') is distinct from 'string'
                     or jsonb_typeof(node->'content'->'variables') is distinct from 'array'))
    ) or exists (
        with recursive ancestors as (
            select id, node->'content'->>'parent' as parent, 0 as depth
                from jsonb_each(p_tree) as item(id, node)
            union all
            select a.id, p_tree->a.parent->'content'->>'parent', a.depth + 1
                from ancestors a where a.parent is not null and a.depth < 64
        )
        select 1 from ancestors where parent is not null and depth = 64
    ) then
        raise exception using errcode = '22023', message = 'The workspace collection tree is invalid.';
    end if;
end;
$$;

-- Lock the workspace before checking write permissions and expected item revisions.
-- Conflicting items leave the entire patch unapplied.
create function public.prestman_sync(p_workspace uuid, p_changes jsonb, p_revision bigint default null)
returns jsonb language plpgsql security definer set search_path = '' as $$
declare
    workspace prestman_private.workspaces;
    membership text;
    conflicts jsonb;
    change record;
    changed boolean := false;
begin
    if jsonb_typeof(p_changes) is distinct from 'object' or octet_length(p_changes::text) > 8388608 then
        raise exception using errcode = '22023', message = 'Invalid workspace changes.';
    end if;
    if p_changes = '{}'::jsonb then
        select * into workspace from prestman_private.workspaces where id = p_workspace for share;
        membership := prestman_private.require_role(p_workspace, array['owner', 'editor', 'viewer']);
    else
        select * into workspace from prestman_private.workspaces where id = p_workspace for update;
        membership := prestman_private.require_role(p_workspace, array['owner', 'editor']);
    end if;

    select coalesce(jsonb_agg(id order by id), '[]'::jsonb) into conflicts
    from jsonb_each(p_changes) as item(id, patch)
    where (workspace.tree->id->>'revision')::bigint is distinct from (patch->>'expected')::bigint
      and coalesce(workspace.tree->id->'content', 'null'::jsonb) is distinct from patch->'content';

    if p_changes <> '{}'::jsonb and conflicts = '[]'::jsonb then
        for change in select key as id, value->'content' as content from jsonb_each(p_changes) loop
            if change.content is not distinct from coalesce(workspace.tree->change.id->'content', 'null'::jsonb) then
                continue;
            end if;
            changed := true;
            if change.content is null then
                raise exception using errcode = '22023', message = 'Missing workspace item content.';
            elsif change.content = 'null'::jsonb then
                workspace.tree := workspace.tree - change.id;
            else
                workspace.tree := jsonb_set(workspace.tree, array[change.id],
                    jsonb_build_object('revision', workspace.revision + 1, 'content', change.content));
            end if;
        end loop;
        if changed then
            perform prestman_private.validate_tree(workspace.tree);
            workspace.revision := workspace.revision + 1;
            update prestman_private.workspaces set tree = workspace.tree, revision = workspace.revision
            where id = p_workspace;
        end if;
    end if;

    return jsonb_build_object(
        'tree', case when p_revision is distinct from workspace.revision then workspace.tree end,
        'revision', workspace.revision, 'conflicts', conflicts, 'role', membership
    );
end;
$$;

revoke all on function
    public.prestman_workspaces(),
    public.prestman_create_workspace(text),
    public.prestman_invite(uuid, text, text),
    public.prestman_accept_invite(uuid),
    public.prestman_members(uuid),
    public.prestman_set_member_role(uuid, uuid, text),
    public.prestman_sync(uuid, jsonb, bigint)
from public, anon, authenticated;

grant execute on function
    public.prestman_workspaces(),
    public.prestman_create_workspace(text),
    public.prestman_invite(uuid, text, text),
    public.prestman_accept_invite(uuid),
    public.prestman_members(uuid),
    public.prestman_set_member_role(uuid, uuid, text),
    public.prestman_sync(uuid, jsonb, bigint)
to authenticated;
