use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use futures::{
    SinkExt as _, Stream,
    channel::{mpsc, oneshot},
    stream,
};
use prost::Message as _;
use prost_reflect::{
    DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor, SerializeOptions,
};
use reqwest::Url;
use tonic::{
    Code, Status, Streaming,
    client::Grpc,
    codec::{Codec, CompressionEncoding, DecodeBuf, Decoder, EncodeBuf, Encoder},
    codegen::http::uri::PathAndQuery,
    metadata::{Ascii, Binary, KeyAndValueRef, MetadataKey, MetadataMap},
    transport::{Channel, ClientTlsConfig, Endpoint},
};
use tonic_reflection::pb::v1::{
    ServerReflectionRequest, ServerReflectionResponse, server_reflection_request::MessageRequest,
    server_reflection_response::MessageResponse,
};

use crate::{
    http::Response,
    request::{Authorization, GrpcOptions, GrpcProto, Request, strip_json_comments},
    variables::{Variable, resolve_text},
};

mod reflection_schema;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETAINED_BYTES: usize = 8 * 1024 * 1024;

pub enum Event {
    Ready {
        client_streaming: bool,
        streaming: bool,
    },
    Sent(String),
    Received(String),
    Metadata(String),
    InputError(String),
}

pub fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}

pub fn import_proto(path: &Path) -> Result<GrpcProto> {
    let path = path.canonicalize()?;
    let pool = if path
        .extension()
        .is_some_and(|extension| extension == "proto")
    {
        let parent = path
            .parent()
            .context("The .proto file has no parent directory")?;
        let mut compiler = protox::Compiler::new([parent])?;
        compiler.include_imports(true).open_file(&path)?;
        DescriptorPool::decode(compiler.encode_file_descriptor_set().as_slice())?
    } else {
        DescriptorPool::decode(std::fs::read(&path)?.as_slice())
            .context("Choose a .proto file or a Protobuf descriptor set")?
    };
    ensure!(
        !methods(&pool).is_empty(),
        "The schema defines no gRPC methods"
    );
    Ok(GrpcProto {
        path,
        descriptors: STANDARD.encode(pool.encode_to_vec()),
    })
}

fn local_pool(proto: &GrpcProto) -> Result<DescriptorPool> {
    Ok(DescriptorPool::decode(
        STANDARD.decode(&proto.descriptors)?.as_slice(),
    )?)
}

pub fn methods(pool: &DescriptorPool) -> Vec<MethodDescriptor> {
    let mut methods = pool
        .services()
        .filter(|service| !service.full_name().starts_with("grpc.reflection."))
        .flat_map(|service| service.methods().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    methods.sort_by_key(method_path);
    methods
}

pub fn method_path(method: &MethodDescriptor) -> String {
    format!("{}/{}", method.parent_service().full_name(), method.name())
}

pub fn method_kind(method: &MethodDescriptor) -> &'static str {
    match (method.is_client_streaming(), method.is_server_streaming()) {
        (false, false) => "Unary",
        (true, false) => "Client streaming",
        (false, true) => "Server streaming",
        (true, true) => "Bidirectional streaming",
    }
}

pub fn example(method: &MethodDescriptor) -> Result<String> {
    json_message(&DynamicMessage::new(method.input()), true)
}

pub fn discover(request: &Request) -> Result<DescriptorPool> {
    let options = request.grpc.as_ref().context("Expected a gRPC request")?;
    if let Some(proto) = &options.proto {
        return local_pool(proto);
    }
    runtime()?.block_on(async {
        tokio::time::timeout(DISCOVERY_TIMEOUT, async {
            let channel = connect(request, options).await?;
            reflect(channel, metadata(request)?).await
        })
        .await
        .context("Server reflection timed out")?
    })
}

pub fn uses_tls(address: &str, options: &GrpcOptions) -> bool {
    options.secure.unwrap_or_else(|| {
        let address = address.trim();
        explicit_tls(address).unwrap_or_else(|| {
            address
                .parse::<tonic::codegen::http::uri::Authority>()
                .is_ok_and(|authority| authority.port_u16().is_none_or(|port| port == 443))
        })
    })
}

fn explicit_tls(address: &str) -> Option<bool> {
    match address.trim().split_once("://")?.0 {
        "grpc" | "http" => Some(false),
        "grpcs" | "https" => Some(true),
        _ => None,
    }
}

pub fn edit_address(request: &mut Request, address: String) {
    if address != request.url
        && explicit_tls(&address).is_some()
        && let Some(options) = &mut request.grpc
    {
        options.secure = None;
    }
    request.url = address;
}

