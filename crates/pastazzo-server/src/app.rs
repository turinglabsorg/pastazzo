//! The HTTP API (see `docs/PROTOCOL.md`).
//!
//! The server only checks what it can check without being able to read
//! anything: OPAQUE logins, invite proofs, device bindings and request
//! signatures. Items and device records are stored and passed on as they
//! arrive; only devices can open them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use pastazzo_core::account::WrappedAccountKey;
use pastazzo_core::api::{
    self, B64, DeviceRecords, ErrorBody, ItemPosted, ItemsPage, LoginFinish, LoginFinished,
    LoginStart, LoginStarted, RegisterFinish, RegisterStart, RegisterStarted, Registration,
    ServerInfo,
};
use pastazzo_core::device::{DevicePublic, SealedDeviceRecord};
use pastazzo_core::invite::InviteVerifier;
use pastazzo_core::item::{MAX_IMAGE_BYTES, SealedItem};
use pastazzo_core::opaque::{self, ServerLoginState};
use pastazzo_core::registration::RegistrationRecord;
use pastazzo_core::request::{MAX_CLOCK_SKEW_MS, RequestSignature, Scope};
use pastazzo_core::server::{ServerIdentity, ServerKeys};
use pastazzo_core::{Id, random_id, session};
use rand::rngs::OsRng;
use serde::Deserialize;
use tokio::sync::Notify;

use crate::store::{Insert, Store};

/// Largest request body: one item at the image limit, padded, plus headers.
const MAX_BODY_BYTES: usize = MAX_IMAGE_BYTES + 2 * 1024 * 1024;
/// A page of items stops growing past this many bytes of ciphertext.
const MAX_PAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_PAGE_ITEMS: usize = 50;
/// How long a started registration or login may wait for its second step.
const PENDING_TTL_MS: u64 = 2 * 60 * 1000;
/// Registration and login attempts allowed per username in [`ATTEMPT_WINDOW_MS`].
const MAX_ATTEMPTS: usize = 20;
const ATTEMPT_WINDOW_MS: u64 = 10 * 60 * 1000;

pub struct App {
    keys: ServerKeys,
    identity: ServerIdentity,
    fingerprint: [u8; 32],
    registration: Registration,
    store: Mutex<Store>,
    registrations: Mutex<HashMap<Id, PendingRegistration>>,
    logins: Mutex<HashMap<Id, PendingLogin>>,
    nonces: Mutex<HashMap<(Id, [u8; 16]), u64>>,
    attempts: Mutex<HashMap<String, Vec<u64>>>,
    waiters: Mutex<HashMap<Id, Arc<Notify>>>,
}

struct PendingRegistration {
    username: String,
    invite: Option<InviteVerifier>,
    expires_at: u64,
}

