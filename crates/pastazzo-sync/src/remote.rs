//! HTTP calls to the server. Nothing here is trusted for confidentiality:
//! TLS is a bonus, everything that matters is encrypted or signed already.

use std::time::Duration;

use pastazzo_core::Id;
use pastazzo_core::api::{
    self, B64, DeviceRecords, ErrorBody, ItemAnnounce, ItemPosted, ItemsPage, ServerInfo,
};
use pastazzo_core::device::DeviceKeys;
use pastazzo_core::request::{RequestSignature, Scope};
use rand::rngs::OsRng;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ureq::Agent;

use crate::state::State;
use crate::{Result, now};

/// Big enough for a page of items at the image size limit.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

pub struct Remote {
    agent: Agent,
    base: String,
}

impl Remote {
    pub fn new(server_url: &str) -> Self {
        let config = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_resolve(Some(Duration::from_secs(10)))
            .timeout_connect(Some(Duration::from_secs(10)))
            // Long enough for a long poll, short enough to notice a dead link.
            .timeout_global(Some(Duration::from_secs(api::MAX_WAIT_SECONDS + 30)))
            .build();
        Self {
            agent: config.into(),
            base: server_url.trim_end_matches('/').to_owned(),
        }
    }

    fn check<T: DeserializeOwned>(mut response: ureq::http::Response<ureq::Body>) -> Result<T> {
        let status = response.status();
        let body = response.body_mut().with_config().limit(MAX_RESPONSE_BYTES);
        if status.is_success() {
            body.read_json::<T>()
                .map_err(|e| format!("invalid server response: {e}"))
        } else {
            let message = body
                .read_json::<ErrorBody>()
                .map(|b| b.error)
                .unwrap_or_else(|_| status.to_string());
            Err(format!("server: {message} ({})", status.as_u16()))
        }
    }

    fn check_empty(mut response: ureq::http::Response<ureq::Body>) -> Result<()> {
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let message = response
            .body_mut()
            .read_json::<ErrorBody>()
            .map(|b| b.error)
            .unwrap_or_else(|_| status.to_string());
        Err(format!("server: {message} ({})", status.as_u16()))
    }

    pub fn server_info(&self) -> Result<ServerInfo> {
        let response = self
            .agent
            .get(format!("{}/v1/server", self.base))
            .call()
            .map_err(|e| format!("can't reach {}: {e}", self.base))?;
        Self::check(response)
    }

