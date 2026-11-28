use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread,
};

fn variable(name: &str, value: &str) -> Variable {
    Variable {
        name: name.into(),
        value: value.into(),
        enabled: true,
    }
}

#[test]
fn cloud_request_resolves_and_updates_linked_local_workspace_variables() -> Result<()> {
    use crate::request::Authorization;

    let request = Request {
        url: "{{base_url}}/users".into(),
        authorization: Authorization::Bearer {
            token: "{{auth_token}}".into(),
        },
        pre_request_script: r#"
            pm.expect(pm.globals.get('base_url')).to.equal('https://example.test');
            pm.expect(pm.variables.get('auth_token')).to.equal('local-token');
            pm.environment.set('auth_token', 'refreshed-token');
            pm.expect(pm.variables.replaceIn('{{auth_token}}')).to.equal('refreshed-token');
        "#
        .into(),
        ..Request::default()
    };
    let globals = [variable("base_url", "https://example.test")];
    let mut environment = vec![variable("auth_token", "local-token")];
    let scopes = Scopes::for_cloud(true, &globals, &environment);
    let client = Client::new();
    let execution = execute_with(&client, request.clone(), scopes, |outgoing, _, _, _| {
        assert_eq!(outgoing.url, "https://example.test/users");
        assert_eq!(
            outgoing.authorization,
            Authorization::Bearer {
                token: "refreshed-token".into()
            },
        );
        Ok(Response {
            status: 200,
            reason: "OK",
            elapsed: Duration::ZERO,
            body_bytes: 0,
            is_json: false,
            body: "".into(),
            raw_body: "".into(),
            headers: String::new(),
            trailers: String::new(),
            grpc_server_streaming: None,
        })
    });
    assert!(execution.response.is_ok(), "{:?}", execution.response.err());
    let [mutation] = execution.mutations.as_slice() else {
        anyhow::bail!("Expected exactly one variable write");
    };
    assert_eq!(mutation.scope, "environment");
    mutation.apply(&mut environment);
    let scopes = Scopes::for_cloud(true, &globals, &environment);
    assert_eq!(
        scopes.resolve_request(&request)?.authorization,
        Authorization::Bearer {
            token: "refreshed-token".into()
        },
    );
    assert_eq!(crate::cloud::share_request(request.clone()), request);
    Ok(())
}

#[test]
fn cloud_disallows_persistent_variables_without_a_local_workspace_link() -> Result<()> {
    let environment = [variable("auth_token", "local-token")];
    let (output, error) = evaluate(
        r#"
        pm.expect(pm.variables.get('auth_token')).to.be.undefined;
        pm.expect(() => pm.environment.set('auth_token', 'token')).to.throw('Use Local workspace variables');
        pm.expect(() => pm.globals.clear()).to.throw('Use Local workspace variables');
        pm.variables.set('temporary', 'value');
        "#,
        None,
        &Scopes::for_cloud(false, &[], &environment),
    )?;
    assert!(error.is_none(), "{error:?}");
    assert!(output.mutations.is_empty());
    assert_eq!(output.locals["temporary"], "value");

    let (_, error) = evaluate(
        "pm.collectionVariables.set('auth_token', 'token');",
        None,
        &Scopes::for_cloud(true, &[], &environment),
    )?;
    assert!(
        error
            .unwrap_or_default()
            .contains("through pm.environment or pm.globals")
    );
    Ok(())
}

