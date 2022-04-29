use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use pool_prover_message::PoolMessage;
use snarkvm::{
    dpc::{testnet2::Testnet2, PoSWProof},
    prelude::{Address, BlockTemplate, Network},
};
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot,
};
use tracing::{error, info};

use crate::{
    client::{Client, ClientMsg},
    statistic::{Statistic, StatisticMsg},
    worker::{Worker, WorkerMsg},
};
use anyhow::Context;
use tokio::task;

pub struct Prover {
    workers: Vec<Sender<WorkerMsg>>,
    name: String,
    address: Address<Testnet2>,
}

#[derive(Debug)]
pub enum ProverMsg {
    NewWork(BlockTemplate<Testnet2>, u64),
    SubmitResult(bool, Option<String>),
    Submit(<Testnet2 as Network>::PoSWNonce, PoSWProof<Testnet2>, u32),
    Exit,
}

impl Prover {
    pub fn new(name: String, address: Address<Testnet2>) -> Prover {
        Prover {
            workers: vec![],
            name,
            address,
        }
    }

    pub async fn start_cpu(mut self, pool_ip: SocketAddr) -> Result<ProverHandler> {
        let (tx, mut rx) = mpsc::channel(100);
        let statistic_router = Statistic::start();
        self.workers.push(Worker::new(tx.clone(), statistic_router.clone()));
        let client_router =
            Client::start(pool_ip, tx.clone(), self.name.clone(), self.address);

        task::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    ProverMsg::Exit => {
                        if let Err(err) =
                            self.exit(&client_router, &statistic_router).await
                        {
                            error!("failed to exit: {err}");
                        }
                        break;
                    }
                    _ => {
                        if let Err(err) =
                            self.process_msg(msg, &client_router, &statistic_router)
                        {
                            error!("prover failed to process message: {err}");
                        }
                    }
                }
            }
            info!("prover exited");
        });
        Ok(ProverHandler { sender: tx.clone() })
    }

    async fn exit(
        &mut self,
        client_router: &Sender<ClientMsg>,
        statistic_router: &Sender<StatisticMsg>,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        client_router
            .send(ClientMsg::Exit(tx))
            .await
            .context("client")?;
        rx.await.context("failed to get exit response of client")?;

        for worker in self.workers.iter() {
            let (tx, rx) = oneshot::channel();
            worker.send(WorkerMsg::Exit(tx)).await.context("worker")?;
            rx.await.context("failed to get exit response of worker")?;
        }
        let (tx, rx) = oneshot::channel();
        statistic_router
            .send(StatisticMsg::Exit(tx))
            .await
            .context("statistic")?;
        rx.await
            .context("failed to get exit response of statistic mod")?;
        Ok(())
    }

    pub fn start_gpu() -> Result<()> {
        todo!()
    }

    fn process_msg(
        &mut self,
        msg: ProverMsg,
        client_router: &Sender<ClientMsg>,
        statistic_router: &Sender<StatisticMsg>,
    ) -> Result<()> {
        match msg {
            ProverMsg::NewWork(template, difficulty) => {
                let template = Arc::new(template);
                for worker in self.workers.iter() {
                    worker.try_send(WorkerMsg::Notify(template.clone(), difficulty))?;
                }
            }
            ProverMsg::SubmitResult(valid, msg) => {
                if let Err(err) = statistic_router.try_send(StatisticMsg::SubmitResult(valid, msg))
                {
                    error!("failed to send submit result to statistic mod: {err}");
                }
            }
            ProverMsg::Submit(nonce, proof, height) => {
                client_router
                    .try_send(ClientMsg::PoolMessage(PoolMessage::Submit(
                        height, nonce, proof,
                    )))
                    .context("failed to send submit to client router")?;
            }
            ProverMsg::Exit => {}
        }

        Ok(())
    }
}

pub struct ProverHandler {
    sender: Sender<ProverMsg>,
}

impl ProverHandler {
    pub async fn stop(&self) {
        if let Err(err) = self.sender.send(ProverMsg::Exit).await {
            error!("failed to stop prover: {err}");
        }
    }
}
