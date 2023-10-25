use std::{net::SocketAddr, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use log::*;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use tokio_tungstenite::{
    accept_async,
    tungstenite::{Error, Message, Result},
    WebSocketStream,
};

type Tx = futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>;
type PeerMap = Arc<Mutex<Vec<Tx>>>;

async fn accept_connection(peer: SocketAddr, stream: TcpStream, peers: PeerMap) {
    if let Err(e) = handle_connection(peer, stream, peers).await {
        match e {
            Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
            err => error!("Error processing connection: {}", err),
        }
    }
}

async fn handle_connection(peer: SocketAddr, stream: TcpStream, peers: PeerMap) -> Result<()> {
    let ws_stream = accept_async(stream).await.expect("Failed to accept");
    let (tx, mut rx) = ws_stream.split();

    peers.lock().await.push(tx);

    info!("New WebSocket connection: {}", peer);

    while let Some(msg) = rx.next().await {
        let msg = msg?;
        if msg.is_text() || msg.is_binary() {
            // Echo the message back to the client
            for peer in peers.lock().await.iter_mut() {
                peer.send(msg.clone()).await?;
            }
        }
    }

    Ok(())
}

pub struct WebSocket {
    peers: PeerMap,
}

impl WebSocket {
    pub async fn run(addr: &str) -> WebSocket {
        let peers: PeerMap = Arc::new(Mutex::new(Vec::new()));

        let addr = addr.to_string();
        let peers_clone = peers.clone();

        tokio::spawn(async move {
            let listener = TcpListener::bind(&addr).await.expect("Can't listen");
            info!("Listening on: {}", addr);

            while let Ok((stream, _)) = listener.accept().await {
                let peer = stream
                    .peer_addr()
                    .expect("connected streams should have a peer address");
                info!("Peer address: {}", peer);

                tokio::spawn(accept_connection(peer, stream, peers_clone.clone()));
            }
        });

        WebSocket { peers }
    }

    pub async fn send(&self, msg: String) {
        for peer in self.peers.lock().await.iter_mut() {
            if let Err(e) = peer.send(Message::text(msg.clone())).await {
                match e {
                    Error::ConnectionClosed | Error::Protocol(_) | Error::Utf8 => (),
                    Error::Io(ref err)
                        if err.kind() == std::io::ErrorKind::ConnectionReset
                            || err.kind() == std::io::ErrorKind::BrokenPipe => {},
                    _ => error!("Error sending message: {}", e),
                }
            }
        }
    }
}