async fn connect(request: &Request, options: &GrpcOptions) -> Result<Channel> {
    let raw = request.url.trim();
    let authority = if let Some((scheme, address)) = raw.split_once("://") {
        ensure!(
            matches!(scheme, "http" | "https" | "grpc" | "grpcs"),
            "Use grpc:// or grpcs:// for the server address"
        );
        address
    } else {
        raw
    };
    let secure = uses_tls(raw, options);
    let scheme = if secure { "https" } else { "http" };
    let address = format!("{scheme}://{authority}");
    let url =
        Url::parse(&address).context("Enter a gRPC server address such as localhost:50051")?;
    ensure!(
        url.has_host()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none(),
        "Enter only the gRPC server host and port"
    );
    let mut endpoint = Endpoint::from_shared(url.to_string())?
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("Prestman/", env!("CARGO_PKG_VERSION")))?;
    if secure {
        let mut tls = ClientTlsConfig::new().with_native_roots();
        if !options.server_name.trim().is_empty() {
            tls = tls.domain_name(options.server_name.trim());
        }
        endpoint = if options.verify_tls {
            endpoint.tls_config(tls)?
        } else {
            endpoint.tls_config_with_verifier(tls, Arc::new(SkipCertificateVerification))?
        };
    }
    let result = endpoint
        .connect()
        .await
        .context("Could not connect to the gRPC server");
    if secure
        && let Err(error) = &result
        && error.chain().any(|cause| {
            let rustls = cause.downcast_ref::<rustls::Error>().or_else(|| {
                cause
                    .downcast_ref::<std::io::Error>()?
                    .get_ref()?
                    .downcast_ref::<rustls::Error>()
            });
            matches!(
                rustls,
                Some(rustls::Error::InvalidMessage(
                    rustls::InvalidMessage::InvalidContentType
                ))
            )
        })
    {
        bail!(
            "The server did not respond with TLS. Use grpc:// or turn off Enable TLS for a plaintext endpoint."
        );
    }
    result
}

#[derive(Debug)]
struct SkipCertificateVerification;