    pub fn post_json<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let response = self
            .agent
            .post(format!("{}{path}", self.base))
            .send_json(body)
            .map_err(|e| format!("can't reach {}: {e}", self.base))?;
        Self::check(response)
    }

    pub fn post_json_empty<B: Serialize>(&self, path: &str, body: &B) -> Result<()> {
        let response = self
            .agent
            .post(format!("{}{path}", self.base))
            .send_json(body)
            .map_err(|e| format!("can't reach {}: {e}", self.base))?;
        Self::check_empty(response)
    }

    fn signature_headers(
        state: &State,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> [(&'static str, String); 4] {
        Self::headers_for(&state.device, &state.scope(), method, path, body)
    }

    fn headers_for(
        device: &DeviceKeys,
        scope: &Scope,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> [(&'static str, String); 4] {
        let signature =
            RequestSignature::sign(device, scope, method, path, body, now(), &mut OsRng);
        [
            (api::HEADER_DEVICE, B64::encode(&signature.device)),
            (api::HEADER_TIMESTAMP, signature.timestamp.to_string()),
            (api::HEADER_NONCE, B64::encode(&signature.nonce)),
            (api::HEADER_SIGNATURE, B64::encode(&signature.signature)),
        ]
    }

    fn signed_get(&self, state: &State, path: &str) -> Result<ureq::http::Response<ureq::Body>> {
        let mut request = self.agent.get(format!("{}{path}", self.base));
        for (name, value) in Self::signature_headers(state, "GET", path, b"") {
            request = request.header(name, value);
        }
        request
            .call()
            .map_err(|e| format!("can't reach {}: {e}", self.base))
    }

    fn signed_send(
        &self,
        state: &State,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let url = format!("{}{path}", self.base);
        let headers = Self::signature_headers(state, method, path, body);
        let result = match method {
            "POST" => {
                let mut request = self
                    .agent
                    .post(url)
                    .content_type("application/octet-stream");
                for (name, value) in headers {
                    request = request.header(name, value);
                }
                request.send(body)
            }
            "PUT" => {
                let mut request = self.agent.put(url).content_type("application/octet-stream");
                for (name, value) in headers {
                    request = request.header(name, value);
                }
                request.send(body)
            }
            "DELETE" => {
                let mut request = self.agent.delete(url);
                for (name, value) in headers {
                    request = request.header(name, value);
                }
                request.call()
            }
            _ => unreachable!("only POST, PUT and DELETE carry signed bodies here"),
        };
        result.map_err(|e| format!("can't reach {}: {e}", self.base))
    }

    pub fn post_item(&self, state: &State, sealed: &[u8]) -> Result<u64> {
        let posted: ItemPosted =
            Self::check(self.signed_send(state, "POST", "/v1/items", sealed)?)?;
        Ok(posted.cursor)
    }

    /// Tells the server a big item is coming, so the other devices can show it.
    pub fn announce(&self, state: &State, item: &Id, size: u64) -> Result<()> {
        let body = serde_json::to_vec(&ItemAnnounce {
            id: B64(item.to_vec()),
            size,
        })
        .map_err(|e| e.to_string())?;
        let path = "/v1/items/announce";
        let mut request = self
            .agent
            .post(format!("{}{path}", self.base))
            .content_type("application/json");
        for (name, value) in Self::signature_headers(state, "POST", path, &body) {
            request = request.header(name, value);
        }
        Self::check_empty(
            request
                .send(&body[..])
                .map_err(|e| format!("can't reach {}: {e}", self.base))?,
        )
    }

    /// Uploads an item as a stream, reporting `(sent, total)` bytes.
    pub fn upload_item(
        &self,
        state: &State,
        sealed: &[u8],
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<u64> {
        let path = "/v1/items";
        let mut request = self
            .agent
            .post(format!("{}{path}", self.base))
            .content_type("application/octet-stream");
        for (name, value) in Self::signature_headers(state, "POST", path, sealed) {
            request = request.header(name, value);
        }
        let mut reader = Counting {
            inner: sealed,
            done: 0,
            total: sealed.len() as u64,
            progress,
        };
        let response = request
            .send(ureq::SendBody::from_reader(&mut reader))
            .map_err(|e| format!("can't reach {}: {e}", self.base))?;
        let posted: ItemPosted = Self::check(response)?;
        Ok(posted.cursor)
    }

    /// Items after `cursor`, waiting up to `wait` seconds if there are none.
    pub fn items(&self, state: &State, after: u64, wait: u64) -> Result<ItemsPage> {
        self.items_with_progress(state, after, wait, None, &mut |_, _| {})
    }

    /// [`Remote::items`] that also returns when announced uploads progress
    /// past `pending_version`, and reports `(read, total)` bytes of the
    /// answer as it downloads.
    pub fn items_with_progress(
        &self,
        state: &State,
        after: u64,
        wait: u64,
        pending_version: Option<u64>,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<ItemsPage> {
        let mut path = format!("/v1/items?after={after}&wait={wait}");
        if let Some(version) = pending_version {
            path.push_str(&format!("&pending={version}"));
        }
        let mut response = self.signed_get(state, &path)?;
        if !response.status().is_success() {
            return Self::check(response);
        }
        let total = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let reader = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .reader();
        let counting = Counting {
            inner: reader,
            done: 0,
            total,
            progress,
        };
        serde_json::from_reader(std::io::BufReader::new(counting))
            .map_err(|e| format!("invalid server response: {e}"))
    }

    /// Deletes every item of the account from the server.
    pub fn delete_items(&self, state: &State) -> Result<()> {
        Self::check_empty(self.signed_send(state, "DELETE", "/v1/items", b"")?)
    }

    pub fn latest_cursor(&self, state: &State) -> Result<u64> {
        let page: ItemsPage = Self::check(self.signed_get(state, "/v1/items?after=latest")?)?;
        Ok(page.cursor)
    }

    pub fn put_device_record(&self, state: &State, record: &[u8]) -> Result<()> {
        let path = format!("/v1/devices/{}", B64::encode(&state.device.id()));
        Self::check_empty(self.signed_send(state, "PUT", &path, record)?)
    }

    pub fn device_records(&self, state: &State) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .devices(state)?
            .records
            .into_iter()
            .map(|r| r.0)
            .collect())
    }

    /// The account's device records, and the devices waiting for approval.
    pub fn devices(&self, state: &State) -> Result<DeviceRecords> {
        Self::check(self.signed_get(state, "/v1/devices")?)
    }

    /// Hands a waiting device its grant, which approves it.
    pub fn put_grant(&self, state: &State, device: &Id, grant: &[u8]) -> Result<()> {
        let path = format!("/v1/devices/{}/grant", B64::encode(device));
        Self::check_empty(self.signed_send(state, "PUT", &path, grant)?)
    }

    /// For a device waiting for approval: its grant, once someone approved it.
    pub fn grant(&self, device: &DeviceKeys, scope: &Scope) -> Result<Option<Vec<u8>>> {
        let path = format!("/v1/devices/{}/grant", B64::encode(&device.id()));
        let mut request = self.agent.get(format!("{}{path}", self.base));
        for (name, value) in Self::headers_for(device, scope, "GET", &path, b"") {
            request = request.header(name, value);
        }
        let mut response = request
            .call()
            .map_err(|e| format!("can't reach {}: {e}", self.base))?;
        match response.status().as_u16() {
            200 => Ok(Some(
                response
                    .body_mut()
                    .with_config()
                    .limit(4096)
                    .read_to_vec()
                    .map_err(|e| e.to_string())?,
            )),
            404 => Ok(None),
            _ => Self::check_empty(response).map(|()| None),
        }
    }

    pub fn revoke_device(&self, state: &State, device: &[u8; 16]) -> Result<()> {
        let path = format!("/v1/devices/{}", B64::encode(device));
        Self::check_empty(self.signed_send(state, "DELETE", &path, b"")?)
    }

    pub fn pairing_create(
        &self,
        state: &State,
        body: &api::PairingCreate,
    ) -> Result<api::PairingStatus> {
        let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
        Self::check(self.signed_send(state, "POST", "/v1/pairings", &bytes)?)
    }

    pub fn pairing_status(&self, state: &State, id: &Id) -> Result<api::PairingStatus> {
        Self::check(self.signed_get(state, &format!("/v1/pairings/{}", B64::encode(id)))?)
    }

    pub fn pairing_grant(&self, state: &State, id: &Id, grant: &[u8]) -> Result<()> {
        Self::check_empty(self.signed_send(
            state,
            "PUT",
            &format!("/v1/pairings/{}/grant", B64::encode(id)),
            grant,
        )?)
    }

    pub fn pairing_cancel(&self, state: &State, id: &Id) -> Result<()> {
        Self::check_empty(self.signed_send(
            state,
            "DELETE",
            &format!("/v1/pairings/{}", B64::encode(id)),
            b"",
        )?)
    }
}

/// A reader that reports how much has gone through it.
struct Counting<'a, R> {
    inner: R,
    done: u64,
    total: u64,
    progress: &'a mut dyn FnMut(u64, u64),
}

impl<R: std::io::Read> std::io::Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.done += read as u64;
        (self.progress)(self.done, self.total);
        Ok(read)
    }
}
