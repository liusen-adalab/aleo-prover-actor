#[cfg(feature = "cuda")]
use process::Command;
use std::{
    net::SocketAddr,
    process,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use anyhow::{ensure, Result};
use log::{debug, error, info, warn};
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
#[cfg(feature = "cuda")]
use rust_gpu_tools::Device;
use tokio::task;

pub struct Prover {
    workers: Vec<Sender<WorkerMsg>>,
}

#[derive(Debug)]
pub enum ProverMsg {
    NewWork(BlockTemplate<Testnet2>, u64),
    SubmitResult(bool, Option<String>),
    Exit(oneshot::Sender<()>),
}

impl ProverMsg {
    pub fn name(&self) -> &str {
        match self {
            ProverMsg::NewWork(..) => "new work",
            ProverMsg::SubmitResult(_, _) => "submit result",
            ProverMsg::Exit(_) => "exit",
        }
    }
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
        worker_per_gpu: u8,
        gpus: Vec<u8>,
        address: Address<Testnet2>,
        name: String,
        pool_ip: SocketAddr,
    ) -> Result<Sender<ProverMsg>> {
        let all = Device::all();
        ensure!(!all.is_empty(), "No available gpu in your device");

        let gpus = if gpus.is_empty() {
            all.iter().enumerate().map(|(a, _)| a as u8).collect()
        } else {
            gpus
        };

        let (prover_router, rx) = mpsc::channel(100);
        let client_router = Client::start(pool_ip, prover_router.clone(), name, address);
        let statistic_router = Statistic::start(client_router.clone());

        for index in gpus {
            for _ in 0..worker_per_gpu {
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

        Ok(prover_router)
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
                    ProverMsg::Exit(responder) => {
                        if let Err(err) = self.exit(&client_router, &statistic_router).await {
                            error!("failed to exit: {err}");
                            // grace exit failed, force exit
                            process::exit(1);
                        }
                        responder.send(()).unwrap();
                        break;
                    }
                    _ => {
                        if let Err(err) = self.process_msg(msg, &statistic_router) {
                            error!("prover failed to process message: {err}");
                        }
                    }
                }
            }
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
            _ => {
                warn!("unexpected msg");
            }
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
            let (tx, rx) = oneshot::channel();
            if let Err(err) = sender.send(ProverMsg::Exit(tx)).await {
                error!("failed to stop prover: {err}");
            }
            rx.await.unwrap();
            debug!("prover exited");
            self.running.store(false, Ordering::SeqCst);
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

    #[cfg(feature = "cuda")]
    pub async fn start_gpu(
        &self,
        worker_per_gpu: u8,
        gpus: Vec<u8>,
        address: Address<Testnet2>,
        name: String,
        pool_ip: SocketAddr,
    ) -> Result<()> {
        ensure!(detect_gpu(), "there is no cuda-tool-kit on your device");
        let address = Address::from_str(&address.to_string()).context("invalid aleo address")?;
        ensure!(!self.running(), "prover is already running");
        self.running.store(true, Ordering::SeqCst);

        let prover = Prover::new();
        let router = prover.start_gpu(worker_per_gpu, gpus, address, name, pool_ip).await?;
        let mut prover_router = self.prover_router.write().await;
        *prover_router = router;
        Ok(())
    }

    fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[cfg(feature = "cuda")]
fn detect_gpu() -> bool {
    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .arg("/C")
            .arg("nvcc --help")
            .output()
            .expect("failed to execute process")
    } else {
        Command::new("sh")
            .arg("-c")
            .arg("nvcc --help")
            .output()
            .expect("failed to execute process")
    };

    output.status.success()
}
