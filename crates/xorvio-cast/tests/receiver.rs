//! Cast v2 against a fake receiver: a TLS server on this computer with a
//! self-signed certificate, speaking the protocol as a Chromecast does
//! (connection, heartbeat, receiver and media namespaces). No real device is
//! ever contacted.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use xorvio_cast::wire::{CastMessage, Payload, read_message, write_message};
use xorvio_cast::{CastError, DEFAULT_MEDIA_RECEIVER, IdleReason, Media, PlayerState, Session, SessionState};

const NS_CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
const NS_HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const NS_RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
const NS_MEDIA: &str = "urn:x-cast:com.google.cast.media";
const TRANSPORT: &str = "web-7";
const APP_SESSION: &str = "7f3c0a12-app-session";

/// How the fake receiver behaves.
#[derive(Clone, Default)]
struct Behaviour {
    /// The Default Media Receiver is showing before anyone connects.
    already_running: bool,
    /// LAUNCH is answered before the app is up; a status says so after.
    slow_launch: bool,
    /// LOAD fails, as for media the receiver can't play.
    fail_load: bool,
    /// LOAD fetches the media's address first, as a receiver does.
    fetch: bool,
}

/// What fetching the media's address answered: its status and body.
type Fetched = Arc<Mutex<Option<(u16, Vec<u8>)>>>;

/// What the fake receiver was sent, and a way to make it speak up.
#[derive(Clone)]
struct Fake {
    address: SocketAddr,
    /// Every JSON message received: (namespace, destination, payload).
    received: Arc<Mutex<Vec<(String, String, Value)>>>,
    fetched: Fetched,
    /// Messages to send to the sender out of the blue.
    say: Arc<Mutex<Option<mpsc::UnboundedSender<Say>>>>,
}

enum Say {
    Finished,
    AppClosed,
    Drop,
}

impl Fake {
    fn types(&self, namespace: &str) -> Vec<String> {
        self.received.lock().unwrap().iter().filter(|(ns, _, _)| ns == namespace).filter_map(|(_, _, p)| p["type"].as_str().map(String::from)).collect()
    }

    fn last(&self, kind: &str) -> Value {
        self.received.lock().unwrap().iter().rev().find(|(_, _, p)| p["type"] == kind).map(|(_, _, p)| p.clone()).unwrap_or(Value::Null)
    }

    fn say(&self, what: Say) {
        self.say.lock().unwrap().as_ref().expect("a sender connected").send(what).unwrap();
    }
}

async fn start(behaviour: Behaviour) -> Fake {
    let key = rcgen::generate_simple_self_signed(vec!["fake-receiver.local".to_string()]).unwrap();
    let certificate = key.cert.der().clone();
    let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], private)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake = Fake {
        address: listener.local_addr().unwrap(),
        received: Arc::default(),
        fetched: Arc::default(),
        say: Arc::default(),
    };
    let serving = fake.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let Ok(tls) = acceptor.accept(tcp).await else { continue };
            tokio::spawn(serve(tls, behaviour.clone(), serving.clone()));
        }
    });
    fake
}

struct Receiver {
    writer: WriteHalf<TlsStream<TcpStream>>,
    behaviour: Behaviour,
    running: bool,
    sender: String,
    player: &'static str,
    idle_reason: Option<&'static str>,
    position: f64,
    duration: Option<f64>,
    volume: f64,
    muted: bool,
}

impl Receiver {
    async fn send(&mut self, source: &str, destination: &str, namespace: &str, payload: Value) {
        let message = CastMessage::text(source, destination, namespace, payload.to_string());
        let _ = write_message(&mut self.writer, &message).await;
    }

    fn receiver_status(&self, request_id: i64) -> Value {
        let applications: Vec<Value> = if self.running {
            vec![json!({ "appId": DEFAULT_MEDIA_RECEIVER, "displayName": "Default Media Receiver", "sessionId": APP_SESSION, "transportId": TRANSPORT, "namespaces": [{ "name": NS_MEDIA }] })]
        } else {
            vec![]
        };
        json!({ "type": "RECEIVER_STATUS", "requestId": request_id, "status": { "applications": applications, "volume": { "level": self.volume, "muted": self.muted } } })
    }

    fn media_status(&self, request_id: i64) -> Value {
        let mut entry = json!({ "mediaSessionId": 1, "playerState": self.player, "currentTime": self.position, "media": { "duration": self.duration } });
        if let Some(reason) = self.idle_reason {
            entry["idleReason"] = reason.into();
        }
        json!({ "type": "MEDIA_STATUS", "requestId": request_id, "status": [entry] })
    }
}

