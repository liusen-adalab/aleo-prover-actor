use std::time::Duration;

use tokio::sync::mpsc::{self, Sender};
use tokio::sync::oneshot;
use tokio::task;
use tracing::{debug, error};

#[derive(Default)]
pub struct Statistic {
    prove_weight: u32,
    prove_num: u32,

    submit_valid: u32,
    submit_invalid: u32,
}

#[derive(Debug)]
pub enum StatisticMsg {
    Prove(u32),
    SubmitResult(bool, Option<String>),
    Exit(oneshot::Sender<()>),
    Log
}

impl Statistic {
    pub fn start() -> Sender<StatisticMsg> {
        let (tx, mut rx) = mpsc::channel(100);
        task::spawn(async move {
            let mut statistic = Statistic::default();
            while let Some(msg) = rx.recv().await {
                match msg {
                    StatisticMsg::Prove(weight) => {
                        statistic.prove_num += 1;
                        statistic.prove_weight += weight;
                    }
                    StatisticMsg::SubmitResult(valid, msg) => {
                        if valid {
                            debug!("submit accepted {}", msg.unwrap_or("".into()));
                            statistic.submit_valid += 1;
                        } else {
                            debug!("submit rejected {}", msg.unwrap_or("".into()));
                            statistic.submit_invalid += 1;
                        }
                    }
                    StatisticMsg::Exit(responder) => {
                        responder.send(()).expect("failed to respond exit msg")
                    }
                    StatisticMsg::Log => {
                        todo!()
                    },

                }
            }
        });
        
        let self_sender = tx.clone();
        task::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if let Err(err) = self_sender.send(StatisticMsg::Log).await{
                    error!("statistic has exited: {err}");
                }
            }
        });
        tx
    }
}
