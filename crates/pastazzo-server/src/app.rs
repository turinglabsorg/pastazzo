//! The HTTP API (see `docs/PROTOCOL.md`).
//!
//! The server only checks what it can check without being able to read
//! anything: OPAQUE logins, invite proofs, device bindings and request
//! signatures. Items and device records are stored and passed on as they
//! arrive; only devices can open them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use http_body_util::BodyExt;
use pastazzo_core::account::WrappedAccountKey;
use pastazzo_core::api::{
    self, B64, DeviceRecords, ErrorBody, ItemAnnounce, ItemPosted, ItemsPage, LoginFinish,
    LoginFinished, LoginStart, LoginStarted, PendingTransfer, RegisterFinish, RegisterStart,
    RegisterStarted, Registration, ServerInfo,
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
/// Announced uploads are forgotten after this long without progress.
const TRANSFER_TTL_MS: u64 = 10 * 60 * 1000;
const MAX_TRANSFERS_PER_ACCOUNT: usize = 8;
/// How often upload progress is passed on to the other devices.
const PROGRESS_INTERVAL_MS: u64 = 200;
const MAX_ATTEMPTS: usize = 20;
/// Registration and login attempts allowed in all, in [`ATTEMPT_WINDOW_MS`].
/// The relay in front of a server may hide clients' addresses, so this is the
/// limit that keeps a flood of made-up usernames from filling memory: every
/// started registration or login is an attempt.
const MAX_ATTEMPTS_OVERALL: usize = 600;
/// Bodies of the JSON endpoints are small; only items are big.
const MAX_JSON_BYTES: usize = 64 * 1024;
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
    attempts: Mutex<Attempts>,
    waiters: Mutex<HashMap<Id, Arc<Notify>>>,
    transfers: Mutex<HashMap<Id, Transfers>>,
}

#[derive(Default)]
struct Attempts {
    by_username: HashMap<String, Vec<u64>>,
    overall: std::collections::VecDeque<u64>,
}

/// An account's announced uploads, with a version that changes with them.
#[derive(Default)]
struct Transfers {
    version: u64,
    pending: HashMap<Id, Pending>,
}

