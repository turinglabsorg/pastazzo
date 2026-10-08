use axum::{
    Json, Router,
    extract::State as Extract,
    routing::{get, post},
};
use pastazzo_core::{
    api::Registration,
    approval::approval_code,
    device::DevicePublic,
    invite::{Invite, InviteKey},
    item::{Content, ItemHeader, SealedItem},
    random_id,
    server::ServerKeys,
};
use pastazzo_server::{
    app::{App, router},
    store::Store,
};
use pastazzo_sync::{
    account,
    pairing::{self, Offer},
    remote::Remote,
    state::State,
};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
struct Fixture {
    desktop: Arc<Mutex<State>>,
    offer: Arc<Mutex<Option<Offer>>>,
    offline: Arc<AtomicBool>,
}

fn main() {
    let keys = ServerKeys::generate(&mut OsRng);
    let fingerprint = keys.identity().fingerprint();
    let invite = InviteKey::generate(&mut OsRng);
    let verifier = invite.verifier();
    let store = Store::open(Path::new(":memory:")).unwrap();
    store
        .add_invite(&verifier.id, &verifier.public, 0, u64::MAX / 2)
        .unwrap();
    let app = Arc::new(App::new(keys, store, Registration::Invite));
    let offline = Arc::new(AtomicBool::new(false));
    let server_offline = offline.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:32951")
                    .await
                    .unwrap();
                ready_tx.send(()).unwrap();
                use axum::response::IntoResponse;
                let gate = axum::middleware::from_fn(
                    move |request: axum::extract::Request, next: axum::middleware::Next| {
                        let offline = server_offline.clone();
                        async move {
                            if offline.load(Ordering::SeqCst) {
                                axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
                            } else {
                                next.run(request).await
                            }
                        }
                    },
                );
                axum::serve(listener, router(app).layer(gate))
                    .await
                    .unwrap();
            });
    });
    ready_rx.recv().unwrap();
    let desktop = account::join(
        Invite {
            server_url: "http://127.0.0.1:32951".into(),
            fingerprint,
            key: invite,
        }
        .to_link()
        .as_str(),
        "android-fixture",
        "synthetic fixture password",
        "Mac Pro QA",
    )
    .unwrap();
    let fixture = Fixture {
        desktop: Arc::new(Mutex::new(desktop)),
        offer: Arc::new(Mutex::new(None)),
        offline,
    };
    let approver = fixture.clone();
    std::thread::spawn(move || {
        loop {
            {
                let desktop = approver.desktop.lock().unwrap();
                let mut offer = approver.offer.lock().unwrap();
                if let Some(current) = offer.as_ref()
                    && let Ok(status) = pairing::status(&desktop, current)
                    && let Some(peer) = status.peer
                {
                    let device = DevicePublic::from_bytes(&peer.device.0).unwrap();
                    pairing::approve(&desktop, current, &approval_code(&device)).unwrap();
                    *offer = None;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async move {
            let controls = Router::new()
                .route(
                    "/offline",
                    post(
                        |Extract(f): Extract<Fixture>, Json(body): Json<Value>| async move {
                            f.offline
                                .store(body["offline"].as_bool().unwrap(), Ordering::SeqCst);
                            Json(json!({"ok":true}))
                        },
                    ),
                )
                .route(
                    "/link",
                    get(|Extract(f): Extract<Fixture>| async move {
                        tokio::task::spawn_blocking(move || {
                            let desktop = f.desktop.lock().unwrap();
                            let (link, offer) = pairing::create(&desktop).unwrap();
                            *f.offer.lock().unwrap() = Some(offer);
                            Json(json!({"link":link.to_link().as_str()}))
                        })
                        .await
                        .unwrap()
                    }),
                )
                .route(
                    "/send",
                    post(
                        |Extract(f): Extract<Fixture>, Json(body): Json<Value>| async move {
                            tokio::task::spawn_blocking(move || {
                                let desktop = f.desktop.lock().unwrap();
                                let sealed = SealedItem::seal(
                                    &desktop.account_key,
                                    &desktop.account,
                                    ItemHeader {
                                        id: random_id(&mut OsRng),
                                        device: desktop.device.id(),
                                        epoch: desktop.account_key.epoch(),
                                        created_at: pastazzo_sync::now(),
                                    },
                                    &Content::Text(body["text"].as_str().unwrap().into()),
                                    &mut OsRng,
                                )
                                .unwrap();
                                Remote::new(&desktop.server_url)
                                    .post_item(&desktop, &sealed.to_bytes())
                                    .unwrap();
                                Json(json!({"sent":true}))
                            })
                            .await
                            .unwrap()
                        },
                    ),
                )
                .route(
                    "/received",
                    get(|Extract(f): Extract<Fixture>| async move {
                        tokio::task::spawn_blocking(move || {
                            let desktop = f.desktop.lock().unwrap();
                            let page = Remote::new(&desktop.server_url)
                                .items(&desktop, 0, 0)
                                .unwrap();
                            let values: Vec<Value> = page
                                .items
                                .iter()
                                .map(|bytes| {
                                    let sealed = SealedItem::from_bytes(&bytes.0).unwrap();
                                    match sealed
                                        .open(&desktop.account_key, &desktop.account)
                                        .unwrap()
                                    {
                                        Content::Text(text) => json!({"text":text}),
                                        Content::Image { mime, data } => {
                                            json!({"mime":mime,"size":data.len()})
                                        }
                                        Content::ClearHistory => json!({"clear":true}),
                                    }
                                })
                                .collect();
                            Json(json!({"items":values}))
                        })
                        .await
                        .unwrap()
                    }),
                )
                .with_state(fixture);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:32952")
                .await
                .unwrap();
            println!(
                "Android fixture ready on loopback ports 32951/32952; synthetic account only."
            );
            axum::serve(listener, controls).await.unwrap();
        });
}