#[test]
fn nested_folder_scopes_match_script_lookup_and_keep_collection_mutations_at_the_root() -> Result<()>
{
    let groups = vec![
        crate::storage::Collection {
            id: 1,
            parent_id: None,
            name: "API".into(),
            variables: vec![variable("value", "root"), variable("root_only", "root")],
        },
        crate::storage::Collection {
            id: 2,
            parent_id: Some(1),
            name: "Parent".into(),
            variables: vec![
                variable("value", "parent"),
                variable("parent_only", "parent"),
            ],
        },
        crate::storage::Collection {
            id: 3,
            parent_id: Some(2),
            name: "Child".into(),
            variables: vec![variable("value", "child"), variable("nested", "{{value}}")],
        },
    ];
    let scopes = Scopes::for_request(&[], &[variable("value", "environment")], &groups, Some(3));
    assert_eq!(scopes.collection_id, Some(1));
    let (output, error) = evaluate(
        r#"
        pm.expect(pm.collectionVariables.get('value')).to.equal('root');
        pm.expect(pm.variables.get('value')).to.equal('environment');
        pm.expect(pm.variables.get('parent_only')).to.equal('parent');
        pm.expect(pm.variables.replaceIn('{{nested}}')).to.equal('environment');
        pm.collectionVariables.set('written', 'root-only');
    "#,
        None,
        &scopes,
    )?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(output.mutations[0].scope, "collection");
    let resolved = crate::variables::resolve_text(
        "{{nested}}",
        scopes
            .globals
            .into_iter()
            .chain(scopes.collection)
            .chain(scopes.folder)
            .chain(scopes.environment),
    )?;
    assert_eq!(resolved, "environment");
    let scopes = Scopes::for_request(&[], &[], &groups, Some(3));
    assert_eq!(
        crate::variables::resolve_text(
            "{{nested}}",
            scopes.collection.into_iter().chain(scopes.folder)
        )?,
        "child"
    );
    Ok(())
}

#[test]
fn imported_path_variables_can_be_updated_by_scripts_and_reach_the_server() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> Result<()> {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        assert_eq!(
            line,
            "GET /themes/dark%20theme?code=environment HTTP/1.1\r\n"
        );
        let mut headers = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line.is_empty() {
                break;
            }
            headers.push_str(&line);
        }
        assert!(headers.contains("x-code: environment\r\n"), "{headers}");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        )?;
        Ok(())
    });
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(
        file.path(),
        r#"
openapi: 3.1.0
info: { title: Path test, version: '1' }
paths:
  /themes/{code}:
    get:
      summary: Get theme
      parameters:
        - { name: code, in: path, required: true, schema: { type: string, default: initial } }
        - { name: code, in: query, required: true, schema: { type: string, default: '{{code}}' } }
        - { name: X-Code, in: header, required: true, schema: { type: string, default: '{{code}}' } }
      responses:
        '200': { description: OK }
"#,
    )?;
    let mut original = crate::openapi::read_file(file.path())?.requests.remove(0).1;
    original.pre_request_script = r#"
        pm.expect(pm.request.url.variables.get('code')).to.equal('initial');
        pm.request.url.variables.set('code', '{{theme}}');
        pm.expect(pm.variables.get('code')).to.equal('environment');
    "#
    .into();
    original.post_response_script = r#"
        pm.test('path is resolved', () => pm.expect(String(pm.request.url)).to.include('/themes/dark%20theme'));
    "#.into();
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;
    let execution = execute(
        &client,
        original.clone(),
        Scopes {
            collection: vec![
                variable("baseUrl", &format!("http://{address}")),
                variable("theme", "dark theme"),
            ],
            environment: vec![variable("code", "environment")],
            ..Scopes::default()
        },
    );
    assert!(execution.response.is_ok(), "{:?}", execution.response.err());
    assert!(
        execution.report.errors.is_empty(),
        "{:?}",
        execution.report.errors
    );
    assert_eq!(execution.report.tests.len(), 1);
    assert!(
        execution.report.tests[0].passed,
        "{:?}",
        execution.report.tests
    );
    assert_eq!(original.path_variables[0].value, "initial");
    server.join().unwrap()?;
    Ok(())
}