struct Pending {
    device: Id,
    size: u64,
    received: u64,
    updated_at: u64,
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
            transfers: Mutex::default(),
        }
    }

    fn allow_attempt(&self, username: &str, now: u64) -> ApiResult<()> {
        let busy = ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "too many attempts, try again later",
        );
        let mut attempts = lock(&self.attempts);
        while attempts
            .overall
            .front()
            .is_some_and(|&t| now.saturating_sub(t) >= ATTEMPT_WINDOW_MS)
        {
            attempts.overall.pop_front();
        }
        if attempts.overall.len() >= MAX_ATTEMPTS_OVERALL {
            return Err(busy);
        }
        attempts.by_username.retain(|_, times| {
            times.retain(|&t| now.saturating_sub(t) < ATTEMPT_WINDOW_MS);
            !times.is_empty()
        });
        let times = attempts.by_username.entry(username.to_owned()).or_default();
        if times.len() >= MAX_ATTEMPTS {
            return Err(busy);
        }
        times.push(now);
        attempts.overall.push_back(now);
        Ok(())
    }

    fn waiter(&self, account: &Id) -> Arc<Notify> {
        lock(&self.waiters).entry(*account).or_default().clone()
    }

    /// Changes an account's transfers and wakes its devices' long polls.
    fn update_transfers(&self, account: &Id, change: impl FnOnce(&mut Transfers)) {
        {
            let mut transfers = lock(&self.transfers);
            let entry = transfers.entry(*account).or_default();
            change(entry);
            entry.version += 1;
        }
        self.waiter(account).notify_waiters();
    }

    fn announce(&self, account: &Id, device: &Id, item: &Id, size: u64) {
        let now = now();
        self.update_transfers(account, |transfers| {
            transfers
                .pending
                .retain(|_, p| now.saturating_sub(p.updated_at) < TRANSFER_TTL_MS);
            if transfers.pending.len() >= MAX_TRANSFERS_PER_ACCOUNT
                && let Some(oldest) = transfers
                    .pending
                    .iter()
                    .min_by_key(|(_, p)| p.updated_at)
                    .map(|(id, _)| *id)
            {
                transfers.pending.remove(&oldest);
            }
            transfers.pending.insert(
                *item,
                Pending {
                    device: *device,
                    size,
                    received: 0,
                    updated_at: now,
                },
            );
        });
    }

    fn is_announced(&self, account: &Id, device: &Id, item: &Id) -> bool {
        lock(&self.transfers)
            .get(account)
            .and_then(|t| t.pending.get(item))
            .is_some_and(|p| p.device == *device)
    }

    fn progress(&self, account: &Id, item: &Id, received: u64) {
        let now = now();
        self.update_transfers(account, |transfers| {
            if let Some(pending) = transfers.pending.get_mut(item) {
                pending.received = received.min(pending.size);
                pending.updated_at = now;
            }
        });
    }

    fn finish_transfer(&self, account: &Id, item: &Id) {
        self.update_transfers(account, |transfers| {
            transfers.pending.remove(item);
        });
    }

    fn transfers(&self, account: &Id) -> (u64, Vec<PendingTransfer>) {
        let now = now();
        let transfers = lock(&self.transfers);
        let Some(transfers) = transfers.get(account) else {
            return (0, Vec::new());
        };
        let pending = transfers
            .pending
            .iter()
            .filter(|(_, p)| now.saturating_sub(p.updated_at) < TRANSFER_TTL_MS)
            .map(|(item, p)| PendingTransfer {
                device: B64(p.device.to_vec()),
                item: B64(item.to_vec()),
                size: p.size,
                received: p.received,
            })
            .collect();
        (transfers.version, pending)
    }

    /// [`App::authenticate`], for a device that's been approved: the
    /// account's first one, or one another device approved.
    fn authenticate_approved(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &[u8],
    ) -> ApiResult<(Id, Id)> {
        let (account, device) = self.authenticate(method, uri, headers, body)?;
        if !lock(&self.store)
            .device(&device)?
            .is_some_and(|d| d.approved)
        {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "this device is waiting for approval",
            ));
        }
        Ok((account, device))
    }

    /// The account and device a signed request claims to come from, before
    /// its body has arrived: enough to refuse unknown or revoked devices
    /// early. [`App::authenticate`] still checks everything at the end.
    fn claimed_device(&self, headers: &HeaderMap) -> ApiResult<(Id, Id)> {
        let device_id: Id = headers
            .get(api::HEADER_DEVICE)
            .and_then(|v| v.to_str().ok())
            .and_then(B64::decode)
            .and_then(|v| v.try_into().ok())
            .ok_or(bad("missing signature headers"))?;
        let device = lock(&self.store)
            .device(&device_id)?
            .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unknown device"))?;
        if device.revoked {
            return Err(ApiError(StatusCode::UNAUTHORIZED, "device revoked"));
        }
        if !device.approved {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "this device is waiting for approval",
            ));
        }
        Ok((device.account, device_id))
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
    // Uploads read their body as a stream and enforce MAX_BODY_BYTES
    // themselves; everything else takes small bodies only.
    let items = Router::new().route(
        "/v1/items",
        post(post_item).get(get_items).delete(delete_items),
    );
    Router::new()
        .route("/v1/server", get(server_info))
        .route("/v1/register/start", post(register_start))
        .route("/v1/register/finish", post(register_finish))
        .route("/v1/login/start", post(login_start))
        .route("/v1/login/finish", post(login_finish))
        .route("/v1/devices", get(list_devices))
        .route("/v1/devices/{id}", put(put_device).delete(revoke_device))
        .route("/v1/devices/{id}/grant", put(put_grant).get(get_grant))
        .route("/v1/items/announce", post(announce_item))
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES))
        .merge(items)
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

    // The account's first device approves itself: nobody else is there to.
    lock(&app.store).approve_if_first(&device.id, &account, now)?;

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
    let (_, device_id) = app.authenticate_approved(&method, &uri, &headers, &body)?;
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
    let (account, _) = app.authenticate_approved(&method, &uri, &headers, b"")?;
    let store = lock(&app.store);
    let records = store.device_records(&account)?;
    let pending = store.pending_devices(&account, now())?;
    Ok(Json(DeviceRecords {
        records: records.into_iter().map(B64).collect(),
        pending: pending.into_iter().map(B64).collect(),
    }))
}

/// An approved device hands a pending one the account secret, sealed to it.
async fn put_grant(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let (account, _) = app.authenticate_approved(&method, &uri, &headers, &body)?;
    let id: Id = B64::decode(&id)
        .and_then(|v| v.try_into().ok())
        .ok_or(bad("invalid device id"))?;
    if body.len() > 512 {
        return Err(bad("invalid grant"));
    }
    if lock(&app.store).set_grant(&id, &account, &body, now())? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(
            StatusCode::NOT_FOUND,
            "no device waiting for approval with that id",
        ))
    }
}

/// A device collects the grant that approved it.
async fn get_grant(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<Bytes> {
    // Waiting devices may call this: it's how they stop waiting.
    let (_, device) = app.authenticate(&method, &uri, &headers, b"")?;
    if B64::decode(&id).as_deref() != Some(&device[..]) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "a device can only collect its own grant",
        ));
    }
    lock(&app.store)
        .grant(&device)?
        .map(Bytes::from)
        .ok_or(ApiError(StatusCode::NOT_FOUND, "not approved yet"))
}