async fn serve(tls: TlsStream<TcpStream>, behaviour: Behaviour, fake: Fake) {
    let (mut reader, writer): (ReadHalf<_>, _) = tokio::io::split(tls);
    let (say_to, mut said) = mpsc::unbounded_channel();
    *fake.say.lock().unwrap() = Some(say_to);
    let mut receiver = Receiver {
        writer,
        running: behaviour.already_running,
        behaviour,
        sender: String::new(),
        player: "IDLE",
        idle_reason: None,
        position: 0.0,
        duration: None,
        volume: 0.4,
        muted: false,
    };
    loop {
        let message = tokio::select! {
            message = read_message(&mut reader) => match message { Ok(Some(m)) => m, _ => break },
            what = said.recv() => {
                match what {
                    Some(Say::Finished) => {
                        (receiver.player, receiver.idle_reason) = ("IDLE", Some("FINISHED"));
                        let status = receiver.media_status(0);
                        receiver.send(TRANSPORT, "*", NS_MEDIA, status).await;
                    }
                    Some(Say::AppClosed) => {
                        receiver.running = false;
                        let status = receiver.receiver_status(0);
                        receiver.send("receiver-0", "*", NS_RECEIVER, status).await;
                    }
                    Some(Say::Drop) | None => break,
                }
                continue;
            }
        };
        let Payload::Text(text) = &message.payload else { continue };
        let payload: Value = serde_json::from_str(text).unwrap();
        fake.received.lock().unwrap().push((message.namespace.clone(), message.destination.clone(), payload.clone()));
        receiver.sender = message.source.clone();
        let id = payload["requestId"].as_i64().unwrap_or(0);
        let from = message.source.clone();
        match (message.namespace.as_str(), payload["type"].as_str().unwrap_or_default()) {
            (NS_CONNECTION, "CONNECT") if message.destination == "receiver-0" => {
                // Receivers ping as well, and expect an answer.
                receiver.send("receiver-0", &from, NS_HEARTBEAT, json!({ "type": "PING" })).await;
            }
            (NS_HEARTBEAT, "PING") => receiver.send("receiver-0", &from, NS_HEARTBEAT, json!({ "type": "PONG" })).await,
            (NS_RECEIVER, "GET_STATUS") => {
                let status = receiver.receiver_status(id);
                receiver.send("receiver-0", &from, NS_RECEIVER, status).await;
            }
            (NS_RECEIVER, "LAUNCH") => {
                assert_eq!(payload["appId"], DEFAULT_MEDIA_RECEIVER);
                if receiver.behaviour.slow_launch {
                    let status = receiver.receiver_status(id);
                    receiver.send("receiver-0", &from, NS_RECEIVER, status).await;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    receiver.running = true;
                    let status = receiver.receiver_status(0);
                    receiver.send("receiver-0", "*", NS_RECEIVER, status).await;
                } else {
                    receiver.running = true;
                    let status = receiver.receiver_status(id);
                    receiver.send("receiver-0", &from, NS_RECEIVER, status).await;
                }
            }
            (NS_RECEIVER, "SET_VOLUME") => {
                if let Some(level) = payload["volume"]["level"].as_f64() {
                    receiver.volume = level;
                }
                if let Some(muted) = payload["volume"]["muted"].as_bool() {
                    receiver.muted = muted;
                }
                let status = receiver.receiver_status(id);
                receiver.send("receiver-0", &from, NS_RECEIVER, status.clone()).await;
            }
            (NS_RECEIVER, "STOP") => {
                assert_eq!(payload["sessionId"], APP_SESSION);
                receiver.running = false;
                let status = receiver.receiver_status(id);
                receiver.send("receiver-0", &from, NS_RECEIVER, status).await;
                receiver.send(TRANSPORT, &from, NS_CONNECTION, json!({ "type": "CLOSE" })).await;
            }
            (NS_MEDIA, "LOAD") => {
                assert_eq!(message.destination, TRANSPORT, "media goes to the app's transport");
                assert_eq!(payload["sessionId"], APP_SESSION);
                if receiver.behaviour.fetch {
                    let url = payload["media"]["contentId"].as_str().unwrap().to_string();
                    let response = reqwest::get(&url).await.unwrap();
                    let status = response.status().as_u16();
                    let bytes = response.bytes().await.unwrap_or_default().to_vec();
                    *fake.fetched.lock().unwrap() = Some((status, bytes));
                }
                if receiver.behaviour.fail_load {
                    receiver.send(TRANSPORT, &from, NS_MEDIA, json!({ "type": "LOAD_FAILED", "requestId": id, "detailedErrorCode": 104 })).await;
                    continue;
                }
                receiver.send(TRANSPORT, "*", NS_MEDIA, json!({ "type": "MEDIA_STATUS", "requestId": 0, "status": [{ "mediaSessionId": 1, "playerState": "BUFFERING", "currentTime": 0.0 }] })).await;
                (receiver.player, receiver.idle_reason) = ("PLAYING", None);
                receiver.position = payload["currentTime"].as_f64().unwrap_or(0.0);
                receiver.duration = payload["media"]["duration"].as_f64();
                let status = receiver.media_status(id);
                receiver.send(TRANSPORT, &from, NS_MEDIA, status).await;
            }
            (NS_MEDIA, command) => {
                assert_eq!(payload["mediaSessionId"], 1, "{command} names the media session");
                match command {
                    "PLAY" => receiver.player = "PLAYING",
                    "PAUSE" => receiver.player = "PAUSED",
                    "SEEK" => receiver.position = payload["currentTime"].as_f64().unwrap(),
                    "STOP" => (receiver.player, receiver.idle_reason) = ("IDLE", Some("CANCELLED")),
                    "GET_STATUS" => {}
                    other => panic!("unexpected media command {other}"),
                }
                let status = receiver.media_status(id);
                receiver.send(TRANSPORT, &from, NS_MEDIA, status).await;
            }
            _ => {}
        }
    }
}