#[test]
fn cryptojs_supports_the_postman_base64url_function_and_hash_vectors() -> Result<()> {
    let (output, error) = evaluate(
        r#"
        const imported = require('crypto-js');
        pm.expect(imported).to.equal(CryptoJS);
        const b64url = (wordArray) =>
            CryptoJS.enc.Base64.stringify(wordArray)
                .replace(/=/g, '')
                .replace(/\+/g, '-')
                .replace(/\//g, '_');
        pm.test('Base64URL', () => {
            pm.expect(b64url(CryptoJS.enc.Utf8.parse('hello'))).to.equal('aGVsbG8');
            pm.expect(b64url(CryptoJS.enc.Hex.parse('fbff'))).to.equal('-_8');
            pm.expect(b64url(CryptoJS.lib.WordArray.create())).to.equal('');
        });
        pm.test('SHA-256', () => pm.expect(imported.SHA256('abc').toString()).to.equal('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'));
        pm.test('HMAC-SHA256', () => pm.expect(CryptoJS.HmacSHA256('The quick brown fox jumps over the lazy dog', 'key').toString()).to.equal('f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8'));
        pm.test('PBKDF2-SHA256', () => pm.expect(CryptoJS.PBKDF2('password', 'salt', {keySize:8, iterations:1, hasher:CryptoJS.algo.SHA256}).toString()).to.equal('120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b'));
        pm.environment.set('signature', b64url(CryptoJS.HmacSHA256('message', 'key')));
    "#,
        None,
        &Scopes::default(),
    )?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(output.tests.len(), 4);
    assert!(
        output.tests.iter().all(|test| test.passed),
        "{:?}",
        output.tests
    );
    assert_eq!(output.mutations[0].name, "signature");
    assert!(
        !output.mutations[0]
            .value
            .as_ref()
            .unwrap()
            .contains(['+', '/', '='])
    );
    Ok(())
}

#[test]
fn cryptojs_aes_and_secure_random_work_without_math_random() -> Result<()> {
    let (output, error) = evaluate(
        r#"
        const CryptoJS = require('crypto-js');
        Math.random = () => { throw new Error('Cryptographic randomness must come from the OS'); };
        pm.test('AES-128 known vector', () => {
            const key = CryptoJS.enc.Hex.parse('000102030405060708090a0b0c0d0e0f');
            const input = CryptoJS.enc.Hex.parse('00112233445566778899aabbccddeeff');
            const encrypted = CryptoJS.AES.encrypt(input, key, {mode:CryptoJS.mode.ECB, padding:CryptoJS.pad.NoPadding});
            pm.expect(encrypted.ciphertext.toString()).to.equal('69c4e0d86a7b0430d8cdb78070b4c55a');
        });
        pm.test('Random salts and Unicode AES roundtrip', () => {
            const first = CryptoJS.lib.WordArray.random(16);
            const second = CryptoJS.lib.WordArray.random(16);
            pm.expect(first.sigBytes).to.equal(16);
            pm.expect(first.toString()).not.to.equal(second.toString());
            const encrypted = CryptoJS.AES.encrypt('Привет 🌍', 'test passphrase').toString();
            pm.expect(CryptoJS.AES.decrypt(encrypted, 'test passphrase').toString(CryptoJS.enc.Utf8)).to.equal('Привет 🌍');
        });
        pm.test('Random API validates arguments', () => {
            pm.expect(() => crypto.getRandomValues(new Float32Array(1))).to.throw();
            pm.expect(() => crypto.getRandomValues(new Uint8Array(65537))).to.throw();
            const input = new Uint8Array(5);
            pm.expect(crypto.getRandomValues(input)).to.equal(input);
        });
    "#,
        None,
        &Scopes::default(),
    )?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(output.tests.len(), 3);
    assert!(
        output.tests.iter().all(|test| test.passed),
        "{:?}",
        output.tests
    );
    Ok(())
}

#[test]
fn visualizer_supports_handlebars_options_and_clear_between_phases() -> Result<()> {
    let (output, error) = evaluate(
        r#"
        const data = {title: '<API>', rows: [{name:'Alice'}, {name:'<Bob>'}]};
        pm.visualizer.set('<h1>{{title}}</h1><ul>{{#each rows}}<li>{{@index}}: {{name}}</li>{{/each}}</ul><script>pm.getData((err, data) => console.log(data));</script>', data);
        data.title = 'changed';
    "#,
        None,
        &Scopes::default(),
    )?;
    assert!(error.is_none(), "{error:?}");
    let visualization = output.visualization.as_ref().unwrap();
    assert!(visualization.html.contains("<h1>&lt;API&gt;</h1>"));
    assert!(visualization.html.contains("<li>1: &lt;Bob&gt;</li>"));
    assert!(visualization.html.contains("<script>pm.getData"));
    assert_eq!(visualization.data["title"], "<API>");
    let mut scopes = Scopes::default();
    let mut mutations = Vec::new();
    let mut report = Report::default();
    absorb(output, &mut scopes, &mut mutations, &mut report);
    assert!(report.visualization.is_some());
    let (unchanged, error) = evaluate(
        "console.log('keep pre-request visualization');",
        None,
        &scopes,
    )?;
    assert!(error.is_none());
    absorb(unchanged, &mut scopes, &mut mutations, &mut report);
    assert!(report.visualization.is_some());
    let (cleared, error) = evaluate("pm.visualizer.clear();", None, &scopes)?;
    assert!(error.is_none());
    absorb(cleared, &mut scopes, &mut mutations, &mut report);
    assert!(report.visualization.is_none());
    let (raw, error) = evaluate(
        "pm.visualizer.set('<p>{{value}}</p>', {value:'<b>raw</b>'}, {noEscape:true});",
        None,
        &scopes,
    )?;
    assert!(error.is_none());
    assert_eq!(raw.visualization.unwrap().html, "<p><b>raw</b></p>");
    let (_, error) = evaluate(
        "pm.visualizer.set('{{#each rows}}broken', {});",
        None,
        &scopes,
    )?;
    assert!(error.is_some());
    Ok(())
}

fn evaluate(
    source: &str,
    response: Option<&Response>,
    scopes: &Scopes,
) -> Result<(Output, Option<String>)> {
    run(
        &Client::new(),
        source,
        "Pre-request",
        &Request {
            url: "https://example.test/?q=1".into(),
            ..Request::default()
        },
        response,
        scopes,
        SCRIPT_TIMEOUT,
    )
}

#[test]
fn post_response_item_tokens_are_written_to_environment_scope() -> Result<()> {
    let body = json!({"items": [
        {"id":42,"tokens":[{"kid":"kid-a","secret":"secret-a","request_salt":"salt-a"}]},
        {"id":7,"tokens":[{"kid":"kid-b","secret":"secret-b","request_salt":"salt-b"}]}
    ]})
    .to_string();
    let response = Response {
        status: 200,
        reason: "OK",
        elapsed: Duration::from_millis(1),
        body_bytes: body.len(),
        is_json: true,
        body: body.clone().into(),
        raw_body: body.into(),
        headers: String::new(),
        trailers: String::new(),
        grpc_server_streaming: None,
    };
    let source = r#"
        const data = pm.response.json();
        const scope = pm.environment;
        const ids = [];
        data.items.forEach(item => {
            const id = item.id;
            const t = item.tokens[0];
            scope.set(`kid_id_${id}`, t.kid);
            scope.set(`secret_id_${id}`, t.secret);
            scope.set(`request_salt_id_${id}`, t.request_salt);
            ids.push(id);
        });
        console.log(`Items count: ${ids.length}`);
        console.log(`IDs: ${ids.join(", ")}`);
    "#;
    let (output, error) = run(
        &Client::new(),
        source,
        "Post-response",
        &Request::default(),
        Some(&response),
        &Scopes::default(),
        SCRIPT_TIMEOUT,
    )?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(output.mutations.len(), 6);
    assert!(
        output
            .mutations
            .iter()
            .all(|mutation| mutation.scope == "environment")
    );
    assert!(output.locals.is_empty());
    let mut scopes = Scopes::default();
    absorb(output, &mut scopes, &mut Vec::new(), &mut Report::default());
    assert_eq!(
        scopes
            .environment
            .iter()
            .find(|row| row.name == "kid_id_42")
            .unwrap()
            .value,
        "kid-a"
    );
    assert_eq!(
        scopes
            .environment
            .iter()
            .find(|row| row.name == "secret_id_7")
            .unwrap()
            .value,
        "secret-b"
    );
    assert_eq!(
        scopes
            .environment
            .iter()
            .find(|row| row.name == "request_salt_id_42")
            .unwrap()
            .value,
        "salt-a"
    );
    assert!(scopes.globals.is_empty());
    assert!(scopes.collection.is_empty());
    Ok(())
}

#[test]
fn scripts_mutate_variables_and_request_before_substitution() -> Result<()> {
    let scopes = Scopes {
        collection_id: Some(1),
        globals: vec![variable("value", "global")],
        collection: vec![variable("value", "collection")],
        environment: vec![variable("value", "environment")],
        ..Scopes::default()
    };
    let (output, error) = evaluate(
        r#"
        pm.expect(pm.variables.get('value')).to.equal('environment');
        pm.variables.set('value', 'local');
        pm.expect(pm.environment.get('value')).to.equal('environment');
        pm.expect(pm.variables.get('value')).to.equal('local');
        pm.globals.set('new', 'global');
        pm.collectionVariables.set('saved', 'collection');
        pm.environment.set('token', 'abc');
        pm.environment.unset('value');
        pm.expect(pm.variables.replaceIn('{{value}}/{{token}}')).to.equal('local/abc');
        pm.request.method = 'POST';
        pm.request.body.update('body={{value}}');
        pm.request.headers.upsert({key: 'X-Token', value: '{{token}}'});
        pm.request.headers.upsert({key: 'x-token', value: '{{value}}'});
        pm.request.url.query.upsert({key: 'q', value: '{{token}}'});
        pm.request.url.addQueryParams({key: 'other', value: 'a & b'});
        console.log('ready', {ok: true});
    "#,
        None,
        &scopes,
    )?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(output.request.method, "POST");
    assert_eq!(output.request.body, "body={{value}}");
    assert_eq!(output.request.headers.len(), 2);
    assert_eq!(
        output
            .request
            .headers
            .iter()
            .find(|row| row.name.eq_ignore_ascii_case("x-token"))
            .unwrap()
            .value,
        "{{value}}"
    );
    assert_eq!(
        output.request.url,
        "https://example.test/?q={{token}}&other=a%20%26%20b"
    );
    assert_eq!(output.locals["value"], "local");
    assert_eq!(output.mutations.len(), 4);
    assert_eq!(output.logs[0].message, "ready {\"ok\":true}");
    Ok(())
}

#[test]
fn chai_assertions_async_tests_and_raw_response_are_supported() -> Result<()> {
    let response = Response {
        status: 201,
        reason: "Created",
        elapsed: Duration::from_millis(12),
        body_bytes: 11,
        is_json: true,
        body: "{\n  \"ok\": true\n}".into(),
        raw_body: r#"{"ok":true}"#.into(),
        headers: "content-type: application/json\nx-test: yes".into(),
        trailers: String::new(),
        grpc_server_streaming: None,
    };
    let (output, error) = evaluate(
        r#"
        pm.test('status', () => pm.response.to.have.status(201));
        pm.test('raw text', () => pm.expect(pm.response.text()).to.equal('{"ok":true}'));
        pm.test('json', () => pm.expect(pm.response.json()).to.deep.equal({ok: true}));
        pm.test('header', () => pm.response.to.have.header('Content-Type', 'application/json'));
        pm.test('failed assertion', () => pm.expect([1, 2]).to.include(3));
        pm.test('async', async () => { await Promise.resolve(); pm.expect(pm.response.responseTime).to.be.below(20); });
        pm.test('done', done => { pm.expect(true).to.be.true; done(); });
        pm.test('async done', done => Promise.resolve().then(() => done()));
        pm.environment.set('ok', String(pm.response.json().ok));
    "#,
        Some(&response),
        &Scopes::default(),
    )?;
    assert!(error.is_none(), "{error:?}");
    assert_eq!(output.tests.len(), 8);
    assert_eq!(output.tests.iter().filter(|test| test.passed).count(), 7);
    assert!(output.tests[4].error.as_ref().unwrap().contains("include"));
    assert_eq!(output.mutations[0].value.as_deref(), Some("true"));
    Ok(())
}

#[test]
fn syntax_errors_exceptions_and_infinite_loops_are_reported() -> Result<()> {
    for source in [
        "let broken = ;",
        "throw new Error('failed');",
        "throw 'custom error';",
        "const huge = 'x'.repeat(80 * 1024 * 1024);",
        "function recurse() { return recurse(); } recurse();",
    ] {
        let (_, error) = evaluate(source, None, &Scopes::default())?;
        assert!(error.is_some(), "{source}");
    }
    let (output, error) = run(
        &Client::new(),
        "console.log('before'); while (true) {}",
        "Pre-request",
        &Request::default(),
        None,
        &Scopes::default(),
        Duration::from_millis(100),
    )?;
    assert!(error.unwrap().contains("execution limit"));
    assert_eq!(output.logs[0].message, "before");
    let execution = execute(
        &Client::new(),
        Request {
            pre_request_script: "throw new Error('stop');".into(),
            post_response_script: "pm.globals.set('wrong', 'yes');".into(),
            ..Request::default()
        },
        Scopes::default(),
    );
    assert!(execution.response.err().unwrap().contains("Pre-request"));
    assert!(execution.mutations.is_empty());
    let (_, error) = evaluate(
        "await pm.sendRequest('invalid-url').catch(error => pm.expect(error.message).to.be.a('string'));",
        None,
        &Scopes::default(),
    )?;
    assert!(error.is_none(), "{error:?}");
    Ok(())
}

#[test]
fn pre_request_can_fetch_a_token_and_post_response_retains_http_response_on_error() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> Result<()> {
        for path in ["/token", "/token", "/main?token=secret"] {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            assert!(line.starts_with(&format!("POST {path} HTTP/1.1")), "{line}");
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line)?;
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                headers.push_str(&line);
            }
            let length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length: ")
                        .and_then(|length| length.parse::<usize>().ok())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            if path.starts_with("/main") {
                assert!(headers.contains("x-token: secret\r\n"));
                assert_eq!(body, b"token=secret");
            }
            let body = r#"{"token":"secret"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )?;
        }
        Ok(())
    });
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;
    let request = Request {
        url: format!("http://{address}/main?token={{{{token}}}}"),
        pre_request_script: format!(r#"
            const url = 'http://{address}/token';
            pm.sendRequest({{url, method: 'POST'}}, (error, response) => {{
                pm.expect(error).to.equal(null);
                pm.variables.set('token', response.json().token);
            }});
            const response = await pm.sendRequest({{url, method: 'POST'}});
            pm.expect(response.code).to.equal(200);
            pm.request.method = 'POST';
            pm.request.body.update('token={{{{token}}}}');
            pm.request.headers.add({{key: 'X-Token', value: '{{{{token}}}}'}});
        "#),
        post_response_script: "pm.test('status', () => pm.response.to.have.status(200)); pm.environment.set('token', pm.response.json().token); console.log(pm.variables.get('token')); throw new Error('post failure');".into(),
        ..Request::default()
    };
    let execution = execute(&client, request.clone(), Scopes::default());
    assert!(execution.response.is_ok(), "{:?}", execution.report.errors);
    assert_eq!(execution.response.unwrap().status, 200);
    assert!(execution.report.errors[0].contains("post failure"));
    assert_eq!(execution.mutations[0].value.as_deref(), Some("secret"));
    assert_eq!(execution.report.logs[0].message, "secret");
    assert_eq!(request.method, "GET");
    server.join().unwrap()?;
    Ok(())
}

