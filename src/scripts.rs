use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use reqwest::blocking::Client;
use rquickjs::{CatchResultExt, Context, Function, Promise, Runtime, context::EvalOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    http::{self, Response},
    request::{Request, query_rows},
    variables::{Variable, resolve_request},
};

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(5);
use crate::visualizer::Visualization;

#[cfg(test)]
mod tests;

#[derive(Clone, Default, Serialize)]
pub struct Scopes {
    pub globals: Vec<Variable>,
    pub collection: Vec<Variable>,
    pub folder: Vec<Variable>,
    pub environment: Vec<Variable>,
    pub locals: BTreeMap<String, String>,
    pub collection_id: Option<usize>,
}

impl Scopes {
    pub fn for_request(
        globals: &[Variable],
        environment: &[Variable],
        groups: &[crate::storage::Collection],
        group: Option<usize>,
    ) -> Self {
        let chain = group
            .map(|id| crate::storage::lineage(groups, id))
            .unwrap_or_default();
        Self {
            globals: globals.to_vec(),
            environment: environment.to_vec(),
            collection_id: chain.first().map(|group| group.id),
            collection: chain
                .first()
                .map(|group| group.variables.clone())
                .unwrap_or_default(),
            folder: chain
                .iter()
                .skip(1)
                .flat_map(|group| group.variables.iter().cloned())
                .collect(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Mutation {
    pub scope: String,
    pub name: String,
    pub value: Option<String>,
}

impl Mutation {
    pub fn apply(&self, variables: &mut Vec<Variable>) {
        if let Some(value) = &self.value {
            if let Some(variable) = variables.iter_mut().find(|row| row.name == self.name) {
                variable.value = value.clone();
                variable.enabled = true;
            } else {
                variables.push(Variable {
                    name: self.name.clone(),
                    value: value.clone(),
                    enabled: true,
                });
            }
        } else {
            variables.retain(|row| row.name != self.name);
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub error: Option<String>,
    pub phase: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
    pub phase: String,
}

#[derive(Default)]
pub struct Report {
    pub tests: Vec<TestResult>,
    pub logs: Vec<LogEntry>,
    pub errors: Vec<String>,
    pub visualization: Option<Visualization>,
}

pub struct Execution {
    pub response: Result<Response, String>,
    pub report: Report,
    pub mutations: Vec<Mutation>,
}

#[derive(Deserialize)]
struct Output {
    request: Request,
    locals: BTreeMap<String, String>,
    mutations: Vec<Mutation>,
    tests: Vec<TestResult>,
    logs: Vec<LogEntry>,
    visualization: Option<Visualization>,
    visualizer_changed: bool,
}

pub fn execute(client: &Client, request: Request, scopes: Scopes) -> Execution {
    execute_with(client, request, scopes, |request, _, _, _| {
        http::send(client, request)
    })
}

pub type MessageObserver<'a> = dyn FnMut(&str) + 'a;

pub fn execute_with(
    client: &Client,
    request: Request,
    mut scopes: Scopes,
    send: impl FnOnce(
        Request,
        &[Variable],
        &[Variable],
        Option<&mut MessageObserver<'_>>,
    ) -> Result<Response>,
) -> Execution {
    let pre = request.pre_request_script.clone();
    let post = request.post_response_script.clone();
    let mut outgoing = request;
    let mut report = Report::default();
    let mut mutations = Vec::new();
    let response = (|| -> Result<Response> {
        if !pre.trim().is_empty() {
            let (output, error) = run(
                client,
                &pre,
                "Pre-request",
                &outgoing,
                None,
                &scopes,
                SCRIPT_TIMEOUT,
            )
            .context("Pre-request")?;
            outgoing = output.request.clone();
            absorb(output, &mut scopes, &mut mutations, &mut report);
            if let Some(error) = error {
                bail!("Pre-request: {error}");
            }
        }
        let collection: Vec<_> = scopes
            .collection
            .iter()
            .chain(&scopes.folder)
            .chain(&scopes.environment)
            .cloned()
            .chain(scopes.locals.iter().map(|(name, value)| Variable {
                name: name.clone(),
                value: value.clone(),
                enabled: true,
            }))
            .collect();
        let outgoing = resolve_request(&outgoing, &scopes.globals, &collection)?;
        let globals = scopes.globals.clone();
        let on_message_source = outgoing
            .grpc
            .as_ref()
            .map(|options| options.on_message_script.as_str())
            .filter(|source| !source.trim().is_empty());
        let mut on_message = |message: &str| {
            let source = on_message_source.expect("message script is configured");
            // reqwest's blocking client may be called by the script. Keep it outside
            // the gRPC worker's Tokio context while preserving message order.
            let result = std::thread::scope(|thread| {
                thread
                    .spawn(|| {
                        run_script(
                            client,
                            source,
                            "On message",
                            &outgoing,
                            ScriptData::Message(message),
                            &scopes,
                            SCRIPT_TIMEOUT,
                        )
                    })
                    .join()
                    .unwrap_or_else(|_| {
                        Err(anyhow::anyhow!("The message script stopped unexpectedly"))
                    })
            });
            match result {
                Ok((output, error)) => {
                    absorb(output, &mut scopes, &mut mutations, &mut report);
                    if let Some(error) = error {
                        report.errors.push(format!("On message: {error}"));
                    }
                }
                Err(error) => report.errors.push(format!("On message: {error:#}")),
            }
        };
        let observer: Option<&mut MessageObserver<'_>> = if on_message_source.is_some() {
            Some(&mut on_message)
        } else {
            None
        };
        let response = send(outgoing.clone(), &globals, &collection, observer)?;
        if !post.trim().is_empty() {
            match run(
                client,
                &post,
                "Post-response",
                &outgoing,
                Some(&response),
                &scopes,
                SCRIPT_TIMEOUT,
            ) {
                Ok((output, error)) => {
                    absorb(output, &mut scopes, &mut mutations, &mut report);
                    if let Some(error) = error {
                        report.errors.push(format!("Post-response: {error}"));
                    }
                }
                Err(error) => report.errors.push(format!("Post-response: {error:#}")),
            }
        }
        Ok(response)
    })()
    .map_err(|error| format!("{error:#}"));
    if let Err(error) = &response {
        report.errors.push(error.clone());
    }
    Execution {
        response,
        report,
        mutations,
    }
}

fn absorb(output: Output, scopes: &mut Scopes, mutations: &mut Vec<Mutation>, report: &mut Report) {
    if output.visualizer_changed {
        report.visualization = output.visualization;
    }
    scopes.locals = output.locals;
    for mutation in &output.mutations {
        let variables = match mutation.scope.as_str() {
            "globals" => &mut scopes.globals,
            "environment" => &mut scopes.environment,
            "collection" => &mut scopes.collection,
            _ => continue,
        };
        mutation.apply(variables);
    }
    mutations.extend(output.mutations);
    report.tests.extend(output.tests);
    report.logs.extend(output.logs);
}

fn response_data(response: &Response) -> Value {
    let rows = |text: &str| {
        text.lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                Some(json!({"key": key, "value": value.trim_start()}))
            })
            .collect::<Vec<_>>()
    };
    let mut data = json!({"code": response.status, "status": response.reason,
        "responseTime": response.elapsed.as_millis(), "size": response.body_bytes,
        "body": &*response.raw_body, "headers": rows(&response.headers)});
    if let Some(streaming) = response.grpc_server_streaming {
        let messages = if streaming {
            serde_json::from_str::<Vec<Value>>(&response.raw_body).unwrap_or_default()
        } else {
            serde_json::from_str::<Value>(&response.raw_body)
                .into_iter()
                .collect()
        };
        data["grpc"] = json!({
            "messages": messages,
            "trailers": rows(&response.trailers),
        });
    }
    data
}

fn run(
    client: &Client,
    source: &str,
    phase: &str,
    request: &Request,
    response: Option<&Response>,
    scopes: &Scopes,
    timeout: Duration,
) -> Result<(Output, Option<String>)> {
    run_script(
        client,
        source,
        phase,
        request,
        response.map_or(ScriptData::Request, ScriptData::Response),
        scopes,
        timeout,
    )
}

enum ScriptData<'a> {
    Request,
    Response(&'a Response),
    Message(&'a str),
}

fn run_script(
    client: &Client,
    source: &str,
    phase: &str,
    request: &Request,
    data: ScriptData<'_>,
    scopes: &Scopes,
    timeout: Duration,
) -> Result<(Output, Option<String>)> {
    let runtime = Runtime::new().context("Could not create the JavaScript runtime")?;
    runtime.set_memory_limit(64 * 1024 * 1024);
    runtime.set_max_stack_size(1024 * 1024);
    let started = Instant::now();
    let deadline = started + timeout;
    let limit = Arc::new(AtomicU64::new(timeout.as_micros() as u64));
    runtime.set_interrupt_handler(Some(Box::new({
        let limit = limit.clone();
        move || started.elapsed().as_micros() as u64 >= limit.load(Ordering::Relaxed)
    })));
    let context = Context::full(&runtime).context("Could not create the JavaScript context")?;
    context.with(|ctx| -> Result<_> {
        // Chai's UMD bundle detects the standard Node-style global alias.
        ctx.globals().set("global", ctx.globals())?;
        ctx.globals().set(
            "__cryptoRandomU32",
            Function::new(
                ctx.clone(),
                |ctx: rquickjs::Ctx<'_>| -> rquickjs::Result<u32> {
                    let mut bytes = [0u8; 4];
                    getrandom::fill(&mut bytes).map_err(|error| {
                        rquickjs::Exception::throw_message(
                            &ctx,
                            &format!("Could not read OS randomness: {error}"),
                        )
                    })?;
                    Ok(u32::from_ne_bytes(bytes))
                },
            )?,
        )?;
        ctx.eval::<(), _>(include_str!("scripts/crypto.js"))
            .catch(&ctx)
            .map_err(|e| anyhow::anyhow!("Could not initialize secure randomness: {e}"))?;
        let crypto_js = format!(
            "(function() {{\n{}\n}}).call(globalThis);",
            include_str!("../assets/vendor/crypto-js.js")
        );
        ctx.eval::<(), _>(crypto_js)
            .catch(&ctx)
            .map_err(|e| anyhow::anyhow!("Could not initialize CryptoJS: {e}"))?;
        let mut request = request.clone();
        let (response, message) = match data {
            ScriptData::Request => (None, None),
            ScriptData::Response(response) => (Some(response_data(response)), None),
            ScriptData::Message(message) => (None, Some(serde_json::from_str::<Value>(message)?)),
        };
        if response.is_some() || request.query.is_empty() {
            request.query = query_rows(&request.url);
        }
        let input = json!({"request": request, "response": response, "message": message,
            "scopes": scopes, "phase": phase});
        ctx.globals()
            .set("__inputJSON", serde_json::to_string(&input)?)?;
        let client = client.clone();
        ctx.globals().set(
            "__sendRequest",
            Function::new(ctx.clone(), move |input: String| -> String {
                let response = (|| -> Result<_> {
                    let request: Request = serde_json::from_str(&input)?;
                    let remaining = deadline
                        .checked_duration_since(Instant::now())
                        .context("Script timed out")?;
                    Ok(response_data(&http::send_with_timeout(
                        &client, request, remaining,
                    )?))
                })();
                match response {
                    Ok(response) => json!({"response": response}).to_string(),
                    Err(error) => json!({"error": format!("{error:#}")}).to_string(),
                }
            })?,
        )?;
        ctx.eval::<(), _>(include_str!("../assets/vendor/chai.js"))
            .catch(&ctx)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let handlebars = format!(
            "(function() {{\n{}\n}}).call(globalThis);",
            include_str!("../assets/vendor/handlebars.js")
        );
        ctx.eval::<(), _>(handlebars)
            .catch(&ctx)
            .map_err(|e| anyhow::anyhow!("Could not initialize Handlebars: {e}"))?;
        ctx.eval::<(), _>(include_str!("scripts/sandbox.js"))
            .catch(&ctx)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let script = format!("(async function() {{\n{source}\n}})().then(() => __waitForTests())");
        let mut options = EvalOptions::default();
        options.filename = Some(format!("{phase}.js"));
        let result = ctx
            .eval_with_options::<Promise, _>(script, options)
            .and_then(|promise| promise.finish::<()>());
        let error = result.catch(&ctx).err().map(|error| {
            if Instant::now() >= deadline {
                format!(
                    "Script exceeded the {} second execution limit",
                    timeout.as_secs_f64()
                )
            } else if matches!(
                error,
                rquickjs::CaughtError::Error(rquickjs::Error::WouldBlock)
            ) {
                "Script left an unresolved Promise or test callback. Timers are not available."
                    .into()
            } else {
                error.to_string()
            }
        });
        // Allow exporting completed logs/variable updates after an interrupted script.
        limit.store(
            (started.elapsed() + Duration::from_millis(100)).as_micros() as u64,
            Ordering::Relaxed,
        );
        let output = ctx
            .eval::<String, _>("__exportJSON()")
            .catch(&ctx)
            .map_err(|error| anyhow::anyhow!("Could not read script results: {error}"))?;
        Ok((serde_json::from_str(&output)?, error))
    })
}
