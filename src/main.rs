use async_nats::{
    jetstream::{kv::Store, object_store},
    rustls::crypto::ring::sign,
};
use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{FromRequest, OriginalUri, Path, State},
    http::{HeaderValue, Request, Response, StatusCode, Uri, header::AUTHORIZATION},
    routing::{get, put},
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
use futures_util::{StreamExt, TryStreamExt};
use std::{borrow::Cow, collections::HashMap};
use time::format_description::well_known::Rfc3339;

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
    host_mapping: HashMap<String, String>,
}

#[derive(Clone, Debug)]
struct AuthInfo {
    namespace: String,
    access_key: String,
    username: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt::init();

    // dbg!(args);
    let nats_client = async_nats::connect(&args.nats_address).await?;
    let app_state = AppState {
        nats_client,
        host_mapping: HashMap::new(),
    };
    let app = Router::new()
        // .route("/", get(|| async { "Hello, World!" }))
        // .route("/protected/", get(protected_handler))
        .route("/", get(list_buckets))
        .route("/{bucket}/", put(create_bucket))
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

fn map_host_to_external<'a>(host: &str, mapping: &'a HashMap<String, String>) -> Cow<'a, str> {
    if let Some(external) = mapping.get(host) {
        Cow::from(external)
    } else if host.starts_with("localhost") {
        Cow::from(format!("http://{host}"))
    } else {
        Cow::from(format!("https://{host}"))
    }
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
    tracing::debug!("Incoming request for host: {}", host);

    let external_host = map_host_to_external(&host, &state.host_mapping);
    tracing::debug!(
        "Incoming request mapped to external host: {}",
        external_host
    );
    let namespace = format_namespace(&external_host);
    tracing::debug!("Incoming request using namespace: {}", namespace);

    let (parts, body) = req.into_parts();

    if let Some(s3_params) = signature::parse_from_headers(&parts.headers) {
        if let Some(secret_key) =
            fetch_secret_key(state.nats_client, &namespace, s3_params.access_key).await
        {
            let bytes = match axum::body::to_bytes(body, MAX_READ_SIZE).await {
                Ok(b) => b,
                Err(e) => {
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Body::from(format!("Failed to read body: {}", e)))
                        .unwrap();
                }
            };

            if signature::verify_headers(
                &parts.headers,
                &s3_params,
                &parts.method,
                &format!("{external_host}{original_uri}"),
                &secret_key,
                &bytes,
            ) {
                let auth = AuthInfo {
                    namespace: namespace.to_string(),
                    access_key: s3_params.access_key.to_string(),
                    username: Some("admin".to_string()),
                };
                let mut req2 = Request::from_parts(parts, Body::from(bytes));
                req2.extensions_mut().insert(auth);
                return next.run(req2).await;
            }
        };
    }

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .body(Body::from("Unauthorized"))
        .unwrap()
}

async fn fetch_secret_key(
    nats_client: async_nats::Client,
    namespace: &str,
    access_key: &str,
) -> Option<String> {
    let subject = format_auth_subject(namespace);
    let jetstream = async_nats::jetstream::new(nats_client);
    dbg!("HERE!", &subject);
    let store = jetstream.get_key_value(subject).await.ok()?;
    let secret_key = store.get(access_key).await.ok()??;

    Some(String::from_utf8(secret_key.to_vec()).ok()?)
}

#[derive(Debug, bincode::Encode, bincode::Decode)]
struct BucketRequest {
    namespace: String,
    bucket: String,
    access_key: String,
    #[bincode(with_serde)]
    created_at: time::UtcDateTime,
}

impl BucketRequest {
    fn new(namespace: String, bucket: String, access_key: String) -> Self {
        Self {
            namespace,
            bucket,
            access_key,
            created_at: time::UtcDateTime::now(),
        }
    }
}

async fn create_or_get_key_value_store(
    nats_client: async_nats::Client,
    subject: &str,
) -> anyhow::Result<(async_nats::jetstream::Context, Store)> {
    let jetstream = async_nats::jetstream::new(nats_client.clone());

    match jetstream.get_key_value(subject).await {
        Ok(store) => Ok((jetstream, store)),
        Err(_) => {
            let cfg = async_nats::jetstream::kv::Config {
                bucket: subject.to_string(),
                ..Default::default()
            };
            let store = jetstream.create_key_value(cfg).await?;
            Ok((jetstream, store))
        }
    }
}