struct PendingLogin {
    username: String,
    account: Option<(Id, Vec<u8>)>,
    state: ServerLoginState,
    expires_at: u64,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_millis() as u64
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding a lock leaves plain data behind, nothing half
    // written: keep serving.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(ErrorBody {
                error: self.1.to_owned(),
            }),
        )
            .into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        eprintln!("pastazzo-server: storage error: {error}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "storage error")
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn bad(message: &'static str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message)
}

fn id_from(value: &B64, what: &'static str) -> ApiResult<Id> {
    value.array().ok_or(bad(what))
}

impl App {
    pub fn new(keys: ServerKeys, store: Store, registration: Registration) -> Self {
        let identity = keys.identity();
        Self {
            fingerprint: identity.fingerprint(),
            identity,
            keys,
            registration,
            store: Mutex::new(store),
            registrations: Mutex::default(),
            logins: Mutex::default(),
            nonces: Mutex::default(),
            attempts: Mutex::default(),
            waiters: Mutex::default(),
        }
    }

    fn allow_attempt(&self, username: &str, now: u64) -> ApiResult<()> {
        let mut attempts = lock(&self.attempts);
        attempts.retain(|_, times| {
            times.retain(|&t| now.saturating_sub(t) < ATTEMPT_WINDOW_MS);
            !times.is_empty()
        });
        let times = attempts.entry(username.to_owned()).or_default();
        if times.len() >= MAX_ATTEMPTS {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "too many attempts, try again later",
            ));
        }
        times.push(now);
        Ok(())
    }

    fn waiter(&self, account: &Id) -> Arc<Notify> {
        lock(&self.waiters).entry(*account).or_default().clone()
    }

    /// Checks a signed request and returns the account and device it comes
    /// from.
    fn authenticate(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &[u8],
    ) -> ApiResult<(Id, Id)> {
        let unauthorized = ApiError(StatusCode::UNAUTHORIZED, "invalid request signature");
        let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        let decode = |name: &str| header(name).and_then(B64::decode);
        let device_id: Id = decode(api::HEADER_DEVICE)
            .and_then(|v| v.try_into().ok())
            .ok_or(bad("missing signature headers"))?;
        let nonce: [u8; 16] = decode(api::HEADER_NONCE)
            .and_then(|v| v.try_into().ok())
            .ok_or(bad("missing signature headers"))?;
        let signature: [u8; 64] = decode(api::HEADER_SIGNATURE)
            .and_then(|v| v.try_into().ok())
            .ok_or(bad("missing signature headers"))?;
        let timestamp: u64 = header(api::HEADER_TIMESTAMP)
            .and_then(|v| v.parse().ok())
            .ok_or(bad("missing signature headers"))?;

        let device = lock(&self.store)
            .device(&device_id)?
            .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unknown device"))?;
        if device.revoked {
            return Err(ApiError(StatusCode::UNAUTHORIZED, "device revoked"));
        }
        let public = DevicePublic::from_bytes(&device.public).map_err(|_| unauthorized)?;
        let scope = Scope {
            server_fingerprint: self.fingerprint,
            account: device.account,
        };
        let path = uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or_else(|| uri.path());
        let now = now();
        RequestSignature {
            device: device_id,
            timestamp,
            nonce,
            signature,
        }
        .verify(&public, &scope, method.as_str(), path, body, now)
        .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "invalid request signature"))?;

        // Only after the signature checks out, so nobody can fill the cache.
        let mut nonces = lock(&self.nonces);
        nonces.retain(|_, &mut seen| now.saturating_sub(seen) <= 2 * MAX_CLOCK_SKEW_MS);
        if nonces.insert((device_id, nonce), now).is_some() {
            return Err(ApiError(StatusCode::UNAUTHORIZED, "replayed request"));
        }
        Ok((device.account, device_id))
    }
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/v1/server", get(server_info))
        .route("/v1/register/start", post(register_start))
        .route("/v1/register/finish", post(register_finish))
        .route("/v1/login/start", post(login_start))
        .route("/v1/login/finish", post(login_finish))
        .route("/v1/devices", get(list_devices))
        .route("/v1/devices/{id}", put(put_device).delete(revoke_device))
        .route("/v1/items", post(post_item).get(get_items))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(app)
}

async fn server_info(State(app): State<Arc<App>>) -> Json<ServerInfo> {
    Json(ServerInfo {
        version: pastazzo_core::PROTOCOL_VERSION,
        identity: B64(app.identity.to_bytes().to_vec()),
        registration: app.registration,
    })
}

async fn register_start(
    State(app): State<Arc<App>>,
    Json(body): Json<RegisterStart>,
) -> ApiResult<Json<RegisterStarted>> {
    if app.registration == Registration::Closed {
        return Err(ApiError(StatusCode::FORBIDDEN, "registration is closed"));
    }
    opaque::validate_username(&body.username).map_err(|_| bad("invalid username"))?;
    let now = now();
    app.allow_attempt(&body.username, now)?;

    let invite = match (&body.invite_id, &body.proof) {
        (Some(id), Some(proof)) => {
            let id = id_from(id, "invalid invite id")?;
            let invite_error = ApiError(StatusCode::FORBIDDEN, "invalid or expired invite");
            let stored = lock(&app.store).invite(&id)?.ok_or(invite_error)?;
            if stored.used || stored.expires_at <= now {
                return Err(ApiError(StatusCode::FORBIDDEN, "invalid or expired invite"));
            }
            let verifier = InviteVerifier {
                id,
                public: stored.public,
            };
            verifier
                .verify_start_proof(&body.username, &body.request.0, &proof.0)
                .map_err(|_| ApiError(StatusCode::FORBIDDEN, "invalid or expired invite"))?;
            Some(verifier)
        }
        (None, None) if app.registration == Registration::Open => None,
        _ => return Err(ApiError(StatusCode::FORBIDDEN, "an invite is required")),
    };
    if lock(&app.store).username_taken(&body.username)? {
        return Err(ApiError(StatusCode::CONFLICT, "username taken"));
    }

    let response = opaque::server_registration_start(&app.keys, &body.request.0, &body.username)
        .map_err(|_| bad("invalid registration request"))?;
    let registration_id = random_id(&mut OsRng);
    let mut pending = lock(&app.registrations);
    pending.retain(|_, p| p.expires_at > now);
    pending.insert(
        registration_id,
        PendingRegistration {
            username: body.username,
            invite,
            expires_at: now + PENDING_TTL_MS,
        },
    );
    Ok(Json(RegisterStarted {
        registration_id: B64(registration_id.to_vec()),
        response: B64(response.response),
        signature: B64(response.signature.to_vec()),
    }))
}

