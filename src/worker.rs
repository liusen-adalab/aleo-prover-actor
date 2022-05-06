use std::{
    hint::spin_loop,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use aleo_mine_protocol::Message as PoolMessage;
use rand::thread_rng;
use rayon::{ThreadPool, ThreadPoolBuilder};
use snarkvm::{
    dpc::testnet2::Testnet2,
    prelude::{BlockHeader, BlockTemplate},
};
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot,
};
use tracing::{debug, error, info};

use crate::{client::ClientMsg, prover::ProverMsg, statistic::StatisticMsg};
use tokio::task;

pub struct Worker {
    pool: ThreadPool,
    terminator: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
    #[allow(dead_code)]
    prover_router: Sender<ProverMsg>,
    statistic_router: Sender<StatisticMsg>,
    client_router: Sender<ClientMsg>,
}

#[derive(Debug)]
pub enum WorkerMsg {
    Notify(Arc<BlockTemplate<Testnet2>>, u64),
    Exit(oneshot::Sender<()>),
}

impl Worker {
    pub fn start_cpu(
        prover_router: Sender<ProverMsg>,
        statistic_router: Sender<StatisticMsg>,
        client_router: Sender<ClientMsg>,
        threads: u8,
    ) -> Sender<WorkerMsg> {
        Self::start(prover_router, statistic_router, client_router, -1, threads)
    }

    #[cfg(feature = "cuda")]
    pub fn start_gpu(
        prover_router: Sender<ProverMsg>,
        statistic_router: Sender<StatisticMsg>,
        client_router: Sender<ClientMsg>,
        gpu_index: i16,
    ) -> Sender<WorkerMsg> {
        Self::start(prover_router, statistic_router, client_router, gpu_index, 2)
    }

    fn start(
        prover_router: Sender<ProverMsg>,
        statistic_router: Sender<StatisticMsg>,
        client_router: Sender<ClientMsg>,
        gpu_index: i16,
        threads: u8,
    ) -> Sender<WorkerMsg> {
        let (tx, mut rx) = mpsc::channel(100);
        let worker = Worker {
            pool: ThreadPoolBuilder::new().num_threads(threads as usize).build().unwrap(),
            terminator: Arc::new(AtomicBool::new(false)),
            ready: Arc::new(AtomicBool::new(true)),
            prover_router,
            statistic_router,
            client_router,
        };
        task::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    WorkerMsg::Notify(template, diff) => worker.new_work(template, diff, gpu_index).await,
                    WorkerMsg::Exit(responder) => {
                        worker.wait_for_terminator();
                        responder.send(()).expect("failed response exit msg");
                        break;
                    }
                }
            }
            debug!("worker terminated");
        });
        tx
    }

    fn wait_for_terminator(&self) {
        self.terminator.store(true, Ordering::SeqCst);
        while !self.ready.load(Ordering::SeqCst) {
            spin_loop();
        }
        self.terminator.store(false, Ordering::SeqCst);
        debug!("the pool is ready to go");
    }

    async fn new_work(&self, template: Arc<BlockTemplate<Testnet2>>, share_difficulty: u64, gpu_index: i16) {
        let height = template.block_height();
        debug!("starting new work: {}", height);
        self.wait_for_terminator();

        let terminator = self.terminator.clone();
        let ready = self.ready.clone();
        let statistic_router = self.statistic_router.clone();
        let client_router = self.client_router.clone();
        let (tx, rx) = oneshot::channel();
        self.pool.spawn(move || {
            ready.store(false, Ordering::SeqCst);
            // ensure new work starts before returning
            tx.send(()).unwrap();
            while !terminator.load(Ordering::SeqCst) {
                match BlockHeader::mine_once_unchecked(&template, &terminator, &mut thread_rng(), gpu_index) {
                    Ok(block_header) => {
                        let nonce = block_header.nonce();
                        let proof = block_header.proof().clone();
                        let proof_difficulty = proof.to_proof_difficulty().unwrap_or(u64::MAX);
                        if proof_difficulty > share_difficulty {
                            debug!(
                                "Share difficulty target not met: {} > {}",
                                proof_difficulty, share_difficulty
                            );
                            if let Err(err) = statistic_router
                                .try_send(StatisticMsg::Prove(false, (u64::MAX / share_difficulty) as u32))
                            {
                                error!("failed to report prove to statistic: {err}");
                            }
                            continue;
                        }
                        debug!(
                            "Share found for block {} with weight {}",
                            height,
                            u64::MAX / proof_difficulty
                        );
                        if let Err(err) =
                            client_router.try_send(ClientMsg::PoolMessage(PoolMessage::Submit(height, nonce, proof)))
                        {
                            error!("failed to send submit to client router: {err}");
                        }
                        if let Err(err) =
                            statistic_router.try_send(StatisticMsg::Prove(true, (u64::MAX / share_difficulty) as u32))
                        {
                            error!("failed to report prove to statistic: {err}");
                        }
                    }
                    Err(_) => {
                        info!("block {} terminated", height);
                        break;
                    }
                }
            }
            ready.store(true, Ordering::SeqCst);
        });
        rx.await.unwrap();
        debug!("spawned new work");
    }
}
