use super::*;
use pastazzo_core::api::{PairingCreate, PairingPeer, PairingReply, PairingStatus};
use pastazzo_core::pairing::{TTL_MS, verify_peer};

pub(super) struct Session {
    account: Id,
    owner: Id,
    verifier: InviteVerifier,
    username: String,
    expires_at: u64,
    peer: Option<PairingPeer>,
    grant: Option<Vec<u8>>,
}

fn parsed_id(id: &str) -> ApiResult<Id> {
    B64::decode(id)
        .and_then(|x| x.try_into().ok())
        .ok_or(bad("invalid pairing id"))
}

fn active<'a>(
    app: &App,
    sessions: &'a mut HashMap<Id, Session>,
    id: &Id,
) -> ApiResult<&'a mut Session> {
    sessions.retain(|_, session| session.expires_at > now());
    let session = sessions.get_mut(id).ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "QR expired; show a new QR on your Mac",
    ))?;
    let owner = lock(&app.store).device(&session.owner)?;
    if !owner.is_some_and(|d| d.approved && !d.revoked && d.account == session.account) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "pairing owner is no longer authorized",
        ));
    }
    Ok(session)
}

fn own(session: &Session, account: &Id, device: &Id) -> ApiResult<()> {
    if session.account != *account || session.owner != *device {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "this QR belongs to another device",
        ));
    }
    Ok(())
}

pub(super) async fn create(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    bytes: Bytes,
) -> ApiResult<Json<PairingStatus>> {
    let (account, owner) = app.authenticate_approved(&method, &uri, &headers, &bytes)?;
    let body: PairingCreate = serde_json::from_slice(&bytes).map_err(|_| bad("invalid pairing"))?;
    let id = id_from(&body.id, "invalid pairing id")?;
    let public = body
        .verifier
        .array()
        .ok_or(bad("invalid pairing verifier"))?;
    opaque::validate_username(&body.username).map_err(|_| bad("invalid username"))?;
    let mut sessions = lock(&app.pairings);
    sessions.retain(|_, s| s.expires_at > now());
    if let Some(existing) = sessions.get(&id) {
        own(existing, &account, &owner)?;
        if existing.verifier.public != public || existing.username != body.username {
            return Err(ApiError(StatusCode::CONFLICT, "pairing id already exists"));
        }
        return Ok(Json(PairingStatus {
            expires_at: existing.expires_at,
            peer: existing.peer.clone(),
            completed: existing.grant.is_some(),
        }));
    }
    sessions.retain(|_, s| s.account != account || s.owner != owner);
    if sessions.len() >= MAX_ATTEMPTS_OVERALL {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "too many active pairings",
        ));
    }
    let expires_at = now() + TTL_MS;
    sessions.insert(
        id,
        Session {
            account,
            owner,
            verifier: InviteVerifier { id, public },
            username: body.username,
            expires_at,
            peer: None,
            grant: None,
        },
    );
    Ok(Json(PairingStatus {
        expires_at,
        peer: None,
        completed: false,
    }))
}

pub(super) async fn status(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<Json<PairingStatus>> {
    let (account, owner) = app.authenticate_approved(&method, &uri, &headers, b"")?;
    let mut sessions = lock(&app.pairings);
    let session = active(&app, &mut sessions, &parsed_id(&id)?)?;
    own(session, &account, &owner)?;
    Ok(Json(PairingStatus {
        expires_at: session.expires_at,
        peer: session.peer.clone(),
        completed: session.grant.is_some(),
    }))
}

pub(super) async fn cancel(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let (account, owner) = app.authenticate_approved(&method, &uri, &headers, b"")?;
    let id = parsed_id(&id)?;
    let mut sessions = lock(&app.pairings);
    own(active(&app, &mut sessions, &id)?, &account, &owner)?;
    sessions.remove(&id);
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn request(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    Json(peer): Json<PairingPeer>,
) -> ApiResult<Json<PairingReply>> {
    let mut sessions = lock(&app.pairings);
    let session = active(&app, &mut sessions, &parsed_id(&id)?)?;
    let device = DevicePublic::from_bytes(&peer.device.0).map_err(|_| bad("invalid device"))?;
    verify_peer(
        &session.verifier,
        &session.username,
        &device,
        &peer.name,
        &peer.proof.0,
    )
    .map_err(|_| ApiError(StatusCode::FORBIDDEN, "invalid QR proof"))?;
    if let Some(existing) = &session.peer {
        if existing.device != peer.device || existing.name != peer.name {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "this QR has already been scanned; show a new QR",
            ));
        }
    } else {
        session.peer = Some(peer);
    }
    Ok(Json(PairingReply {
        account: B64(session.account.to_vec()),
        grant: session.grant.clone().map(B64),
    }))
}

pub(super) async fn grant(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    let (account, owner) = app.authenticate_approved(&method, &uri, &headers, &bytes)?;
    if bytes.len() < 112 || bytes.len() > 512 {
        return Err(bad("invalid pairing grant"));
    }
    let mut sessions = lock(&app.pairings);
    let session = active(&app, &mut sessions, &parsed_id(&id)?)?;
    own(session, &account, &owner)?;
    if let Some(existing) = &session.grant {
        return if existing == bytes.as_ref() {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(ApiError(StatusCode::CONFLICT, "pairing already completed"))
        };
    }
    let peer = session
        .peer
        .as_ref()
        .ok_or(bad("no device has scanned this QR"))?;
    let device = DevicePublic::from_bytes(&peer.device.0).map_err(|_| bad("invalid device"))?;
    let store = lock(&app.store);
    if store.device(&device.id)?.is_some() {
        return Err(ApiError(StatusCode::CONFLICT, "device id already exists"));
    }
    if matches!(
        store.add_device(&device.id, &account, &peer.device.0, now())?,
        Insert::Conflict
    ) || !store.set_grant(&device.id, &account, &bytes, now())?
    {
        return Err(ApiError(StatusCode::CONFLICT, "device cannot be paired"));
    }
    session.grant = Some(bytes.to_vec());
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn expired_qr_requests_are_rejected_and_removed() {
        use tower::ServiceExt;
        let app = Arc::new(App::new(
            ServerKeys::generate(&mut OsRng),
            Store::open(std::path::Path::new(":memory:")).unwrap(),
            Registration::Invite,
        ));
        let id = [1; 16];
        lock(&app.pairings).insert(
            id,
            Session {
                account: [2; 16],
                owner: [3; 16],
                verifier: InviteVerifier {
                    id,
                    public: [4; 32],
                },
                username: "seb".into(),
                expires_at: now() - 1,
                peer: None,
                grant: None,
            },
        );
        let body = serde_json::to_vec(&PairingPeer {
            device: B64(vec![]),
            name: "iPhone".into(),
            proof: B64(vec![]),
        })
        .unwrap();
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/v1/pairings/{}/request", B64::encode(&id)))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        assert_eq!(
            router(app.clone()).oneshot(request).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        assert!(lock(&app.pairings).is_empty());
    }
}