#[test]
fn bundled_example_scripts_pass_all_seven_checks_against_local_http() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> Result<()> {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(&mut stream);
        let mut first = String::new();
        reader.read_line(&mut first)?;
        let parts: Vec<_> = first.split_whitespace().collect();
        let url = reqwest::Url::parse(&format!("http://localhost{}", parts[1]))?;
        let method = parts[0].to_owned();
        let mut headers = BTreeMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.trim_end().split_once(':') {
                headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
            }
        }
        let length = headers.get("content-length").unwrap().parse::<usize>()?;
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        let body = json!({"method": method, "query": query, "headers": headers,
            "body": serde_json::from_slice::<Value>(&body)?})
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        Ok(())
    });
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;
    let execution = execute(
        &client,
        Request {
            url: format!("http://{address}/check?id={{{{test_id}}}}"),
            pre_request_script: include_str!("../../examples/scripts/pre-request.js").into(),
            post_response_script: include_str!("../../examples/scripts/post-response.js").into(),
            ..Request::default()
        },
        Scopes::default(),
    );
    assert!(execution.response.is_ok(), "{:?}", execution.report.errors);
    assert!(
        execution.report.errors.is_empty(),
        "{:?}",
        execution.report.errors
    );
    assert_eq!(execution.report.tests.len(), 7);
    assert!(
        execution.report.tests.iter().all(|test| test.passed),
        "{:?}",
        execution.report.tests
    );
    assert_eq!(execution.mutations[0].scope, "environment");
    assert_eq!(execution.mutations[0].name, "script_check");
    assert_eq!(execution.mutations[0].value.as_deref(), Some("post-ok"));
    assert_eq!(execution.report.logs.len(), 2);
    server.join().unwrap()?;
    Ok(())
}
