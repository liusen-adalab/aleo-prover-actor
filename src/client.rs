use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{bail, Result};
use futures_util::sink::SinkExt;
use pool_prover_message::PoolMessage;
use snarkvm::dpc::testnet2::Testnet2;
use snarkvm::prelude::Address;
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::oneshot;
use tokio::{
    net::TcpStream,
    task,
    time::{sleep, timeout},
};
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;
use tracing::{debug, error, info, warn};

use crate::prover::ProverMsg;
use anyhow::Context;

pub(crate) struct Client;

#[derive(Debug)]
pub enum ClientMsg {
    PoolMessage(PoolMessage),
    Exit(oneshot::Sender<()>),
}

impl Client {
    pub fn start(
        pool_ip: SocketAddr,
        prover_router: Sender<ProverMsg>,
        name: String,
        address: Address<Testnet2>,
    ) -> Sender<ClientMsg> {
        let (router, mut receiver) = mpsc::channel(10);
        let client = Client;
        task::spawn(async move {
            let mut framed = client.connect_pool(pool_ip, address, name).await;

            loop {
                tokio::select! {
                    Some(message) = receiver.recv() => {
                        match message{
                            ClientMsg::PoolMessage(msg) => {
                                let name = msg.name();
                                debug!("Sending {} to server", name);
                                if let Err(e) = framed.send(msg).await {
                                    error!("Error sending {}: {:?}", name, e);
                                }
                            }
                            ClientMsg::Exit(sender) => {
                                sender.send(()).expect("client failed to respond exit msg");
                            },
                        }
                    }
                    result = framed.next() => match result {
                        Some(Ok(message)) => {
                            if Self::process_message(message, &prover_router).await.is_err(){
                                info!("error process message, breaking");
                                // todo: maybe it is better to exit process
                                break ;
                            }
                        }
                        Some(Err(e)) => {
                            warn!("Failed to read the message: {:?}", e);
                        }
                        None => {
                            error!("Disconnected from server");
                            sleep(Duration::from_secs(5)).await;
                            break;
                        }
                    }
                }
            }
        });
        router
    }

    async fn connect_pool(
        &self,
        pool_ip: SocketAddr,
        address: Address<Testnet2>,
        name: String,
    ) -> Framed<TcpStream, PoolMessage> {
        loop {
            match timeout(Duration::from_secs(5), TcpStream::connect(pool_ip)).await {
                Ok(socket) => match socket {
                    Ok(socket) => {
                        info!("Connected to {}", &pool_ip);
                        let mut framed = Framed::new(socket, PoolMessage::Canary);
                        Self::authorize(&mut framed, address, name.clone()).await;
                        return framed;
                    }
                    Err(e) => {
                        error!("Failed to connect to operator: {}", e);
                        sleep(Duration::from_secs(5)).await;
                    }
                },
                Err(_) => {
                    error!("Failed to connect to operator: Timed out");
                }
            }
        }
    }

    async fn authorize(
        framed: &mut Framed<TcpStream, PoolMessage>,
        address: Address<Testnet2>,
        name: String,
    ) {
        let authorization =
            PoolMessage::Authorize(address, name, *PoolMessage::version());
        if let Err(e) = framed.send(authorization).await {
            error!("Error sending authorization: {}", e);
        } else {
            debug!("Sent authorization");
        }
    }

    async fn process_message(
        message: PoolMessage,
        prover_sender: &Sender<ProverMsg>,
    ) -> Result<()> {
        debug!("Received {} from server", message.name());
        match message {
            PoolMessage::AuthorizeResult(result, message) => {
                if result {
                    debug!("Authorized");
                } else {
                    bail!(
                        "Authorization failed: {}",
                        message.unwrap_or("".to_string())
                    );
                }
            }
            PoolMessage::Notify(block_template, share_difficulty) => {
                prover_sender
                    .send(ProverMsg::NewWork(block_template, share_difficulty))
                    .await
                    .context("failed to send notify to prover")?;
                debug!("Sent work to prover");
            }
            PoolMessage::SubmitResult(success, message) => {
                prover_sender
                    .send(ProverMsg::SubmitResult(success, message))
                    .await
                    .context("")?;
                debug!("Sent share result to prover");
            }
            _ => {
                debug!("Unhandled message: {}", message.name());
            }
        }
        Ok(())
    }
}