impl rustls::client::danger::ServerCertVerifier for SkipCertificateVerification {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn metadata(request: &Request) -> Result<MetadataMap> {
    let mut result = MetadataMap::new();
    for (index, row) in request.headers.iter().enumerate() {
        if !row.enabled || row.is_blank() {
            continue;
        }
        let key = row.name.trim().to_ascii_lowercase();
        let value = row.value.trim();
        let context = || format!("Invalid gRPC metadata in row {}", index + 1);
        if key == "authorization" && !request.authorization.is_none() {
            continue;
        }
        if key.ends_with("-bin") {
            let bytes = STANDARD
                .decode(value)
                .or_else(|_| STANDARD_NO_PAD.decode(value))
                .with_context(context)?;
            result.append_bin(
                key.parse::<MetadataKey<Binary>>().with_context(context)?,
                tonic::metadata::MetadataValue::from_bytes(&bytes),
            );
        } else {
            result.append(
                key.parse::<MetadataKey<Ascii>>().with_context(context)?,
                value.parse().with_context(context)?,
            );
        }
    }
    let auth = match &request.authorization {
        Authorization::None => return Ok(result),
        Authorization::Bearer { token } => {
            ensure!(!token.trim().is_empty(), "Enter a Bearer token");
            format!("Bearer {}", token.trim())
        }
        Authorization::Basic { username, password } => {
            ensure!(
                !username.contains(':'),
                "A Basic Auth username cannot contain a colon"
            );
            format!(
                "Basic {}",
                STANDARD.encode(format!("{username}:{password}"))
            )
        }
    };
    result.insert(
        "authorization",
        auth.parse().context("Invalid authorization metadata")?,
    );
    Ok(result)
}

fn metadata_text(metadata: &MetadataMap) -> String {
    metadata
        .iter()
        .map(|row| match row {
            KeyAndValueRef::Ascii(key, value) => format!(
                "{key}: {}",
                String::from_utf8_lossy(value.as_encoded_bytes())
            ),
            KeyAndValueRef::Binary(key, value) => format!(
                "{key}: {}",
                String::from_utf8_lossy(value.as_encoded_bytes())
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn reflect(channel: Channel, metadata: MetadataMap) -> Result<DescriptorPool> {
    match reflection_session(
        channel.clone(),
        metadata.clone(),
        "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo",
    )
    .await
    {
        Ok(pool) => Ok(pool),
        Err(error)
            if error
                .downcast_ref::<Status>()
                .is_some_and(|status| status.code() == Code::Unimplemented) =>
        {
            reflection_session(
                channel,
                metadata,
                "/grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo",
            )
            .await
        }
        Err(error) => Err(error),
    }
    .context("Could not load server reflection. Import a .proto file if reflection is unavailable")
}

async fn reflection_session(
    channel: Channel,
    metadata: MetadataMap,
    path: &'static str,
) -> Result<DescriptorPool> {
    let mut client = Grpc::new(channel);
    client.ready().await?;
    let (sender, receiver) = mpsc::unbounded();
    // Some servers wait for the first request before sending response headers.
    send_reflection_query(&sender, MessageRequest::ListServices(String::new()))?;
    let mut request = tonic::Request::new(receiver);
    *request.metadata_mut() = metadata;
    let mut replies = client
        .streaming(
            request,
            PathAndQuery::from_static(path),
            tonic_prost::ProstCodec::<ServerReflectionRequest, ServerReflectionResponse>::default(),
        )
        .await?
        .into_inner();

    let MessageResponse::ListServicesResponse(list) = reflection_response(&mut replies).await?
    else {
        bail!("Invalid list-services response")
    };
    let mut files = BTreeMap::new();
    for service in list.service {
        if !service.name.starts_with("grpc.reflection.") {
            reflection_files(
                &sender,
                &mut replies,
                MessageRequest::FileContainingSymbol(service.name),
                &mut files,
            )
            .await?;
        }
    }
    let mut requested = BTreeSet::new();
    loop {
        let missing = files
            .values()
            .flat_map(|file| &file.dependency)
            .filter(|name| !files.contains_key(*name))
            .cloned()
            .collect::<BTreeSet<_>>();
        if missing.is_empty() {
            break;
        }
        for name in missing {
            ensure!(
                requested.insert(name.clone()),
                "Reflection did not return dependency {name}"
            );
            reflection_files(
                &sender,
                &mut replies,
                MessageRequest::FileByFilename(name),
                &mut files,
            )
            .await?;
        }
    }
    reflection_schema::build_pool(files)
}

fn send_reflection_query(
    sender: &mpsc::UnboundedSender<ServerReflectionRequest>,
    query: MessageRequest,
) -> Result<()> {
    sender.unbounded_send(ServerReflectionRequest {
        host: String::new(),
        message_request: Some(query),
    })?;
    Ok(())
}

async fn reflection_response(
    replies: &mut Streaming<ServerReflectionResponse>,
) -> Result<MessageResponse> {
    match replies
        .message()
        .await?
        .and_then(|message| message.message_response)
    {
        Some(MessageResponse::ErrorResponse(error)) => {
            Err(Status::new(Code::from_i32(error.error_code), error.error_message).into())
        }
        Some(response) => Ok(response),
        None => bail!("Reflection ended before returning a schema"),
    }
}

async fn reflection_files(
    sender: &mpsc::UnboundedSender<ServerReflectionRequest>,
    replies: &mut Streaming<ServerReflectionResponse>,
    query: MessageRequest,
    files: &mut BTreeMap<String, prost_types::FileDescriptorProto>,
) -> Result<()> {
    send_reflection_query(sender, query)?;
    let MessageResponse::FileDescriptorResponse(response) = reflection_response(replies).await?
    else {
        bail!("Invalid file descriptor response")
    };
    for bytes in response.file_descriptor_proto {
        let file = prost_types::FileDescriptorProto::decode(bytes.as_slice())?;
        files.insert(file.name().to_owned(), file);
    }
    Ok(())
}

struct MessageCodec(MessageDescriptor);

impl Codec for MessageCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = MessageEncoder;
    type Decoder = MessageDecoder;
    fn encoder(&mut self) -> Self::Encoder {
        MessageEncoder
    }
    fn decoder(&mut self) -> Self::Decoder {
        MessageDecoder(self.0.clone())
    }
}

struct MessageEncoder;

impl Encoder for MessageEncoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn encode(&mut self, message: DynamicMessage, buf: &mut EncodeBuf<'_>) -> Result<(), Status> {
        message
            .encode(buf)
            .map_err(|error| Status::internal(error.to_string()))
    }
}

struct MessageDecoder(MessageDescriptor);

impl Decoder for MessageDecoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn decode(&mut self, buf: &mut DecodeBuf<'_>) -> Result<Option<DynamicMessage>, Status> {
        DynamicMessage::decode(self.0.clone(), buf)
            .map(Some)
            .map_err(|error| Status::internal(error.to_string()))
    }
}

fn parse_message(descriptor: MessageDescriptor, text: &str) -> Result<DynamicMessage> {
    let text = text.trim();
    let mut json = serde_json::Deserializer::from_str(if text.is_empty() { "{}" } else { text });
    let message = DynamicMessage::deserialize(descriptor, &mut json)
        .context("Invalid Protobuf JSON message")?;
    json.end().context("Enter one JSON message at a time")?;
    Ok(message)
}

fn json_message(message: &DynamicMessage, include_defaults: bool) -> Result<String> {
    let options = SerializeOptions::new().skip_default_fields(!include_defaults);
    let mut bytes = Vec::new();
    message.serialize_with_options(&mut serde_json::Serializer::pretty(&mut bytes), &options)?;
    Ok(String::from_utf8(bytes)?)
}

pub struct Call {
    pub events: mpsc::Sender<Event>,
    pub messages: tokio::sync::mpsc::Receiver<String>,
    pub cancel: oneshot::Receiver<()>,
    pub known_schema: Option<DescriptorPool>,
}

impl Call {
    pub fn run(
        self,
        request: Request,
        globals: &[Variable],
        scoped: &[Variable],
        on_message: Option<&mut crate::scripts::MessageObserver<'_>>,
    ) -> Result<Response> {
        let Self {
            mut events,
            messages,
            mut cancel,
            known_schema,
        } = self;
        let options = request.grpc.as_ref().context("Expected a gRPC request")?;
        let started = Instant::now();
        runtime()?.block_on(async {
            let call = invoke(
                &request,
                options,
                known_schema,
                messages,
                globals.iter().chain(scoped).cloned().collect(),
                &mut events,
                started,
                on_message,
            );
            let run = async {
                if options.timeout_ms == 0 {
                    call.await
                } else {
                    match tokio::time::timeout(Duration::from_millis(options.timeout_ms), call)
                        .await
                    {
                        Ok(response) => response,
                        Err(_) => Ok(empty_response(
                            Status::deadline_exceeded("The deadline expired"),
                            started,
                        )),
                    }
                }
            };
            tokio::select! {
                biased;
                _ = &mut cancel => Ok(empty_response(Status::cancelled("Cancelled"), started)),
                response = run => response,
            }
        })
    }
}

fn outgoing_messages(
    receiver: tokio::sync::mpsc::Receiver<String>,
    descriptor: MessageDescriptor,
    variables: Vec<Variable>,
    events: mpsc::Sender<Event>,
) -> impl Stream<Item = DynamicMessage> + Send + 'static {
    stream::unfold(
        (receiver, descriptor, variables, events),
        |mut state| async {
            while let Some(text) = state.0.recv().await {
                let resolved = strip_json_comments(&text)
                    .and_then(|body| resolve_text(&body, state.2.iter().cloned()));
                match resolved.and_then(|body| parse_message(state.1.clone(), &body)) {
                    Ok(message) => {
                        if let Ok(body) = json_message(&message, false) {
                            _ = state.3.send(Event::Sent(body)).await;
                        }
                        return Some((message, state));
                    }
                    Err(error) => {
                        _ = state.3.send(Event::InputError(format!("{error:#}"))).await;
                    }
                }
            }
            None
        },
    )
}

async fn invoke(
    request: &Request,
    options: &GrpcOptions,
    known_schema: Option<DescriptorPool>,
    messages: tokio::sync::mpsc::Receiver<String>,
    variables: Vec<Variable>,
    events: &mut mpsc::Sender<Event>,
    started: Instant,
    mut on_message: Option<&mut crate::scripts::MessageObserver<'_>>,
) -> Result<Response> {
    ensure!(!request.method.trim().is_empty(), "Select a gRPC method");
    let channel = connect(request, options).await?;
    let metadata = metadata(request)?;
    let pool = match (known_schema, &options.proto) {
        (Some(pool), _) => pool,
        (None, Some(proto)) => local_pool(proto)?,
        (None, None) => tokio::time::timeout(
            DISCOVERY_TIMEOUT,
            reflect(channel.clone(), metadata.clone()),
        )
        .await
        .context("Server reflection timed out")??,
    };
    let method = methods(&pool)
        .into_iter()
        .find(|method| method_path(method) == request.method.trim().trim_start_matches('/'))
        .with_context(|| format!("The schema does not contain {}", request.method))?;
    let client_streaming = method.is_client_streaming();
    let streaming = client_streaming || method.is_server_streaming();
    let mut client = Grpc::new(channel)
        .accept_compressed(CompressionEncoding::Gzip)
        .max_decoding_message_size(match options.max_response_bytes {
            Some(0) => usize::MAX,
            Some(limit) => limit,
            None => 4 * 1024 * 1024,
        });
    client.ready().await?;
    let mut outgoing: tonic::Request<std::pin::Pin<Box<dyn Stream<Item = DynamicMessage> + Send>>> =
        if client_streaming {
            tonic::Request::new(Box::pin(outgoing_messages(
                messages,
                method.input(),
                variables,
                events.clone(),
            )))
        } else {
            let message = parse_message(method.input(), &request.body)?;
            if let Ok(body) = json_message(&message, false) {
                _ = events.send(Event::Sent(body)).await;
            }
            tonic::Request::new(Box::pin(stream::iter([message])))
        };
    *outgoing.metadata_mut() = metadata;
    if options.timeout_ms != 0 {
        outgoing.set_timeout(
            Duration::from_millis(options.timeout_ms).saturating_sub(started.elapsed()),
        );
    }
    _ = events
        .send(Event::Ready {
            client_streaming,
            streaming,
        })
        .await;
    let path = format!("/{}", method_path(&method)).parse()?;
    let incoming = client
        .streaming(outgoing, path, MessageCodec(method.output()))
        .await;
    let incoming = match incoming {
        Ok(incoming) => incoming,
        Err(status) => return Ok(empty_response(status, started)),
    };
    let headers = metadata_text(incoming.metadata());
    _ = events.send(Event::Metadata(headers.clone())).await;
    let mut incoming = incoming.into_inner();
    let mut response = empty_response(Status::new(Code::Ok, ""), started);
    response.grpc_server_streaming = Some(method.is_server_streaming());
    response.headers = headers;
    let mut bodies = Vec::new();
    let mut retained_bytes = 0;
    loop {
        match incoming.message().await {
            Ok(Some(message)) => {
                response.body_bytes += message.encoded_len();
                let json = json_message(&message, options.include_default_fields)?;
                if streaming {
                    _ = events.send(Event::Received(json.clone())).await;
                }
                if let Some(observe) = &mut on_message {
                    observe(&json);
                }
                if !method.is_server_streaming()
                    || retained_bytes + json.len() <= MAX_RETAINED_BYTES
                {
                    retained_bytes += json.len();
                    bodies.push(json);
                }
            }
            Ok(None) => {
                if let Some(trailers) = incoming.trailers().await? {
                    response.trailers = metadata_text(&trailers);
                }
                break;
            }
            Err(status) => {
                set_status(&mut response, &status);
                break;
            }
        }
    }
    response.elapsed = started.elapsed();
    if method.is_server_streaming() && (response.status == 0 || !bodies.is_empty()) {
        response.body = format!("[\n{}\n]", bodies.join(",\n")).into();
        response.is_json = true;
    } else if let Some(body) = bodies.into_iter().next() {
        response.body = body.into();
        response.is_json = true;
    }
    response.raw_body = response.body.clone();
    Ok(response)
}

fn empty_response(status: Status, started: Instant) -> Response {
    let mut response = Response {
        status: 0,
        reason: "",
        elapsed: started.elapsed(),
        body_bytes: 0,
        is_json: false,
        body: "".into(),
        raw_body: "".into(),
        headers: String::new(),
        trailers: String::new(),
        grpc_server_streaming: Some(false),
    };
    set_status(&mut response, &status);
    response
}

fn set_status(response: &mut Response, status: &Status) {
    const NAMES: [&str; 17] = [
        "OK",
        "CANCELLED",
        "UNKNOWN",
        "INVALID_ARGUMENT",
        "DEADLINE_EXCEEDED",
        "NOT_FOUND",
        "ALREADY_EXISTS",
        "PERMISSION_DENIED",
        "RESOURCE_EXHAUSTED",
        "FAILED_PRECONDITION",
        "ABORTED",
        "OUT_OF_RANGE",
        "UNIMPLEMENTED",
        "INTERNAL",
        "UNAVAILABLE",
        "DATA_LOSS",
        "UNAUTHENTICATED",
    ];
    response.status = status.code() as u16;
    response.reason = NAMES[response.status as usize];
    response.trailers = metadata_text(status.metadata());
    let append_trailer = |trailers: &mut String, key: &str, value: &str| {
        if !trailers.is_empty() {
            trailers.push('\n');
        }
        trailers.push_str(key);
        trailers.push_str(": ");
        trailers.push_str(value);
    };
    if !status.details().is_empty() {
        append_trailer(
            &mut response.trailers,
            "grpc-status-details-bin",
            &STANDARD.encode(status.details()),
        );
    }
    if !status.message().is_empty() {
        append_trailer(&mut response.trailers, "grpc-message", status.message());
        response.body = status.message().into();
        response.raw_body = response.body.clone();
    }
}

#[cfg(test)]
mod tests;
