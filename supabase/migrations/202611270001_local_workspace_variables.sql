create function prestman_private.reject_cloud_variable_writes()
returns trigger language plpgsql set search_path = '' as $$
begin
    if exists (
        select 1 from jsonb_each(new.tree) as item(id, node)
        where node->'content'->>'kind' = 'group'
          and node->'content'->'variables' <> '[]'::jsonb
          and (
              old.tree->id->'content'->>'kind' is distinct from 'group'
              or old.tree->id->'content'->'variables' is distinct from node->'content'->'variables'
          )
    ) then
        raise exception using errcode = '22023', message = 'Cloud variables must be defined in Local workspace.';
    end if;
    return new;
end;
$$;

create trigger reject_cloud_variable_writes
before update of tree on prestman_private.workspaces
for each row execute function prestman_private.reject_cloud_variable_writes();