async fn register_finish(
    State(app): State<Arc<App>>,
    Json(body): Json<RegisterFinish>,
) -> ApiResult<StatusCode> {
    let now = now();
    let registration_id = id_from(&body.registration_id, "invalid registration id")?;
    let pending = lock(&app.registrations)
        .remove(&registration_id)
        .filter(|p| p.expires_at > now)
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "unknown or expired registration",
        ))?;

    if let Some(invite) = &pending.invite {
        let proof = body
            .proof
            .as_ref()
            .ok_or(ApiError(StatusCode::FORBIDDEN, "invalid invite proof"))?;
        invite
            .verify_finish_proof(&pending.username, &body.sealed.0, &proof.0)
            .map_err(|_| ApiError(StatusCode::FORBIDDEN, "invalid invite proof"))?;
    }
    let record = RegistrationRecord::open(&app.keys, &body.sealed.0)
        .map_err(|_| bad("invalid registration record"))?;
    if record.username != pending.username {
        return Err(bad("invalid registration record"));
    }
    let password_file = opaque::server_registration_finish(&record.upload)
        .map_err(|_| bad("invalid registration record"))?;
    let created = lock(&app.store).create_account(
        &record.account,
        &record.username,
        &password_file,
        &record.wrapped.to_bytes(),
        pending.invite.as_ref().map(|invite| &invite.id),
        now,
    )?;
    match created {
        Insert::Done => Ok(StatusCode::NO_CONTENT),
        Insert::Conflict => Err(ApiError(
            StatusCode::CONFLICT,
            "username taken or invite already used",
        )),
    }
}

async fn login_start(
    State(app): State<Arc<App>>,
    Json(body): Json<LoginStart>,
) -> ApiResult<Json<LoginStarted>> {
    opaque::validate_username(&body.username).map_err(|_| bad("invalid username"))?;
    let now = now();
    app.allow_attempt(&body.username, now)?;
    let account = lock(&app.store).account_by_username(&body.username)?;
    // Unknown usernames get a fake response that looks like a real one.
    let (response, state) = opaque::server_login_start(
        &mut OsRng,
        &app.keys,
        account.as_ref().map(|a| a.password_file.as_slice()),
        &body.request.0,
        &body.username,
    )
    .map_err(|_| bad("invalid login request"))?;
    let login_id = random_id(&mut OsRng);
    let mut pending = lock(&app.logins);
    pending.retain(|_, p| p.expires_at > now);
    pending.insert(
        login_id,
        PendingLogin {
            username: body.username,
            account: account.map(|a| (a.id, a.wrapped_key)),
            state,
            expires_at: now + PENDING_TTL_MS,
        },
    );
    Ok(Json(LoginStarted {
        login_id: B64(login_id.to_vec()),
        response: B64(response),
    }))
}

