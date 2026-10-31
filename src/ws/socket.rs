

use crate::channel::{CHANNEL, Phase, WSCommand};
use crate::ws::config::*;
use crate::ws::rates;
use crate::ws::relay::{self, RelayState};
use crate::ws::RatesCommand;
use dannesk_noise_protocol::stream as noise;
use std::collections::HashSet;
use tokio::net::TcpStream;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::time::{timeout, Duration, Instant};
use tungstenite::Message;


const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);


const SECOND_ATTEMPT: Duration = Duration::from_millis(250);


const MAX_INBOUND_FRAME: usize = 16 * 1024 * 1024;


async fn dial() -> std::io::Result<TcpStream> {
    let first = TcpStream::connect(PROXY_ADDR);
    tokio::pin!(first);
    let second = async {
        tokio::time::sleep(SECOND_ATTEMPT).await;
        TcpStream::connect(PROXY_ADDR).await
    };
    tokio::pin!(second);
    tokio::select! {
        done = &mut first => match done {
            Ok(stream) => Ok(stream),
            Err(_) => second.await,
        },
        done = &mut second => match done {
            Ok(stream) => Ok(stream),
            Err(_) => first.await,
        },
    }
}


const IDLE_TIMEOUT: Duration = Duration::from_secs(90);


const MIN_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

const LIVED: Duration = Duration::from_secs(30);

const WALLETS_WAIT: Duration = Duration::from_secs(1);

#[derive(Default, Clone, Copy)]
struct Links {
    relay: bool,
    btc: bool,
    rates: bool,
    book: bool,
}

const LINK_PREFIXES: [(fn(&Links) -> bool, &str); 4] =
    [(|l| l.relay, "relay:"), (|l| l.btc, "btc:"), (|l| l.rates, "rates:"), (|l| l.book, "bookd:")];

fn publish_with(before: Links, connected: bool, links: Links) {
    for (link, prefix) in LINK_PREFIXES {
        if link(&before) && !(connected && link(&links)) {
            CHANNEL.mark_link_down(prefix);
        }
    }
    publish(connected, links);
}

fn publish(connected: bool, links: Links) {
    set(&CHANNEL.proxy_ws_status_tx, connected);
    set(&CHANNEL.relay_ws_status_tx, connected && links.relay);
    set(&CHANNEL.btc_ws_status_tx, connected && links.btc);
    set(&CHANNEL.rates_ws_status_tx, connected && links.rates);
    set(&CHANNEL.book_ws_status_tx, connected && links.book);
}


fn set(tx: &tokio::sync::watch::Sender<bool>, up: bool) {
    tx.send_if_modified(|current| {
        let changed = *current != up;
        *current = up;
        changed
    });
}

fn frame(tag: u8, payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    out.push(tag);
    out.push(0);
    out.extend_from_slice(payload.as_bytes());
    out
}

fn proxy_frame_type(text: &str) -> Option<String> {
    let data = serde_json::from_str::<serde_json::Value>(text).ok()?;
    data.get("type").and_then(|t| t.as_str()).map(str::to_string)
}


