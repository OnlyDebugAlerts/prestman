\set ON_ERROR_STOP on
begin;

insert into auth.users (id, email, email_confirmed_at) values
    ('11000000-0000-4000-8000-000000000001', 'owner@example.test', now()),
    ('11000000-0000-4000-8000-000000000002', 'editor@example.test', now()),
    ('11000000-0000-4000-8000-000000000003', 'viewer@example.test', now()),
    ('11000000-0000-4000-8000-000000000004', 'outsider@example.test', now());

set local role authenticated;

do $$
declare
    owner_id constant uuid := '11000000-0000-4000-8000-000000000001';
    editor_id constant uuid := '11000000-0000-4000-8000-000000000002';
    viewer_id constant uuid := '11000000-0000-4000-8000-000000000003';
    outsider_id constant uuid := '11000000-0000-4000-8000-000000000004';
    collection_id constant text := '22000000-0000-4000-8000-000000000001';
    first_id constant text := '22000000-0000-4000-8000-000000000002';
    second_id constant text := '22000000-0000-4000-8000-000000000003';
    new_group_id constant text := '22000000-0000-4000-8000-000000000004';
    team uuid;
    editor_invite uuid;
    viewer_invite uuid;
    result jsonb;
    collection constant jsonb := '{"kind":"group","parent":null,"name":"Shared API","variables":[]}';
    collection_with_variables constant jsonb := jsonb_set(collection, '{variables}',
        '[{"name":"auth_token","value":"test-token","enabled":true}]');
    request_template constant jsonb := jsonb_build_object('kind', 'request', 'parent', collection_id, 'request',
        '{"name":"Health check","method":"GET","url":"https://example.test/health","body":""}'::jsonb);
    editor_request constant jsonb := jsonb_set(request_template, '{request,url}', '"https://example.test/ready"');
    owner_request constant jsonb := jsonb_set(request_template, '{request,url}', '"https://example.test/status"');
begin
    perform set_config('request.jwt.claim.sub', owner_id::text, true);
    team := (public.prestman_create_workspace('Team test')->>'id')::uuid;
    editor_invite := public.prestman_invite(team, 'editor@example.test', 'editor');
    viewer_invite := public.prestman_invite(team, 'viewer@example.test', 'viewer');
    assert jsonb_array_length(public.prestman_workspaces()) = 1;
    result := public.prestman_sync(team, jsonb_build_object(
        collection_id, jsonb_build_object('expected', null, 'content', collection),
        first_id, jsonb_build_object('expected', null, 'content', request_template),
        second_id, jsonb_build_object('expected', null, 'content', request_template)
    ));
    assert result->>'revision' = '1';

    begin
        perform public.prestman_sync(team, jsonb_build_object(
            new_group_id, jsonb_build_object('expected', null, 'content', collection_with_variables)
        ));
        raise exception 'Cloud accepted a new collection containing variables.';
    exception when invalid_parameter_value then null;
    end;
    begin
        perform public.prestman_sync(team, jsonb_build_object(
            collection_id, jsonb_build_object('expected', 1, 'content', collection_with_variables)
        ));
        raise exception 'Cloud accepted variables added to an existing collection.';
    exception when invalid_parameter_value then null;
    end;
    assert public.prestman_sync(team, '{}')->>'revision' = '1';

    perform set_config('request.jwt.claim.sub', outsider_id::text, true);
    assert public.prestman_workspaces() = '[]'::jsonb;
    begin
        perform public.prestman_accept_invite(editor_invite);
        raise exception 'An invitation could be accepted by the wrong email.';
    exception when insufficient_privilege then null;
    end;
    begin
        perform public.prestman_sync(team, '{}');
        raise exception 'A nonmember could read the workspace.';
    exception when insufficient_privilege then null;
    end;

    perform set_config('request.jwt.claim.sub', editor_id::text, true);
    assert public.prestman_accept_invite(editor_invite)->>'role' = 'editor';
    begin
        perform public.prestman_accept_invite(editor_invite);
        raise exception 'Invitation code could be reused.';
    exception when insufficient_privilege then null;
    end;
    result := public.prestman_sync(team, '{}', 1);
    assert result->'tree' = 'null'::jsonb;
    assert result->>'revision' = '1';
    result := public.prestman_sync(team, jsonb_build_object(
        first_id, jsonb_build_object('expected', 1, 'content', editor_request)
    ), 1);
    assert result->>'revision' = '2';
    assert result->'tree'->first_id->'content' = editor_request;

    perform set_config('request.jwt.claim.sub', owner_id::text, true);
    result := public.prestman_sync(team, jsonb_build_object(
        second_id, jsonb_build_object('expected', 1, 'content', owner_request)
    ), 1);
    assert result->>'revision' = '3', 'Concurrent edits to different requests should succeed.';
    assert result->'tree'->second_id->'content' = owner_request;
    result := public.prestman_sync(team, jsonb_build_object(
        second_id, jsonb_build_object('expected', 1, 'content', owner_request)
    ), 3);
    assert result->>'revision' = '3' and result->'conflicts' = '[]', 'A retry should be idempotent.';
    result := public.prestman_sync(team, jsonb_build_object(
        first_id, jsonb_build_object('expected', 1, 'content', owner_request),
        second_id, jsonb_build_object('expected', 3, 'content', editor_request)
    ), 2);
    assert result->'conflicts' = jsonb_build_array(first_id);
    assert result->'tree'->first_id->'content' = editor_request;
    assert result->'tree'->second_id->'content' = owner_request, 'A conflicting patch should be atomic.';
    begin
        perform public.prestman_sync(team, jsonb_build_object(
            collection_id, jsonb_build_object('expected', 1, 'content', null)
        ), 3);
        raise exception 'The server accepted a patch leaving orphan requests.';
    exception when invalid_parameter_value then null;
    end;

    perform set_config('request.jwt.claim.sub', viewer_id::text, true);
    assert public.prestman_accept_invite(viewer_invite)->>'role' = 'viewer';
    assert public.prestman_sync(team, '{}')->'tree'->first_id->'content' = editor_request;
    begin
        perform public.prestman_sync(team, jsonb_build_object(
            first_id, jsonb_build_object('expected', 2, 'content', owner_request)
        ));
        raise exception 'Viewer was allowed to edit.';
    exception when insufficient_privilege then null;
    end;

    perform set_config('request.jwt.claim.sub', owner_id::text, true);
    assert jsonb_array_length(public.prestman_members(team)) = 3;
    perform public.prestman_set_member_role(team, editor_id, 'viewer');
    perform set_config('request.jwt.claim.sub', editor_id::text, true);
    assert public.prestman_sync(team, '{}')->>'role' = 'viewer';
    perform set_config('request.jwt.claim.sub', owner_id::text, true);
    perform public.prestman_set_member_role(team, editor_id, null);
    perform set_config('request.jwt.claim.sub', editor_id::text, true);
    begin
        perform public.prestman_sync(team, '{}');
        raise exception 'A removed member could still read the workspace.';
    exception when insufficient_privilege then null;
    end;
    assert not has_schema_privilege(current_user, 'prestman_private', 'usage');
end;
$$;

rollback;
