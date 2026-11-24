use anyhow::{Context, Result, bail, ensure};
use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::{
    http,
    request::{GraphqlOptions, Request, strip_json_comments},
};

pub fn payload(query: &str, options: &GraphqlOptions) -> Result<String> {
    ensure!(!query.trim().is_empty(), "Enter a GraphQL operation");
    let mut body = json!({"query": query});
    if !options.variables.trim().is_empty() {
        let variables: Value = serde_json::from_str(&strip_json_comments(&options.variables)?)
            .context("Invalid GraphQL variables JSON")?;
        ensure!(
            variables.is_object(),
            "GraphQL variables must be a JSON object"
        );
        body["variables"] = variables;
    }
    if !options.operation_name.trim().is_empty() {
        body["operationName"] = json!(options.operation_name.trim());
    }
    Ok(serde_json::to_string(&body)?)
}

const INTROSPECTION_QUERY: &str = r#"
query PrestmanIntrospection {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types {
      kind name
      fields { name args { ...Input } type { ...Type } }
      inputFields { ...Input }
      enumValues { name }
      possibleTypes { name }
    }
  }
}
fragment Input on __InputValue {
  name defaultValue type { ...Type }
}
fragment Type on __Type {
  kind name
  ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } }
}
"#;

pub fn introspection_request(request: &Request) -> Request {
    Request {
        body: INTROSPECTION_QUERY.into(),
        graphql: Some(GraphqlOptions::default()),
        pre_request_script: String::new(),
        post_response_script: String::new(),
        ..request.clone()
    }
}

pub fn load_schema(client: &Client, request: Request) -> Result<String> {
    let response = http::send(client, request)?;
    ensure!(
        (200..300).contains(&response.status),
        "Schema request returned HTTP {}",
        response.status
    );
    let response: Value = serde_json::from_str(&response.raw_body)
        .context("The endpoint did not return GraphQL JSON")?;
    let Some(types) = response
        .pointer("/data/__schema/types")
        .and_then(Value::as_array)
    else {
        let message = response
            .pointer("/errors/0/message")
            .and_then(Value::as_str)
            .unwrap_or("The server did not return a GraphQL schema");
        bail!("{message}");
    };
    let root = &response["data"]["__schema"];
    let operations = ["query", "mutation", "subscription"]
        .into_iter()
        .filter_map(|operation| {
            root[format!("{operation}Type")]["name"]
                .as_str()
                .map(|name| format!("  {operation}: {name}"))
        })
        .collect::<Vec<_>>();
    let mut schema = Vec::new();
    if !operations.is_empty() {
        schema.push(format!("schema {{\n{}\n}}", operations.join("\n")));
    }
    for ty in types {
        let name = name(ty);
        if name.is_empty() || name.starts_with("__") {
            continue;
        }
        let (kind, fields) = match ty["kind"].as_str() {
            Some("OBJECT") => ("type", fields(&ty["fields"])),
            Some("INTERFACE") => ("interface", fields(&ty["fields"])),
            Some("INPUT_OBJECT") => ("input", values(&ty["inputFields"]).map(input).collect()),
            Some("ENUM") => ("enum", values(&ty["enumValues"]).map(name_of).collect()),
            Some("SCALAR") => {
                if !matches!(name, "Boolean" | "Float" | "ID" | "Int" | "String") {
                    schema.push(format!("scalar {name}"));
                }
                continue;
            }
            Some("UNION") => {
                let members = values(&ty["possibleTypes"])
                    .map(name_of)
                    .collect::<Vec<_>>();
                schema.push(format!("union {name} = {}", members.join(" | ")));
                continue;
            }
            _ => continue,
        };
        schema.push(format!("{kind} {name} {{\n  {}\n}}", fields.join("\n  ")));
    }
    Ok(schema.join("\n\n"))
}

fn name(value: &Value) -> &str {
    value["name"].as_str().unwrap_or_default()
}

fn name_of(value: &Value) -> String {
    name(value).into()
}

