use std::{
    hint::spin_loop,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

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

use crate::{prover::ProverMsg, statistic::StatisticMsg};
use tokio::task;

pub struct Worker {
    pool: ThreadPool,
    terminator: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
    prover_router: Sender<ProverMsg>,
    statistic_router: Sender<StatisticMsg>,
}

#[derive(Debug)]
pub enum WorkerMsg {
    Notify(Arc<BlockTemplate<Testnet2>>, u64),
    Exit(oneshot::Sender<()>),
}

impl Worker {
    pub fn new(
        prover_router: Sender<ProverMsg>,
        statistic_router: Sender<StatisticMsg>,
    ) -> Sender<WorkerMsg> {
        let (tx, mut rx) = mpsc::channel(100);
        let worker = Worker {
            pool: ThreadPoolBuilder::new().num_threads(8).build().unwrap(),
            terminator: Arc::new(AtomicBool::new(false)),
            ready: Arc::new(AtomicBool::new(true)),
            prover_router,
            statistic_router,
        };
        task::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    WorkerMsg::Notify(template, diff) => {
                        worker.new_work(template, diff).await
                    }
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
    }

    async fn new_work(
        &self,
        template: Arc<BlockTemplate<Testnet2>>,
        share_difficulty: u64,
    ) {
        self.wait_for_terminator();

        let terminator = self.terminator.clone();
        let block_height = template.block_height();
        let ready = self.ready.clone();
        let prover_router = self.prover_router.clone();
        let statistic_router = self.statistic_router.clone();
        let (tx, rx) = oneshot::channel();
        self.pool.spawn(move || {
            ready.store(false, Ordering::SeqCst);
            // todo: make sure excute here before return
            tx.send(()).unwrap();
            while !terminator.load(Ordering::SeqCst) {
                match BlockHeader::mine_once_unchecked(
                    &template,
                    &terminator,
                    &mut thread_rng(),
                    -1,
                ) {
                    Ok(block_header) => {
                        if let Err(err) = statistic_router.try_send(StatisticMsg::Prove(
                            (u64::MAX / share_difficulty) as u32,
                        )) {
                            error!("failed to report prove to statistic: {err}");
                        }

                        let nonce = block_header.nonce();
                        let proof = block_header.proof().clone();
                        let proof_difficulty =
                            proof.to_proof_difficulty().unwrap_or(u64::MAX);
                        if proof_difficulty > share_difficulty {
                            debug!(
                                "Share difficulty target not met: {} > {}",
                                proof_difficulty, share_difficulty
                            );
                            continue;
                        }
                        debug!(
                            "Share found for block {} with weight {}",
                            block_height,
                            u64::MAX / proof_difficulty
                        );
                        if let Err(err) = prover_router.try_send(ProverMsg::Submit(
                            nonce,
                            proof,
                            block_height,
                        )) {
                            error!("Failed to submit share: {}", err);
                        }
                    }
                    Err(_) => {
                        info!("block {} terminated", block_height);
                        break;
                    }
                }
            }
            ready.store(false, Ordering::SeqCst);
        });

        rx.await.unwrap();
    }
}