async fn revoke_device(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let (account, _) = app.authenticate_approved(&method, &uri, &headers, b"")?;
    let id: Id = B64::decode(&id)
        .and_then(|v| v.try_into().ok())
        .ok_or(bad("invalid device id"))?;
    if lock(&app.store).revoke_device(&id, &account, now())? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "no such device"))
    }
}

async fn announce_item(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let (account, device_id) = app.authenticate_approved(&method, &uri, &headers, &body)?;
    let announce: ItemAnnounce =
        serde_json::from_slice(&body).map_err(|_| bad("invalid announcement"))?;
    let item = id_from(&announce.id, "invalid item id")?;
    if announce.size > MAX_BODY_BYTES as u64 {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "item too large"));
    }
    app.announce(&account, &device_id, &item, announce.size);
    Ok(StatusCode::NO_CONTENT)
}

/// Ends an announced transfer however the upload goes.
struct TransferGuard {
    app: Arc<App>,
    account: Id,
    item: Option<Id>,
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        if let Some(item) = self.item {
            self.app.finish_transfer(&self.account, &item);
        }
    }
}

/// Receives an item. The body streams in so that, for an announced item,
/// the other devices can follow its progress; nothing is stored before the
/// whole request, signature included, checks out.
async fn post_item(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    mut body: Body,
) -> ApiResult<Json<ItemPosted>> {
    let (account, device_id) = app.claimed_device(&headers)?;
    let mut guard = TransferGuard {
        app: app.clone(),
        account,
        item: None,
    };
    let mut data = Vec::new();
    let mut reported_at = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| bad("upload interrupted"))?;
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        if data.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "item too large"));
        }
        data.extend_from_slice(&chunk);
        // version(1) || item id(16) || device(16): enough to recognize the announcement.
        if guard.item.is_none() && data.len() >= 33 {
            let item: Id = data[1..17].try_into().expect("16 bytes");
            if app.is_announced(&account, &device_id, &item) {
                guard.item = Some(item);
            }
        }
        if let Some(item) = guard.item
            && now().saturating_sub(reported_at) >= PROGRESS_INTERVAL_MS
        {
            app.progress(&account, &item, data.len() as u64);
            reported_at = now();
        }
    }

    let (account, device_id) = app.authenticate_approved(&method, &uri, &headers, &data)?;
    let item = SealedItem::from_bytes(&data).map_err(|_| bad("invalid item"))?;
    if item.header.device != device_id {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "a device can only send its own items",
        ));
    }
    let cursor = lock(&app.store)
        .add_item(&account, &item.header.id, &device_id, &data, now())?
        .ok_or(ApiError(StatusCode::CONFLICT, "item already received"))?;
    drop(guard);
    app.waiter(&account).notify_waiters();
    Ok(Json(ItemPosted { cursor }))
}