fn values(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

fn fields(value: &Value) -> Vec<String> {
    values(value)
        .map(|field| {
            let args = values(&field["args"]).map(input).collect::<Vec<_>>();
            let args = if args.is_empty() {
                String::new()
            } else {
                format!("({})", args.join(", "))
            };
            format!("{}{args}: {}", name(field), type_name(&field["type"]))
        })
        .collect()
}

fn input(value: &Value) -> String {
    let default = value["defaultValue"]
        .as_str()
        .map_or_else(String::new, |value| format!(" = {value}"));
    format!("{}: {}{default}", name(value), type_name(&value["type"]))
}

fn type_name(value: &Value) -> String {
    match value["kind"].as_str() {
        Some("LIST") => format!("[{}]", type_name(&value["ofType"])),
        Some("NON_NULL") => format!("{}!", type_name(&value["ofType"])),
        _ => value["name"].as_str().unwrap_or("…").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
        time::Duration,
    };

    use super::*;
    use crate::{
        request::Authorization,
        scripts::{self, Scopes},
        variables::Variable,
    };

    #[test]
    fn graphql_executes_named_mutation_and_loads_authenticated_schema() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/graphql", listener.local_addr()?);
        let server = thread::spawn(move || -> Result<()> {
            for introspection in [false, true] {
                let (mut stream, _) = listener.accept()?;
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line)?;
                assert_eq!(line, "POST /graphql HTTP/1.1\r\n");
                let mut headers = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                        break;
                    }
                    headers.push_str(&line);
                }
                assert!(headers.contains("content-type: application/json\r\n"));
                assert!(headers.contains("authorization: Bearer example-token\r\n"));
                assert!(
                    headers.contains(
                        "accept: application/graphql-response+json, application/json\r\n"
                    )
                );
                let len: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .context("Missing request content length")?
                    .parse()?;
                let mut body = vec![0; len];
                reader.read_exact(&mut body)?;
                let payload: Value = serde_json::from_slice(&body)?;
                let response = if introspection {
                    assert!(payload["query"].as_str().unwrap().contains("__schema"));
                    assert!(payload.get("operationName").is_none());
                    assert!(payload.get("variables").is_none());
                    json!({"data": {"__schema": {
                        "queryType": {"name": "Query"},
                        "types": [{"kind":"OBJECT","name":"Query","fields":[
                            {"name":"viewer","args":[{"name":"id","type":{"kind":"NON_NULL","ofType":{"kind":"SCALAR","name":"ID"}}}],
                             "type":{"kind":"OBJECT","name":"User"}}
                        ]}]
                    }}})
                } else {
                    assert_eq!(payload["operationName"], "Rename");
                    assert_eq!(
                        payload["variables"],
                        json!({"id": "person-42", "name": "Ada"})
                    );
                    assert!(
                        payload["query"]
                            .as_str()
                            .unwrap()
                            .contains("# GraphQL comment")
                    );
                    json!({"data":{"rename":{"id":"person-42"}}})
                };
                let response = response.to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/graphql-response+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                )?;
            }
            Ok(())
        });
        let request = Request {
            url,
            body: "# GraphQL comment\nmutation Rename($id: ID!, $name: String!) { rename(id: $id, name: $name) { id } }\nquery Get { __typename }".into(),
            authorization: Authorization::Bearer { token: "example-token".into() },
            graphql: Some(GraphqlOptions {
                variables: r#"{/* JSON editor comment */ "id":"{{selectedId}}", "name":"Ada"}"#.into(),
                operation_name: "Rename".into(),
            }),
            pre_request_script: "pm.test('phase', () => pm.expect(pm.info.eventName).to.equal('beforeQuery'));".into(),
            ..Request::graphql()
        };
        assert_eq!(
            serde_json::from_str::<Request>(&serde_json::to_string(&request)?)?,
            request
        );
        let client = http::client()?;
        let execution = scripts::execute(
            &client,
            request.clone(),
            Scopes {
                collection: vec![Variable {
                    name: "selectedId".into(),
                    value: "person-42".into(),
                    enabled: true,
                }],
                ..Scopes::default()
            },
        );
        let response = execution.response.map_err(anyhow::Error::msg)?;
        assert!(response.is_json);
        assert!(execution.report.tests[0].passed);
        let schema = load_schema(&client, introspection_request(&request))?;
        assert!(schema.contains("query: Query"));
        assert!(schema.contains("viewer(id: ID!): User"));
        server.join().expect("test GraphQL server panicked")?;
        Ok(())
    }

    #[test]
    fn graphql_checks_required_query_and_variables_shape() {
        assert!(payload("", &GraphqlOptions::default()).is_err());
        let options = GraphqlOptions {
            variables: "[]".into(),
            ..GraphqlOptions::default()
        };
        assert!(payload("{ viewer }", &options).is_err());
        let options = GraphqlOptions {
            variables: "{".into(),
            ..GraphqlOptions::default()
        };
        assert!(payload("{ viewer }", &options).is_err());
    }
}