/// Waits until the session's state matches.
async fn until(session: &Session, what: &str, check: impl Fn(&SessionState) -> bool) -> SessionState {
    let mut state = session.state();
    let found = tokio::time::timeout(Duration::from_secs(10), state.wait_for(|s| check(s))).await;
    found.unwrap_or_else(|_| panic!("timed out waiting for {what}: {:?}", *session.state().borrow())).unwrap().clone()
}

fn film(url: &str) -> Media {
    Media {
        url: url.into(),
        content_type: "video/mp4".into(),
        live: false,
        duration: Some(5400.0),
        hls_fmp4: false,
        title: "A Film".into(),
        subtitle: Some("1999".into()),
        image: None,
    }
}

#[tokio::test]
async fn opens_the_media_receiver_loads_and_controls_it() {
    let fake = start(Behaviour::default()).await;
    let session = Session::open(fake.address).await.unwrap();
    assert_eq!(fake.types(NS_RECEIVER), ["GET_STATUS", "LAUNCH"]);
    assert_eq!(session.state().borrow().volume, 0.4, "the volume comes from the first status");

    session.load(&film("http://192.168.1.2:5000/t/s/id/video.mp4"), 1234.5).await.unwrap();
    // CONNECT has no reply; the fake handles messages in order, so it has by now.
    let connects: Vec<String> = fake.received.lock().unwrap().iter().filter(|(ns, _, _)| ns == NS_CONNECTION).map(|(_, to, _)| to.clone()).collect();
    assert_eq!(connects, ["receiver-0", TRANSPORT], "to the platform, then to the app");
    assert!(fake.last("CONNECT")["userAgent"].as_str().unwrap().starts_with("Xorvio/"), "says who's connecting");
    let load = fake.last("LOAD");
    assert_eq!(load["media"]["contentId"], "http://192.168.1.2:5000/t/s/id/video.mp4");
    assert_eq!(load["media"]["streamType"], "BUFFERED");
    assert_eq!(load["media"]["metadata"]["title"], "A Film");
    assert_eq!(load["currentTime"], 1234.5, "a film starts where it was");
    assert_eq!(load["autoplay"], true);
    let state = until(&session, "playing", |s| s.player == PlayerState::Playing).await;
    assert_eq!((state.position, state.duration), (1234.5, Some(5400.0)));

    session.pause().await.unwrap();
    until(&session, "paused", |s| s.player == PlayerState::Paused).await;
    session.seek(60.0).await.unwrap();
    let state = until(&session, "the seek", |s| s.position == 60.0).await;
    assert_eq!(state.player, PlayerState::Paused, "a seek keeps it paused");
    session.play().await.unwrap();
    until(&session, "playing again", |s| s.player == PlayerState::Playing).await;
    session.set_volume(0.75).await.unwrap();
    until(&session, "the volume", |s| s.volume == 0.75).await;
    session.set_muted(true).await.unwrap();
    until(&session, "muted", |s| s.muted).await;
    assert_eq!(fake.types(NS_MEDIA), ["LOAD", "PAUSE", "SEEK", "PLAY"]);

    // The receiver's own pings are answered.
    assert!(fake.types(NS_HEARTBEAT).contains(&"PONG".to_string()), "{:?}", fake.types(NS_HEARTBEAT));

    session.close().await;
    assert_eq!(fake.types(NS_MEDIA).last().map(String::as_str), Some("STOP"), "the media stops");
    assert_eq!(fake.last("STOP")["sessionId"], APP_SESSION, "then the app, so the TV goes back to idle");
    assert!(!session.is_connected());
    assert!(matches!(session.play().await, Err(CastError::Closed)));
}

