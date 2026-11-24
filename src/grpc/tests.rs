use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context as TaskContext, Poll},
    thread,
};

use futures::StreamExt as _;
use serde_json::Value;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{
    body::Body,
    codegen::{BoxFuture, Service, http},
    server::{NamedService, StreamingService},
    transport::Server,
};
use tonic_reflection::{
    pb::v1::{ListServiceResponse, ServiceResponse},
    server::v1::{ServerReflection, ServerReflectionServer},
};

use super::*;
use crate::request::Pair;

const PROTO: &str = r#"
syntax = "proto3";
package prestman.test;
import "google/protobuf/timestamp.proto";
service Echo {
  rpc Unary(Payload) returns (Payload);
  rpc Client(stream Payload) returns (Payload);
  rpc Server(Payload) returns (stream Payload);
  rpc Duplex(stream Payload) returns (stream Payload);
  rpc Error(Payload) returns (Payload);
}
message Payload {
  string text = 1;
  int64 sequence = 2;
  google.protobuf.Timestamp time = 3;
}
"#;

struct ReflectionWithDelayedHeaders;

#[tonic::async_trait]
impl ServerReflection for ReflectionWithDelayedHeaders {
    type ServerReflectionInfoStream =
        stream::Once<futures::future::Ready<Result<ServerReflectionResponse, Status>>>;

    async fn server_reflection_info(
        &self,
        request: tonic::Request<Streaming<ServerReflectionRequest>>,
    ) -> Result<tonic::Response<Self::ServerReflectionInfoStream>, Status> {
        let first = request.into_inner().message().await?;
        if !matches!(
            first.and_then(|request| request.message_request),
            Some(MessageRequest::ListServices(_))
        ) {
            return Err(Status::invalid_argument("Expected a list-services request"));
        }
        let response = ServerReflectionResponse {
            message_response: Some(MessageResponse::ListServicesResponse(ListServiceResponse {
                service: vec![ServiceResponse {
                    name: "grpc.reflection.v1.ServerReflection".into(),
                }],
            })),
            ..Default::default()
        };
        Ok(tonic::Response::new(stream::once(futures::future::ready(
            Ok(response),
        ))))
    }
}

#[test]
fn reflection_sends_list_request_before_waiting_for_headers() -> Result<()> {
    runtime()?.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut request = Request::grpc();
        request.url = format!("grpc://{}", listener.local_addr()?);
        tokio::spawn(
            Server::builder()
                .add_service(ServerReflectionServer::new(ReflectionWithDelayedHeaders))
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );

        tokio::time::timeout(Duration::from_secs(3), async {
            let channel = connect(&request, request.grpc.as_ref().unwrap()).await?;
            reflect(channel, MetadataMap::new()).await
        })
        .await??;
        Ok(())
    })
}

#[test]
fn reflection_restores_omitted_imports_for_returned_types() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let gateway = directory.path().join("gateway.proto");
    std::fs::write(
        directory.path().join("common.proto"),
        r#"
        syntax = "proto3";
        package shop.common.v1;
        import "google/protobuf/struct.proto";
        message Payload { google.protobuf.Struct details = 1; }
        "#,
    )?;
    std::fs::write(
        &gateway,
        r#"
        syntax = "proto3";
        package shop.gateway.v1;
        import "common.proto";
        service Gateway { rpc Inspect(shop.common.v1.Payload) returns (Reply); }
        message Reply { shop.common.v1.Payload payload = 1; }
        "#,
    )?;
    let compiled = local_pool(&import_proto(&gateway)?)?;
    let mut files: BTreeMap<_, _> = compiled
        .file_descriptor_protos()
        .map(|file| (file.name().to_owned(), file.clone()))
        .collect();
    let common = files.get_mut("common.proto").unwrap();
    common.dependency.clear();
    common.message_type[0].field[0].type_name = Some("google.protobuf.Struct".into());
    let gateway = files.get_mut("gateway.proto").unwrap();
    gateway.dependency.clear();
    gateway.message_type[0].field[0].type_name = Some("common.v1.Payload".into());

    let pool = reflection_schema::build_pool(files)?;
    let method = pool
        .get_service_by_name("shop.gateway.v1.Gateway")
        .unwrap()
        .methods()
        .next()
        .unwrap();
    assert_eq!(method.input().full_name(), "shop.common.v1.Payload");
    let json = serde_json::json!({ "payload": { "details": { "enabled": true } } });
    let message = parse_message(method.output(), &json.to_string())?;
    let bytes = message.encode_to_vec();
    let decoded = DynamicMessage::decode(method.output(), bytes.as_slice())?;
    assert_eq!(
        serde_json::from_str::<Value>(&json_message(&decoded, false)?)?,
        json
    );
    Ok(())
}