async fn put_bucket(nats_client: async_nats::Client, create: BucketRequest) -> anyhow::Result<()> {
    let subject = format_buckets_subject(&create.namespace);
    dbg!("PUT BUCKET SUBJECT", &subject);
    let (jetstream, store) = create_or_get_key_value_store(nats_client, &subject).await?;

    // check if bucket already exists
    if let Some(_) = store.get(&create.bucket).await? {
        return Ok(());
    }

    jetstream
        .create_object_store(object_store::Config {
            bucket: dbg!(format_bucket_subject(&create.namespace, &create.bucket)),
            ..Default::default()
        })
        .await?;

    let bucket = create.bucket.clone();
    let encoded = bincode::encode_to_vec(create, bincode::config::standard())?;
    store.put(bucket, Bytes::from(encoded)).await?;

    Ok(())
}

// move this to other file later
async fn list_buckets_from_nats(
    nats_client: async_nats::Client,
    namespace: &str,
) -> anyhow::Result<Vec<BucketRequest>> {
    let subject = format_buckets_subject(namespace);
    let jetstream = async_nats::jetstream::new(nats_client);
    let store = jetstream.get_key_value(subject).await?;

    let mut buckets = Vec::new();
    let mut entries = store.keys().await?;
    while let Ok(Some(entry)) = entries.try_next().await {
        if let Some(bucket_binary) = store.get(&entry).await? {
            let (bucket, _) =
                bincode::decode_from_slice(&bucket_binary, bincode::config::standard())?;
            buckets.push(bucket);
        };
    }

    Ok(buckets)
}

fn format_namespace(host: &str) -> String {
    host.replace("http://", "")
        .replace("https://", "")
        .replace(':', "_")
        .replace('.', "_")
}

fn format_auth_subject(namespace: &str) -> String {
    format!("s3-auth-{}", namespace)
}

fn format_buckets_subject(namespace: &str) -> String {
    format!("s3-bucket-{}", namespace)
}

fn format_bucket_subject(namespace: &str, bucket: &str) -> String {
    format!("s3-bucket-entry-{}_{}", namespace, bucket)
}

async fn create_bucket(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthInfo>,
    Path(bucket): Path<String>,
) -> axum::response::Response {
    dbg!(format!(
        "create bucket '{}' for: {:?}",
        bucket, auth.access_key
    ));
    let request = BucketRequest::new(auth.namespace.clone(), bucket, auth.access_key.clone());

    match put_bucket(state.nats_client, request).await {
        Ok(_) => axum::response::Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap(),
        Err(e) => {
            dbg!(&e);
            return axum::response::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Failed to create bucket: {}", e)))
                .unwrap();
        }
    }
}

async fn list_buckets(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthInfo>,
) -> String {
    // dbg!(format!("Hello, authenticated user: {:?}", auth.access_key))
    let buckets = match list_buckets_from_nats(state.nats_client, &auth.namespace).await {
        Ok(buckets) => buckets,
        Err(e) => {
            dbg!(&e);
            return format!("Failed to list buckets: {}", e);
        }
    };

    let bucket_xml = buckets
        .iter()
        .map(|bucket| {
            format!(
                r#"<Bucket>
         <CreationDate>{}</CreationDate>
         <Name>{}</Name>
      </Bucket>"#,
                bucket.created_at.format(&Rfc3339).unwrap(),
                bucket.bucket
            )
        })
        .collect::<Vec<String>>()
        .join("\n");

    dbg!(&bucket_xml);

    format!(
        r#"<ListAllMyBucketsResult>
   <Buckets>
    {bucket_xml}
   </Buckets>
   <Owner>
      <DisplayName>Account+Name</DisplayName>
      <ID>AIDACKCEVSQ6C2EXAMPLE</ID>
   </Owner>  
</ListAllMyBucketsResult>"#
    )
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
