//! One TLS connection to a receiver: a task writing framed messages, a task
//! reading them (answering the receiver's pings and handing replies to
//! whoever asked), and a heartbeat that notices when the device has gone.
//!
//! Receivers present a self-signed certificate, so it can't be checked
//! against any authority; Xorvio only checks that the handshake is signed by
//! the key in it, as every open-source sender does. Device authentication
//! (Google's certificate chain, on its own namespace) isn't needed for the
//! Default Media Receiver.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;

use crate::CastError;
use crate::proto::{self, CastMessage, Payload};

pub(crate) const NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
pub(crate) const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
/// The platform's own end of the connection.
pub(crate) const PLATFORM: &str = "receiver-0";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// A ping this often, as Google's senders do; a device that sends nothing
/// at all for three of them has gone (switched off, off the network).
const PING_EVERY: Duration = Duration::from_secs(5);
const SILENCE: Duration = Duration::from_secs(16);

/// A JSON message from the device that isn't a ping.
#[derive(Debug, Clone)]
pub(crate) struct Incoming {
    pub source: String,
    pub namespace: String,
    pub json: Value,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>;

pub(crate) struct Connection {
    sender_id: String,
    out: mpsc::UnboundedSender<CastMessage>,
    pending: Pending,
    next_id: AtomicI64,
    closed: watch::Receiver<bool>,
    tasks: Vec<AbortHandle>,
}

impl Connection {
    /// Connects and starts the tasks. Every message that isn't a ping comes
    /// out of the receiver returned (replies too, since they carry status);
    /// it ends when the connection does.
    pub async fn open(address: SocketAddr) -> Result<(Connection, mpsc::UnboundedReceiver<Incoming>), CastError> {
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
            .await
            .map_err(|_| CastError::NoAnswer)?
            .map_err(CastError::Unreachable)?;
        let _ = tcp.set_nodelay(true);
        let connector = TlsConnector::from(Arc::new(client_config()));
        let tls = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(ServerName::IpAddress(address.ip().into()), tcp))
            .await
            .map_err(|_| CastError::NoAnswer)?
            .map_err(CastError::Unreachable)?;
        let (mut reader, mut writer) = tokio::io::split(tls);

        let (out, mut outgoing) = mpsc::unbounded_channel::<CastMessage>();
        let (events, incoming) = mpsc::unbounded_channel();
        let (closed_sender, closed) = watch::channel(false);
        let closed_sender = Arc::new(closed_sender);
        let pending: Pending = Arc::default();
        let heard = Arc::new(Mutex::new(Instant::now()));
        // Unique per connection, so two copies of Xorvio never answer for each other.
        let sender_id = format!("sender-xorvio-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);

        let writing = {
            let closed = closed_sender.clone();
            tokio::spawn(async move {
                while let Some(message) = outgoing.recv().await {
                    if let Err(error) = proto::write_message(&mut writer, &message).await {
                        tracing::info!("a Cast connection couldn't be written to: {error}");
                        break;
                    }
                }
                closed.send_replace(true);
            })
        };
        let reading = {
            let (out, pending, heard, closed) = (out.clone(), pending.clone(), heard.clone(), closed_sender.clone());
            let sender_id = sender_id.clone();
            tokio::spawn(async move {
                loop {
                    let message = match proto::read_message(&mut reader).await {
                        Ok(Some(message)) => message,
                        Ok(None) => break,
                        Err(error) => {
                            tracing::info!("a Cast connection ended: {error}");
                            break;
                        }
                    };
                    *heard.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                    let Payload::Text(text) = &message.payload else { continue };
                    let Ok(json) = serde_json::from_str::<Value>(text) else { continue };
                    if message.namespace == NS_HEARTBEAT {
                        if json["type"] == "PING" {
                            let pong = CastMessage::text(&sender_id, &message.source, NS_HEARTBEAT, json!({"type": "PONG"}).to_string());
                            let _ = out.send(pong);
                        }
                        continue;
                    }
                    if let Some(id) = json["requestId"].as_i64().filter(|id| *id > 0)
                        && let Some(waiter) = pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id)
                    {
                        let _ = waiter.send(json.clone());
                    }
                    let _ = events.send(Incoming { source: message.source, namespace: message.namespace, json });
                }
                // Whoever waits for a reply hears the connection closed.
                pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
                closed.send_replace(true);
            })
        };
        let beating = {
            let (out, heard, closed) = (out.clone(), heard.clone(), closed_sender.clone());
            let sender_id = sender_id.clone();
            tokio::spawn(async move {
                let mut every = tokio::time::interval(PING_EVERY);
                every.tick().await;
                loop {
                    every.tick().await;
                    let quiet = heard.lock().unwrap_or_else(|e| e.into_inner()).elapsed();
                    if quiet > SILENCE {
                        tracing::info!("a Cast device stopped answering");
                        closed.send_replace(true);
                        break;
                    }
                    let ping = CastMessage::text(&sender_id, PLATFORM, NS_HEARTBEAT, json!({"type": "PING"}).to_string());
                    if out.send(ping).is_err() {
                        break;
                    }
                }
            })
        };
        let connection = Connection {
            sender_id,
            out,
            pending,
            next_id: AtomicI64::new(1),
            closed,
            tasks: vec![writing.abort_handle(), reading.abort_handle(), beating.abort_handle()],
        };
        Ok((connection, incoming))
    }

    /// Sends a message that has no reply (CONNECT, CLOSE).
    pub fn send(&self, namespace: &str, destination: &str, payload: Value) -> Result<(), CastError> {
        if self.is_closed() {
            return Err(CastError::Closed);
        }
        let message = CastMessage::text(&self.sender_id, destination, namespace, payload.to_string());
        self.out.send(message).map_err(|_| CastError::Closed)
    }

    /// Sends a request and waits up to `wait` for the reply with its id.
    pub async fn request(&self, namespace: &str, destination: &str, mut payload: Value, wait: Duration) -> Result<Value, CastError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        payload["requestId"] = id.into();
        let (reply, answer) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, reply);
        if let Err(error) = self.send(namespace, destination, payload) {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            return Err(error);
        }
        let mut closed = self.closed.clone();
        tokio::select! {
            answer = answer => answer.map_err(|_| CastError::Closed),
            () = tokio::time::sleep(wait) => {
                self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                Err(CastError::NoAnswer)
            }
            _ = closed.wait_for(|closed| *closed) => Err(CastError::Closed),
        }
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Ends when the connection does.
    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }

    /// Stops the tasks, once whatever was sent last has had a moment to go.
    pub async fn shut(&self) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn client_config() -> ClientConfig {
    let provider = provider();
    let verifier = Arc::new(SelfSigned { algorithms: provider.signature_verification_algorithms });
    ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports TLS 1.2 and 1.3")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
}

/// Takes the receiver's own certificate, but still checks the handshake
/// was signed with its key.
#[derive(Debug)]
struct SelfSigned {
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for SelfSigned {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