#[tokio::test]
async fn joins_a_receiver_already_open_and_waits_for_a_slow_one() {
    let fake = start(Behaviour { already_running: true, ..Behaviour::default() }).await;
    let session = Session::open(fake.address).await.unwrap();
    assert_eq!(fake.types(NS_RECEIVER), ["GET_STATUS"], "no second launch");
    session.close().await;

    let fake = start(Behaviour { slow_launch: true, ..Behaviour::default() }).await;
    let session = Session::open(fake.address).await.unwrap();
    session.load(&film("http://192.168.1.2:5000/t/s/id/video.mp4"), 0.0).await.unwrap();
    assert!(fake.last("LOAD").get("currentTime").is_none(), "from the start, no time is sent");
    assert!(session.is_connected(), "the status from before the launch doesn't count as the app closing");
}

#[tokio::test]
async fn live_streams_go_as_hls_of_fragmented_mp4() {
    let fake = start(Behaviour::default()).await;
    let session = Session::open(fake.address).await.unwrap();
    let channel = Media {
        url: "http://192.168.1.2:5000/t/l/id/index.m3u8".into(),
        content_type: "application/x-mpegurl".into(),
        live: true,
        duration: None,
        hls_fmp4: true,
        title: "Channel One".into(),
        subtitle: Some("The News".into()),
        image: Some("https://image.example/one.png".into()),
    };
    session.load(&channel, 300.0).await.unwrap();
    let load = fake.last("LOAD");
    assert_eq!(load["media"]["streamType"], "LIVE");
    assert_eq!(load["media"]["hlsVideoSegmentFormat"], "fmp4");
    assert_eq!(load["media"]["metadata"]["images"][0]["url"], "https://image.example/one.png");
    assert!(load.get("currentTime").is_none(), "live starts at live");
}

#[tokio::test]
async fn failures_and_endings_are_reported() {
    let fake = start(Behaviour { fail_load: true, ..Behaviour::default() }).await;
    let session = Session::open(fake.address).await.unwrap();
    let error = session.load(&film("http://192.168.1.2:5000/x"), 0.0).await.unwrap_err();
    assert!(matches!(error, CastError::LoadFailed { reason: Some(ref r) } if r == "error 104"), "{error:?}");
    assert!(matches!(session.pause().await, Err(CastError::Refused(_))), "nothing loaded to pause");

    let fake = start(Behaviour::default()).await;
    let session = Session::open(fake.address).await.unwrap();
    session.load(&film("http://192.168.1.2:5000/x"), 0.0).await.unwrap();
    fake.say(Say::Finished);
    let state = until(&session, "finished", |s| s.player == PlayerState::Idle).await;
    assert_eq!(state.idle_reason, Some(IdleReason::Finished));
    assert!(state.connected);
    // Another app opened on the TV.
    fake.say(Say::AppClosed);
    until(&session, "the app closing", |s| !s.connected).await;

    // The device going away mid-session.
    let fake = start(Behaviour::default()).await;
    let session = Session::open(fake.address).await.unwrap();
    fake.say(Say::Drop);
    until(&session, "the connection dropping", |s| !s.connected).await;
    assert!(!session.is_connected());
}

#[tokio::test]
async fn nobody_there_is_said_plainly() {
    // A port nothing listens on.
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    assert!(matches!(Session::open(free).await, Err(CastError::Unreachable(_))));
}

#[tokio::test]
async fn the_receiver_fetches_what_it_is_given() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route("/t/s/id/video.mp4", get(|| async { "not really a film" }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let fake = start(Behaviour { fetch: true, ..Behaviour::default() }).await;
    let session = Session::open(fake.address).await.unwrap();
    session.load(&film(&format!("{origin}/t/s/id/video.mp4")), 0.0).await.unwrap();
    let fetched = fake.fetched.lock().unwrap().clone().unwrap();
    assert_eq!(fetched, (200, b"not really a film".to_vec()));
}

#[tokio::test]
async fn pings_go_out_every_few_seconds() {
    let fake = start(Behaviour::default()).await;
    let _session = Session::open(fake.address).await.unwrap();
    tokio::time::sleep(Duration::from_millis(5600)).await;
    let pings = fake.received.lock().unwrap().iter().filter(|(ns, to, p)| ns == NS_HEARTBEAT && to == "receiver-0" && p["type"] == "PING").count();
    assert!(pings >= 1, "a ping within five seconds");
}