async fn login_finish(
    State(app): State<Arc<App>>,
    Json(body): Json<LoginFinish>,
) -> ApiResult<Json<LoginFinished>> {
    let now = now();
    let login_id = id_from(&body.login_id, "invalid login id")?;
    let pending = lock(&app.logins)
        .remove(&login_id)
        .filter(|p| p.expires_at > now)
        .ok_or(ApiError(StatusCode::NOT_FOUND, "unknown or expired login"))?;
    let failed = ApiError(StatusCode::UNAUTHORIZED, "login failed");
    let session_key = pending
        .state
        .finish(&body.finalization.0)
        .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "login failed"))?;
    let (account, wrapped) = pending.account.ok_or(failed)?;

    let device =
        DevicePublic::from_bytes(&body.device.0).map_err(|_| bad("invalid device keys"))?;
    session::verify_device_binding(&session_key, &pending.username, &device, &body.binding.0)
        .map_err(|_| ApiError(StatusCode::UNAUTHORIZED, "login failed"))?;
    if let Insert::Conflict =
        lock(&app.store).add_device(&device.id, &account, &device.to_bytes(), now)?
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "device id already registered",
        ));
    }

    let wrapped = WrappedAccountKey::from_bytes(&wrapped)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "corrupt account"))?;
    let tag = session::login_response_tag(&session_key, &pending.username, &account, &wrapped);
    Ok(Json(LoginFinished {
        account: B64(account.to_vec()),
        wrapped: B64(wrapped.to_bytes()),
        tag: B64(tag.to_vec()),
    }))
}

async fn put_device(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let (_, device_id) = app.authenticate(&method, &uri, &headers, &body)?;
    if B64::decode(&id).as_deref() != Some(&device_id[..]) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "a device can only publish its own record",
        ));
    }
    let record = SealedDeviceRecord::from_bytes(&body).map_err(|_| bad("invalid device record"))?;
    let store = lock(&app.store);
    let registered = store
        .device(&device_id)?
        .map(|d| d.public)
        .unwrap_or_default();
    if record.device.to_bytes()[..] != registered[..] {
        return Err(bad("the record isn't for this device's keys"));
    }
    store.set_device_record(&device_id, &body)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_devices(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<Json<DeviceRecords>> {
    let (account, _) = app.authenticate(&method, &uri, &headers, b"")?;
    let records = lock(&app.store).device_records(&account)?;
    Ok(Json(DeviceRecords {
        records: records.into_iter().map(B64).collect(),
    }))
}

async fn revoke_device(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let (account, _) = app.authenticate(&method, &uri, &headers, b"")?;
    let id: Id = B64::decode(&id)
        .and_then(|v| v.try_into().ok())
        .ok_or(bad("invalid device id"))?;
    if lock(&app.store).revoke_device(&id, &account, now())? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "no such device"))
    }
}

async fn post_item(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<ItemPosted>> {
    let (account, device_id) = app.authenticate(&method, &uri, &headers, &body)?;
    let item = SealedItem::from_bytes(&body).map_err(|_| bad("invalid item"))?;
    if item.header.device != device_id {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "a device can only send its own items",
        ));
    }
    let cursor = lock(&app.store)
        .add_item(&account, &item.header.id, &device_id, &body, now())?
        .ok_or(ApiError(StatusCode::CONFLICT, "item already received"))?;
    app.waiter(&account).notify_waiters();
    Ok(Json(ItemPosted { cursor }))
}

#[derive(Deserialize)]
struct ItemsQuery {
    after: Option<String>,
    wait: Option<u64>,
}

async fn get_items(
    State(app): State<Arc<App>>,
    Query(query): Query<ItemsQuery>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<Json<ItemsPage>> {
    let (account, _) = app.authenticate(&method, &uri, &headers, b"")?;
    let after = match query.after.as_deref() {
        // A new device only wants what comes next.
        Some("latest") => {
            let cursor = lock(&app.store).latest_cursor(&account)?;
            return Ok(Json(ItemsPage {
                cursor,
                items: Vec::new(),
            }));
        }
        Some(after) => after.parse().map_err(|_| bad("invalid cursor"))?,
        None => 0,
    };
    let wait = Duration::from_secs(query.wait.unwrap_or(0).min(api::MAX_WAIT_SECONDS));

    // Subscribe before looking, so an item arriving in between isn't missed.
    let waiter = app.waiter(&account);
    let notified = waiter.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();

    let mut items = lock(&app.store).items_after(&account, after, MAX_PAGE_ITEMS)?;
    if items.is_empty() && !wait.is_zero() {
        let _ = tokio::time::timeout(wait, notified).await;
        items = lock(&app.store).items_after(&account, after, MAX_PAGE_ITEMS)?;
    }

    let mut cursor = after;
    let mut size = 0;
    let mut page = Vec::new();
    for (seq, data) in items {
        if !page.is_empty() && size + data.len() > MAX_PAGE_BYTES {
            break;
        }
        size += data.len();
        cursor = seq;
        page.push(B64(data));
    }
    Ok(Json(ItemsPage {
        cursor,
        items: page,
    }))
}