pub async fn run_websocket(
    mut commands_rx: Receiver<WSCommand>,
    mut outgoing_rx: Receiver<Message>,
    mut rates_cmd_rx: Receiver<RatesCommand>,
    mut shutdown_rx: Receiver<()>,
) -> Result<(), String> {
    let mut backoff = MIN_BACKOFF;
    let mut relay_state = RelayState::default();
    
    let mut books: HashSet<String> = HashSet::new();
   
    let mut launched = false;
    let mut wallets_waited = false;
    let mut books_seeded = false;
    let mut last_immediate = false;
  
    tokio::spawn(async {
        tokio::time::sleep(crate::channel::FETCH_LIMIT).await;
        CHANNEL.launch_tx.send_modify(|launch| launch.over = true);
    });

    loop {
        let mut links = Links::default();
        publish(false, links);

        crate::ws::trace("socket: connecting");
        let dialled = tokio::select! {
            dialled = timeout(CONNECT_TIMEOUT, dial()) => dialled,
            _ = shutdown_rx.recv() => return Ok(()),
        };
        let stream = match dialled {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) | Err(_) => {
                crate::ws::trace("socket: connect failed, retrying after the backoff");
                settle_launch(&mut launched, Phase::Failed);
                if backoff_or_shutdown(&mut backoff, &mut shutdown_rx).await {
                    return Ok(());
                }
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        keepalive(&stream);
        crate::ws::trace("socket: tcp connected");
        // The wallet load runs beside the dial; its asks belong in the first message.
        if !wallets_waited {
            wallets_waited = true;
            let mut loaded = CHANNEL.loaded_rx.clone();
            let _ = timeout(WALLETS_WAIT, loaded.wait_for(|l| l.wallets)).await;
        }

      
        let app_version = crate::app().map_or("unregistered", |app| app.version);
        let hello = format!(r#"{{"type":"hello","v":1,"app":"{}"}}"#, app_version);
        let mut first: Vec<Vec<u8>> = vec![frame(TAG_RELAY, &hello)];
        let mut deferred: Vec<WSCommand> = Vec::new();
        while let Ok(mut cmd) = commands_rx.try_recv() {
            if matches!(
                cmd.command.as_str(),
                "get_cached_balance" | "get_bitcoin_cached_balance" | "delete_wallet" | "delete_bitcoin_wallet"
            ) {
                relay_state.track(&mut cmd);
            } else {
                deferred.push(cmd);
            }
        }

        while let Ok(cmd) = rates_cmd_rx.try_recv() {
            rates::track(&mut books, &cmd);
        }
        // An XRP wallet's books are held for as long as the wallet is, from the first message on.
        if books_seeded != relay_state.has_xrp_wallet() {
            books_seeded = !books_seeded;
            for pair in rates::book_pairs() {
                rates::track(&mut books, &book_command(pair, books_seeded));
            }
        }
      
        let relay_asks = relay_state.resync_relay_payloads();
        let btc_asks = relay_state.resync_btc_payloads();
        for payload in relay_asks.iter().chain(btc_asks.iter()) {
            if let Some((tag, wrapped)) = relay::wrap(payload) {
                first.push(frame(tag, &wrapped));
            }
        }
     
        let wallet_known = relay_state.has_wallet();
        if wallet_known {
            first.push(frame(TAG_RATES, &rates::history_frame(&rates::history_assets())));
            for pair in &books {
                first.push(frame(TAG_BOOK, &rates::subscribe_frame(pair, true)));
            }
        }
      
        let mut asked_in_first = (!relay_asks.is_empty(), !btc_asks.is_empty());
        let first_refs: Vec<&[u8]> = first.iter().map(Vec::as_slice).collect();
        let handshake = noise::connect(stream, &PROXY_KEY.1, PROXY_KEY.0, &first_refs, MAX_INBOUND_FRAME);
        let shaken = tokio::select! {
            shaken = timeout(CONNECT_TIMEOUT, handshake) => shaken,
            _ = shutdown_rx.recv() => return Ok(()),
        };
        let (mut reader, mut writer) = match shaken {
            Ok(Ok(halves)) => halves,
            Ok(Err(_)) | Err(_) => {
                crate::ws::trace("socket: handshake failed, retrying after the backoff");
                settle_launch(&mut launched, Phase::Failed);
                if backoff_or_shutdown(&mut backoff, &mut shutdown_rx).await {
                    return Ok(());
                }
                continue;
            }
        };
        crate::ws::trace("socket: open (handshake done)");
        // Its own task: a read cut off by another branch of the select would lose the bytes it held.
        let (inbound_tx, mut inbound) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        let reading = tokio::spawn(async move {
            while let Ok(frame) = reader.recv_frame().await {
                if inbound_tx.send(frame).await.is_err() {
                    return;
                }
            }
        });
        let opened = Instant::now();
        let mut closed_with: Option<u64> = None;
        for mut cmd in deferred {
            relay_state.track(&mut cmd);
            relay_state.spawn_command(cmd);
        }
    
        let mut rates_asked = wallet_known;
        let mut books_told = wallet_known;

      
        let mut healthy = false;
        let idle = tokio::time::sleep(IDLE_TIMEOUT);
        tokio::pin!(idle);

        loop {
         
            if books_seeded != relay_state.has_xrp_wallet() {
                books_seeded = !books_seeded;
                let mut ok = true;
                for pair in rates::book_pairs() {
                    let told = rates::subscribe_frame(&pair, books_seeded);
                    if rates::track(&mut books, &book_command(pair, books_seeded)) && (books_told || !books_seeded) {
                        ok = ok && writer.send_frame(&frame(TAG_BOOK, &told)).await.is_ok();
                    }
                }
                if !ok {
                    break;
                }
            }
            if relay_state.has_wallet() {
                let mut ok = true;
                if links.rates && !rates_asked {
                    rates_asked = true;
                    ok = writer.send_frame(&frame(TAG_RATES, &rates::history_frame(&rates::history_assets()))).await.is_ok();
                }
                if links.book && !books_told {
                    books_told = true;
                    for pair in &books {
                        ok = ok && writer.send_frame(&frame(TAG_BOOK, &rates::subscribe_frame(pair, true))).await.is_ok();
                    }
                }
                if !ok {
                    break;
                }
            }
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    let _ = writer.shutdown().await;
                    reading.abort();
                    publish(false, links);
                    return Ok(());
                }
                _ = &mut idle => {
                    break;
                }
                Some(mut cmd) = commands_rx.recv() => {
                    relay_state.track(&mut cmd);
                    relay_state.spawn_command(cmd);
                }
                Some(msg) = outgoing_rx.recv() => {
                
                    if let Message::Text(payload) = msg
                        && let Some((tag, wrapped)) = relay::wrap(payload.as_str())
                    {
                        let link_up = if tag == TAG_BTC { links.btc } else { links.relay };
                        if link_up || !relay::report_unsent(payload.as_str()) {
                            if writer.send_frame(&frame(tag, &wrapped)).await.is_err() {
                                break;
                            }
                            if link_up {
                                relay_state.written(payload.as_str());
                            }
                        }
                    }
                }
                Some(cmd) = rates_cmd_rx.recv() => {
                    if !rates::track(&mut books, &cmd) {
                        continue;
                    }
                    let (tag, payload) = match &cmd {
                        RatesCommand::SubscribeBook(pair) => (TAG_BOOK, rates::subscribe_frame(pair, true)),
                        RatesCommand::UnsubscribeBook(pair) => (TAG_BOOK, rates::subscribe_frame(pair, false)),
                    };
                    if writer.send_frame(&frame(tag, &payload)).await.is_err() {
                        break;
                    }
                }
                received = inbound.recv() => {
                    let Some(data) = received else { break };
                    idle.as_mut().reset(Instant::now() + IDLE_TIMEOUT);
                    healthy = true;
                    let Some((tag, text)) = decode(&data) else { continue };
                    match tag {
                    
                        TAG_PROXY => match proxy_frame_type(&text).as_deref() {
                            Some("ping") => {
                                if writer.send_frame(&frame(TAG_PROXY, r#"{"type":"pong"}"#)).await.is_err() {
                                    break;
                                }
                            }
                            Some("close") => {
                                closed_with = close_code(&text);
                                break;
                            }
                            Some("status") => {
                                let before = links;
                                links = parse_links(&text, links);
                                publish_with(before, true, links);
                             
                                settle_launch(&mut launched, Phase::Connected);
                           
                                let (relay_asked, btc_asked) = asked_in_first;
                                asked_in_first = (false, false);
                                // An owed removal is cleared only once a later report shows its link stayed up.
                                if links.relay && !before.relay {
                                    let mut ok = true;
                                    if !relay_asked {
                                        for payload in relay_state.resync_relay_payloads() {
                                            if let Some((tag, wrapped)) = relay::wrap(&payload) {
                                                ok &= writer.send_frame(&frame(tag, &wrapped)).await.is_ok();
                                            }
                                        }
                                    }
                                    if ok {
                                        relay_state.deletes_written("delete_wallet");
                                    }
                                } else if links.relay {
                                    relay_state.deletes_confirmed("delete_wallet");
                                } else {
                                    relay_state.deletes_unwritten("delete_wallet");
                                }
                                if links.btc && !before.btc {
                                    let mut ok = true;
                                    if !btc_asked {
                                   
                                        for payload in relay_state.resync_btc_payloads() {
                                            if let Some((tag, wrapped)) = relay::wrap(&payload) {
                                                ok &= writer.send_frame(&frame(tag, &wrapped)).await.is_ok();
                                            }
                                        }
                                    }
                                    if ok {
                                        relay_state.deletes_written("delete_bitcoin_wallet");
                                    }
                                } else if links.btc {
                                    relay_state.deletes_confirmed("delete_bitcoin_wallet");
                                } else {
                                    relay_state.deletes_unwritten("delete_bitcoin_wallet");
                                }
                               
                                if !links.rates { rates_asked = false; }
                                if !links.book { books_told = false; }
                            }
                            _ => {}
                        },
                        TAG_RELAY | TAG_BTC => relay_state.handle_frame(text).await,
                        TAG_RATES | TAG_BOOK => {
                            if tag == TAG_RATES {
                                crate::ws::trace("socket: first rates frame in");
                            }
                            rates::process_message(&text)
                        }
                        _ => {}
                    }
                }
            }
        }

        reading.abort();
        publish_with(links, false, Links::default());
        relay_state.deletes_unwritten("delete_wallet");
        relay_state.deletes_unwritten("delete_bitcoin_wallet");

        
        let mut held = Vec::new();
        while let Ok(msg) = outgoing_rx.try_recv() {
            if let Message::Text(payload) = &msg
                && relay::report_unsent(payload.as_str())
            {
                continue;
            }
            held.push(msg);
        }
        if let Some(tx) = crate::ws::CRYPTO_OUTGOING_TX.get() {
            for msg in held {
                let _ = tx.try_send(msg);
            }
        }
        // Redial at once after a session that ran; young deaths in a row back off.
        if opened.elapsed() >= LIVED {
            backoff = MIN_BACKOFF;
            last_immediate = false;
        }
        if healthy && matches!(closed_with, None | Some(1000)) && !last_immediate {
            last_immediate = true;
            if !matches!(shutdown_rx.try_recv(), Err(TryRecvError::Empty)) {
                return Ok(());
            }
            continue;
        }
        if backoff_or_shutdown(&mut backoff, &mut shutdown_rx).await {
            return Ok(());
        }
    }
}

fn settle_launch(launched: &mut bool, phase: Phase) {
    if !*launched {
        *launched = true;
        CHANNEL.launch_tx.send_modify(|launch| launch.phase = phase);
    }
}


fn decode(data: &[u8]) -> Option<(u8, String)> {
    if data.len() < 2 {
        return None;
    }
    let (tag, flags, body) = (data[0], data[1], &data[2..]);
    let bytes = if flags & FLAG_ZSTD != 0 {
        zstd::decode_all(body).ok()?
    } else {
        body.to_vec()
    };
    Some((tag, String::from_utf8(bytes).ok()?))
}


fn parse_links(text: &str, mut links: Links) -> Links {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(text) else { return links };
    let Some(components) = data.get("components").and_then(|v| v.as_object()) else { return links };
    let up = |name: &str| components.get(name).and_then(|c| c.get("up")).and_then(|v| v.as_bool());
    if let Some(v) = up("link:relay") { links.relay = v; }
    if let Some(v) = up("link:btc") { links.btc = v; }
    if let Some(v) = up("link:rates") { links.rates = v; }
    if let Some(v) = up("link:book") { links.book = v; }
    links
}


async fn backoff_or_shutdown(backoff: &mut Duration, shutdown_rx: &mut Receiver<()>) -> bool {
    let ms = backoff.as_millis() as u64;
    let wait = Duration::from_millis(rand::random_range(ms / 2..=ms));
    let stop = tokio::select! {
        _ = shutdown_rx.recv() => true,
        _ = tokio::time::sleep(wait) => false,
    };
    *backoff = (*backoff * 2).min(MAX_BACKOFF);
    stop
}

fn close_code(text: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(text).ok()?.get("code")?.as_u64()
}

fn book_command(pair: String, on: bool) -> RatesCommand {
    if on { RatesCommand::SubscribeBook(pair) } else { RatesCommand::UnsubscribeBook(pair) }
}

// Idle time past the proxy's 30 s ping, so a healthy socket never probes.
fn keepalive(stream: &TcpStream) {
    let socket = socket2::SockRef::from(stream);
    let probe = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(35))
        .with_interval(Duration::from_secs(5));
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let probe = probe.with_retries(3);
    let _ = socket.set_tcp_keepalive(&probe);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let _ = socket.set_tcp_user_timeout(Some(Duration::from_secs(20)));
}