async fn delete_items(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let (account, _) = app.authenticate_approved(&method, &uri, &headers, b"")?;
    lock(&app.store).delete_items(&account)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ItemsQuery {
    after: Option<String>,
    wait: Option<u64>,
    /// The `pending_version` the device last saw: with it, progress of
    /// announced uploads also ends the wait.
    pending: Option<u64>,
}

async fn get_items(
    State(app): State<Arc<App>>,
    Query(query): Query<ItemsQuery>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<Json<ItemsPage>> {
    let (account, device) = app.authenticate_approved(&method, &uri, &headers, b"")?;
    let after = match query.after.as_deref() {
        // A new device only wants what comes next.
        Some("latest") => {
            let cursor = lock(&app.store).latest_cursor(&account)?;
            let (pending_version, pending) = app.transfers(&account);
            return Ok(Json(ItemsPage {
                cursor,
                items: Vec::new(),
                pending,
                pending_version,
            }));
        }
        Some(after) => after.parse().map_err(|_| bad("invalid cursor"))?,
        None => 0,
    };
    let wait = Duration::from_secs(query.wait.unwrap_or(0).min(api::MAX_WAIT_SECONDS));
    let deadline = tokio::time::Instant::now() + wait;

    let waiter = app.waiter(&account);
    let (items, pending_version, pending) = loop {
        // Subscribe before looking, so nothing arriving in between is missed.
        let notified = waiter.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let items = lock(&app.store).items_after(&account, after, MAX_PAGE_ITEMS)?;
        let (version, pending) = app.transfers(&account);
        let progressed = query.pending.is_some_and(|seen| seen != version);
        if !items.is_empty() || progressed || tokio::time::Instant::now() >= deadline {
            break (items, version, pending);
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            let items = lock(&app.store).items_after(&account, after, MAX_PAGE_ITEMS)?;
            let (version, pending) = app.transfers(&account);
            break (items, version, pending);
        }
    };

    let mut cursor = after;
    let mut size = 0;
    let mut page = Vec::new();
    for (seq, sender, data) in items {
        // A device's own items only move its cursor along: it has them already.
        if sender == device {
            cursor = seq;
            continue;
        }
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
        pending,
        pending_version,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(
            ServerKeys::generate(&mut OsRng),
            Store::open(std::path::Path::new(":memory:")).unwrap(),
            Registration::Invite,
        )
    }

    fn signed(
        keys: &pastazzo_core::device::DeviceKeys,
        app: &App,
        account: &Id,
        method: &str,
        path: &str,
    ) -> axum::http::Request<Body> {
        let signature = RequestSignature::sign(
            keys,
            &Scope {
                server_fingerprint: app.fingerprint,
                account: *account,
            },
            method,
            path,
            b"",
            now(),
            &mut OsRng,
        );
        axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header(api::HEADER_DEVICE, B64::encode(&signature.device))
            .header(api::HEADER_TIMESTAMP, signature.timestamp.to_string())
            .header(api::HEADER_NONCE, B64::encode(&signature.nonce))
            .header(api::HEADER_SIGNATURE, B64::encode(&signature.signature))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn waiting_devices_can_only_collect_their_grant() {
        use pastazzo_core::device::DeviceKeys;
        use tower::ServiceExt;

        let app = app();
        let account = [3; 16];
        let first = DeviceKeys::generate(&mut OsRng);
        let waiting = DeviceKeys::generate(&mut OsRng);
        {
            let mut store = lock(&app.store);
            store
                .create_account(&account, "seb", b"f", b"k", None, 0)
                .unwrap();
            store
                .add_device(&first.id(), &account, &first.public().to_bytes(), now())
                .unwrap();
            store.approve_if_first(&first.id(), &account, 0).unwrap();
            store
                .add_device(&waiting.id(), &account, &waiting.public().to_bytes(), now())
                .unwrap();
            assert!(!store.approve_if_first(&waiting.id(), &account, 1).unwrap());
        }
        let app = Arc::new(app);
        let call = |request: axum::http::Request<Body>| {
            let router = router(app.clone());
            async move { router.oneshot(request).await.unwrap().status().as_u16() }
        };
        let grant_path = format!("/v1/devices/{}/grant", B64::encode(&waiting.id()));

        for path in ["/v1/items?after=0", "/v1/devices"] {
            assert_eq!(
                call(signed(&waiting, &app, &account, "GET", path)).await,
                403,
                "{path}"
            );
            assert_eq!(
                call(signed(&first, &app, &account, "GET", path)).await,
                200,
                "{path}"
            );
        }
        assert_eq!(
            call(signed(&waiting, &app, &account, "GET", &grant_path)).await,
            404
        );
        // Only the device itself collects its grant.
        assert_eq!(
            call(signed(&first, &app, &account, "GET", &grant_path)).await,
            403
        );

        // The first device approves it; now it's in.
        let grant = axum::http::Request::builder()
            .method("PUT")
            .uri(&grant_path);
        let signature = RequestSignature::sign(
            &first,
            &Scope {
                server_fingerprint: app.fingerprint,
                account,
            },
            "PUT",
            &grant_path,
            b"sealed grant",
            now(),
            &mut OsRng,
        );
        let grant = grant
            .header(api::HEADER_DEVICE, B64::encode(&signature.device))
            .header(api::HEADER_TIMESTAMP, signature.timestamp.to_string())
            .header(api::HEADER_NONCE, B64::encode(&signature.nonce))
            .header(api::HEADER_SIGNATURE, B64::encode(&signature.signature))
            .body(Body::from("sealed grant"))
            .unwrap();
        assert_eq!(call(grant).await, 204);
        assert_eq!(
            call(signed(&waiting, &app, &account, "GET", &grant_path)).await,
            200
        );
        assert_eq!(
            call(signed(&waiting, &app, &account, "GET", "/v1/items?after=0")).await,
            200
        );
    }

    #[test]
    fn attempts_are_limited_per_username_and_overall() {
        let app = app();
        for _ in 0..MAX_ATTEMPTS {
            assert!(app.allow_attempt("seb", 1_000).is_ok());
        }
        assert!(app.allow_attempt("seb", 1_000).is_err());
        // Another username still gets in, until the overall limit.
        let mut allowed = MAX_ATTEMPTS;
        for i in 0.. {
            if app.allow_attempt(&format!("user{i}"), 1_000).is_err() {
                break;
            }
            allowed += 1;
        }
        assert_eq!(allowed, MAX_ATTEMPTS_OVERALL);
        // Both windows pass.
        assert!(app.allow_attempt("seb", 1_000 + ATTEMPT_WINDOW_MS).is_ok());
    }
}
