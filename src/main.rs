use async_nats::rustls::crypto::ring::sign;
use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{FromRequest, OriginalUri, State},
    http::{HeaderValue, Request, Response, StatusCode, Uri, header::AUTHORIZATION},
    routing::get,
};
use axum_extra::{
    TypedHeader,
    extract::Host,
    headers::{
        Authorization,
        authorization::{Bearer, Credentials},
    },
};
use clap::Parser;

mod signature;

const MAX_READ_SIZE: usize = 16 * 1024 * 1024; // 16 MB

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Admin access key
    #[arg(long, env = "NATS_S3_ADMIN_ACCESS_KEY")]
    admin_access_key: String,
    /// Admin secret key
    #[arg(long, env = "NATS_S3_ADMIN_SECRET_KEY")]
    admin_secret_key: String,

    /// The port to listen on
    #[arg(short = 'p', long, default_value_t = 9514)]
    listening_port: u16,

    /// The NATS server address
    #[arg(long, env = "NATS_S3_NATS_ADDRESS")]
    nats_address: String,
}

#[derive(Clone)]
struct AppState {
    nats_client: async_nats::Client,
}

#[derive(Clone, Debug)]
struct AuthInfo {
    access_key: String,
    username: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // dbg!(args);
    let nats_client = async_nats::connect(&args.nats_address).await?;
    let app_state = AppState { nats_client };
    let app = Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/protected/", get(protected_handler))
        .fallback(fallback)
        .layer(axum::middleware::from_fn_with_state(
            app_state.clone(),
            s3_middleware,
        ))
        .with_state(app_state);

    // run our app with hyper, listening globally on port 9514
    let address = format!("0.0.0.0:{}", args.listening_port);
    let listener = tokio::net::TcpListener::bind(&address).await.unwrap();
    axum::serve(listener, app).await?;

    Ok(())
}

async fn s3_middleware(
    State(state): State<AppState>,
    Host(host): Host,
    OriginalUri(original_uri): OriginalUri,
    // Bytes(bytes): Bytes,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    dbg!(&req);
    dbg!(&host);
    // maybe map the host to an external host based on some config
    // also figure out how to check https vs http
    let external_host = &host;
    let (parts, body) = req.into_parts();

    if let Some(s3_params) = signature::parse_from_headers(&parts.headers) {
        dbg!(&s3_params);
        if let Some(secret_key) =
            fetch_secret_key(state.nats_client, &host, s3_params.access_key).await
        {
            let bytes = match axum::body::to_bytes(body, MAX_READ_SIZE).await  {
                    Ok(b) => b,
                    Err(e) => {
                        return Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .body(Body::from(format!("Failed to read body: {}", e)))
                            .unwrap();
                }};

            dbg!(signature::verify_headers(
                &parts.headers,
                &s3_params,
                &parts.method,
                &dbg!(format!("http://{external_host}{original_uri}")),
                &secret_key,
                &bytes,
            ));

            let auth = AuthInfo {
                access_key: s3_params.access_key.to_string(),
                username: Some("admin".to_string()),
            };
            let mut req2 = Request::from_parts(parts, Body::from(bytes));
            req2.extensions_mut().insert(auth);
            return next.run(req2).await;
        };
    }

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .body(Body::from("Unauthorized"))
        .unwrap()
}

async fn fetch_secret_key(
    nats_client: async_nats::Client,
    host: &str,
    access_key: &str,
) -> Option<String> {
    let subject = format_auth_subject(host);
    let jetstream = async_nats::jetstream::new(nats_client);
    dbg!("HERE!", &subject);
    let store = jetstream.get_key_value(subject).await.ok()?;
    let secret_key = store.get(access_key).await.ok()??;

    Some(String::from_utf8(secret_key.to_vec()).ok()?)
}

fn format_auth_subject(host: &str) -> String {
    format!("s3-auth-{}", host.replace(':', "_"))
}

async fn protected_handler(
    State(_state): State<AppState>,
    Extension(auth): Extension<AuthInfo>,
) -> String {
    dbg!(format!("Hello, authenticated user: {:?}", auth.access_key))
}

async fn fallback(uri: Uri) -> (StatusCode, String) {
    dbg!(&uri);
    (StatusCode::NOT_FOUND, format!("No route for {uri}"))
}