#[derive(Clone)]
struct Echo(MessageDescriptor);

impl NamedService for Echo {
    const NAME: &'static str = "prestman.test.Echo";
}

impl Service<http::Request<Body>> for Echo {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _: &mut TaskContext<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<Body>) -> Self::Future {
        let endpoint = request
            .uri()
            .path()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        let descriptor = self.0.clone();
        Box::pin(async move {
            Ok(tonic::server::Grpc::new(MessageCodec(descriptor))
                .streaming(EchoMethod(endpoint), request)
                .await)
        })
    }
}

struct EchoMethod(String);

impl StreamingService<DynamicMessage> for EchoMethod {
    type Response = DynamicMessage;
    type ResponseStream = Pin<Box<dyn Stream<Item = Result<DynamicMessage, Status>> + Send>>;
    type Future = BoxFuture<tonic::Response<Self::ResponseStream>, Status>;

    fn call(&mut self, request: tonic::Request<Streaming<DynamicMessage>>) -> Self::Future {
        let endpoint = self.0.clone();
        Box::pin(async move {
            if endpoint == "Error" {
                return Err(Status::permission_denied("Access denied"));
            }
            assert_eq!(request.metadata().get("x-client").unwrap(), "prestman");
            assert_eq!(
                request
                    .metadata()
                    .get_bin("x-binary-bin")
                    .unwrap()
                    .to_bytes()
                    .unwrap(),
                b"hi".as_slice()
            );
            let incoming = request.into_inner();
            let stream: Self::ResponseStream = match endpoint.as_str() {
                "Client" => Box::pin(stream::once(async {
                    let mut incoming = incoming;
                    let mut last = None;
                    while let Some(message) = incoming.message().await? {
                        last = Some(message);
                    }
                    last.ok_or_else(|| Status::invalid_argument("Expected a message"))
                })),
                "Server" => Box::pin(
                    stream::once(async { incoming.into_future().await.0.unwrap() })
                        .flat_map(|message| stream::iter([message.clone(), message])),
                ),
                _ => Box::pin(incoming),
            };
            let mut trailers = MetadataMap::new();
            trailers.insert("x-end", "done".parse().unwrap());
            let stream: Self::ResponseStream = Box::pin(stream.chain(stream::iter([Err(
                Status::with_metadata(Code::Ok, "", trailers),
            )])));
            let mut response = tonic::Response::new(stream);
            response
                .metadata_mut()
                .insert("x-server", "echo".parse().unwrap());
            Ok(response)
        })
    }
}

