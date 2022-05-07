use std::{
    net::SocketAddr,
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, 
    }, str::FromStr,
};

use anyhow::{ensure, Result};
use log::{error, info, debug};
use snarkvm::{
    dpc::testnet2::Testnet2,
    prelude::{Address, BlockTemplate},
};
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    oneshot, RwLock,
};

use crate::{
    client::{Client, ClientMsg},
    statistic::{Statistic, StatisticMsg},
    worker::{Worker, WorkerMsg},
};
use anyhow::Context;
use tokio::task;
#[cfg(feature = "cuda")]
use {anyhow::bail, rust_gpu_tools::Device};

pub struct Prover {
    workers: Vec<Sender<WorkerMsg>>,
}

#[derive(Debug)]
pub enum ProverMsg {
    NewWork(BlockTemplate<Testnet2>, u64),
    SubmitResult(bool, Option<String>),
    Exit,
}

impl Prover {
    fn new() -> Prover {
        Prover { workers: vec![] }
    }

    async fn start_cpu(
        mut self,
        worker: u8,
        thread_per_worker: u8,
        address: Address<Testnet2>,
        name: String,
        pool_ip: SocketAddr,
    ) -> Result<Sender<ProverMsg>> {
        let (prover_router, rx) = mpsc::channel(100);
        let client_router = Client::start(pool_ip, prover_router.clone(), name, address);
        let statistic_router = Statistic::start(client_router.clone());
        for _ in 0..worker {
            self.workers.push(Worker::start_cpu(
                prover_router.clone(),
                statistic_router.clone(),
                client_router.clone(),
                thread_per_worker,
            ));
        }
        info!(
            "created {} workers with {} threads each for the prover",
            self.workers.len(),
            thread_per_worker
        );

        self.serve(rx, client_router, statistic_router);
        info!("prover-cpu started");
        Ok(prover_router)
    }

    #[cfg(feature = "cuda")]
    async fn start_gpu(
        mut self,
        worker: u8,
        gpus: Vec<u8>,
        address: Address<Testnet2>,
        name: String,
        pool_ip: SocketAddr,
    ) -> Result<ProverHandler> {
        let all = Device::all();
        if all.is_empty() {
            bail!("No available gpu in your device");
        }
        let gpus = if gpus.is_empty() {
            all.iter().enumerate().map(|(a, _)| a as u8).collect()
        } else {
            gpus
        };

        let (prover_router, rx) = mpsc::channel(100);
        let statistic_router = Statistic::start();
        let client_router = Client::start(pool_ip, prover_router.clone(), self.name.clone(), self.address);

        for index in gpus {
            for _ in 0..worker {
                self.workers.push(Worker::start_gpu(
                    prover_router.clone(),
                    statistic_router.clone(),
                    client_router.clone(),
                    index as i16,
                ));
                info!("started worker on gpu-{}", index);
            }
        }

        self.serve(rx, client_router, statistic_router);

        Ok(ProverHandler {
            sender: prover_router.clone(),
        })
    }

    fn serve(
        mut self,
        mut rx: Receiver<ProverMsg>,
        client_router: Sender<ClientMsg>,
        statistic_router: Sender<StatisticMsg>,
    ) {
        task::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    ProverMsg::Exit => {
                        if let Err(err) = self.exit(&client_router, &statistic_router).await {
                            error!("failed to exit: {err}");
                            // grace exit failed, force exit
                            process::exit(1);
                        }
                        break;
                    }
                    _ => {
                        if let Err(err) = self.process_msg(msg, &statistic_router) {
                            error!("prover failed to process message: {err}");
                        }
                    }
                }
            }
            debug!("prover exited");
        });
    }

    fn process_msg(&mut self, msg: ProverMsg, statistic_router: &Sender<StatisticMsg>) -> Result<()> {
        match msg {
            ProverMsg::NewWork(template, difficulty) => {
                let template = Arc::new(template);
                for worker in self.workers.iter() {
                    worker.try_send(WorkerMsg::Notify(template.clone(), difficulty))?;
                }
            }
            ProverMsg::SubmitResult(valid, msg) => {
                if let Err(err) = statistic_router.try_send(StatisticMsg::SubmitResult(valid, msg)) {
                    error!("failed to send submit result to statistic mod: {err}");
                }
            }
            ProverMsg::Exit => {}
        }

        Ok(())
    }

    async fn exit(&mut self, client_router: &Sender<ClientMsg>, statistic_router: &Sender<StatisticMsg>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        client_router.send(ClientMsg::Exit(tx)).await.context("client")?;
        rx.await.context("failed to get exit response of client")?;

        for (i, worker) in self.workers.iter().enumerate() {
            let (tx, rx) = oneshot::channel();
            worker.send(WorkerMsg::Exit(tx)).await.context("worker")?;
            rx.await.context("failed to get exit response of worker")?;
            debug!("worker {i} terminated");
        }
        let (tx, rx) = oneshot::channel();
        statistic_router
            .send(StatisticMsg::Exit(tx))
            .await
            .context("statistic")?;
        rx.await.context("failed to get exit response of statistic mod")?;
        Ok(())
    }
}

pub struct ProverHandler {
    running: AtomicBool,
    prover_router: RwLock<Sender<ProverMsg>>,
}

impl ProverHandler {
    pub fn new() -> Self {
        let (tx, _) = mpsc::channel(1);
        Self {
            running: AtomicBool::new(false),
            prover_router: RwLock::new(tx),
        }
    }

    pub async fn stop(&self) {
        if self.running() {
            let sender = self.prover_router.read().await;
            if let Err(err) = sender.send(ProverMsg::Exit).await {
                error!("failed to stop prover: {err}");
            }
        }
    }

    pub async fn start_cpu(
        &self,
        pool_ip: SocketAddr,
        worker: u8,
        thread_per_worker: u8,
        name: String,
        address: impl ToString,
    ) -> Result<()> {
        let address = Address::from_str(&address.to_string()).context("invalid aleo address")?;
        ensure!(!self.running(), "prover is already running");
        self.running.store(true, Ordering::SeqCst);

        let prover = Prover::new();
        let router = prover
            .start_cpu(worker, thread_per_worker, address, name, pool_ip)
            .await?;
        let mut prover_router = self.prover_router.write().await;
        *prover_router = router;
        Ok(())
    }

    fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