#[test]
fn grpc_reflection_proto_json_metadata_and_all_streaming_modes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("echo.proto");
    std::fs::write(&path, PROTO)?;
    let proto = import_proto(&path)?;
    let pool = local_pool(&proto)?;
    let message = pool.get_message_by_name("prestman.test.Payload").unwrap();
    let (address_sender, address) = std::sync::mpsc::channel();
    let (shutdown, stop) = oneshot::channel();
    let server = thread::spawn(move || -> Result<()> {
        runtime()?.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            address_sender.send(listener.local_addr()?)?;
            // Exercise the fallback used by servers that implement only v1alpha.
            let reflection = tonic_reflection::server::Builder::configure()
                .register_file_descriptor_set(prost_types::FileDescriptorSet {
                    file: pool.file_descriptor_protos().cloned().collect(),
                })
                .build_v1alpha()?;
            Server::builder()
                .add_service(reflection)
                .add_service(Echo(message))
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    _ = stop.await;
                })
                .await?;
            Ok(())
        })
    });
    let mut request = Request::grpc();
    request.url = address.recv_timeout(Duration::from_secs(5))?.to_string();
    request.grpc.as_mut().unwrap().secure = Some(true);
    let address = format!("grpc://{}", request.url);
    edit_address(&mut request, address);
    request.body =
        r#"{"text":"hello","sequence":"9007199254740993","time":"2025-01-01T00:00:00Z"}"#.into();
    request.headers = vec![
        Pair {
            name: "x-client".into(),
            value: "prestman".into(),
            enabled: true,
        },
        Pair {
            name: "x-binary-bin".into(),
            value: "aGk".into(),
            enabled: true,
        },
    ];
    let reflected = discover(&request)?;
    assert_eq!(methods(&reflected).len(), 5);
    request.grpc.as_mut().unwrap().timeout_ms = 3000;
    for (method, count) in [("Unary", 1), ("Client", 1), ("Server", 2), ("Duplex", 2)] {
        request.method = format!("prestman.test.Echo/{method}");
        request.grpc.as_mut().unwrap().proto = (method == "Unary").then(|| proto.clone());
        let response = test_call(request.clone(), method == "Client" || method == "Duplex")?;
        assert_eq!(response.status, 0, "{}", response.body);
        assert!(response.headers.contains("x-server: echo"));
        assert!(
            response.trailers.contains("x-end: done"),
            "{}",
            response.trailers
        );
        let json: Value = serde_json::from_str(&response.body)?;
        let values = json.as_array().cloned().unwrap_or_else(|| vec![json]);
        assert_eq!(values.len(), count);
        assert_eq!(values[0]["sequence"], "9007199254740993");
        assert_eq!(values[0]["time"], "2025-01-01T00:00:00Z");
    }
    request.method = "prestman.test.Echo/Server".into();
    let mut scripted = request.clone();
    scripted.grpc.as_mut().unwrap().on_message_script = r#"
        pm.test('incoming message', () => {
            pm.expect(pm.info.eventName).to.equal('onIncomingMessage');
            pm.expect(pm.message.data.text).to.equal('hello');
        });
        pm.variables.set('count', 1 + Number(pm.variables.get('count') || 0));
    "#
    .into();
    scripted.post_response_script = r#"
        pm.test('after response', () => {
            pm.response.to.be.ok;
            pm.expect(pm.response.messages.count()).to.equal(2);
            pm.expect(pm.response.trailers.get('x-end')).to.equal('done');
            pm.expect(pm.variables.get('count')).to.equal('2');
        });
    "#
    .into();
    let (sender, messages) = tokio::sync::mpsc::channel(4);
    drop(sender);
    let (_cancel, cancel) = oneshot::channel();
    let (events, _progress) = mpsc::channel(64);
    let call = Call {
        events,
        messages,
        cancel,
        known_schema: None,
    };
    let client = crate::http::client()?;
    let execution = crate::scripts::execute_with(
        &client,
        scripted,
        Default::default(),
        |request, globals, scoped, observe| call.run(request, globals, scoped, observe),
    );
    assert!(execution.response.is_ok(), "{:?}", execution.report.errors);
    assert_eq!(execution.report.tests.len(), 3);
    assert!(execution.report.tests.iter().all(|test| test.passed));

    request.method = "prestman.test.Echo/Duplex".into();
    let (sender, messages) = tokio::sync::mpsc::channel(4);
    let (stop_call, cancel) = oneshot::channel();
    let (events, mut progress) = mpsc::channel(64);
    let duplex = request.clone();
    let worker = thread::spawn(move || {
        Call {
            events,
            messages,
            cancel,
            known_schema: None,
        }
        .run(duplex, &[], &[], None)
    });
    sender.try_send(request.body.clone())?;
    let received = runtime()?.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(event) = progress.next().await {
                if matches!(event, Event::Received(_)) {
                    return true;
                }
            }
            false
        })
        .await
    })?;
    assert!(
        received,
        "Duplex must receive a message before the client half-closes"
    );
    stop_call.send(()).expect("call should be open");
    assert_eq!(worker.join().expect("gRPC call worker panicked")?.status, 1);
    drop(sender);

    request.method = "prestman.test.Echo/Error".into();
    let response = test_call(request, false)?;
    assert_eq!(response.status, 7);
    assert!(response.trailers.contains("Access denied"));
    _ = shutdown.send(());
    server.join().expect("server panicked")?;
    Ok(())
}

#[test]
fn tls_handshake_gives_a_plaintext_hint_for_an_http2_endpoint() -> Result<()> {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let mut request = Request::grpc();
    request.url = format!("grpcs://{}", listener.local_addr()?);
    let server = thread::spawn(move || -> Result<_> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut first_byte = [0];
        socket.read_exact(&mut first_byte)?;
        // Plaintext HTTP/2 SETTINGS, as sent by servers that speak h2c.
        socket.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0])?;
        // Closing with unread ClientHello bytes can reset the connection on Windows.
        Ok(socket)
    });
    let result = discover(&request);
    let _server_socket = server.join().expect("test server panicked")?;
    let error = result.expect_err("a plaintext endpoint cannot negotiate TLS");
    assert!(format!("{error:#}").contains("Use grpc://"), "{error:#}");
    Ok(())
}

fn test_call(request: Request, client_streaming: bool) -> Result<Response> {
    let (sender, messages) = tokio::sync::mpsc::channel(4);
    if client_streaming {
        sender.try_send(request.body.clone())?;
        sender.try_send(request.body.clone())?;
    }
    drop(sender);
    let (_cancel, cancel) = oneshot::channel();
    let (events, _progress) = mpsc::channel(64);
    Call {
        events,
        messages,
        cancel,
        known_schema: None,
    }
    .run(request, &[], &[], None)
}
