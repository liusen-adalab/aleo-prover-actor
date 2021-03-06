use std::collections::VecDeque;
use std::time::Duration;

use aleo_mine_protocol::Message;
use ansi_term::Color::{Cyan, Green, Red};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::oneshot;
use tokio::task::{self, JoinHandle};
use log::{error, info, debug};

use crate::client::ClientMsg;

pub struct Statistic {
    prove_weight_valid: u32,
    prove_weight_invalid: u32,
    prove_count: u32,

    submit_valid_count: u32,
    submit_invalid_count: u32,

    client_router: Sender<ClientMsg>,
    
    report_handler: JoinHandle<()>
}

#[derive(Debug)]
pub enum StatisticMsg {
    Prove(bool, u32),
    SubmitResult(bool, Option<String>),
    Exit(oneshot::Sender<()>),
    Report,
}

impl Statistic {
    pub fn start(client_router: Sender<ClientMsg>) -> Sender<StatisticMsg> {
        let (tx, rx) = mpsc::channel(100);
        let handler = Self::period_report(tx.clone());
        let statistic = Statistic {
            prove_weight_valid: Default::default(),
            prove_weight_invalid: Default::default(),
            prove_count: Default::default(),
            submit_valid_count: Default::default(),
            submit_invalid_count: Default::default(),
            client_router,
            report_handler: handler
        };
        statistic.serve(rx);
        info!("statistic mod started");
        tx
    }

    fn serve(mut self, mut rx: Receiver<StatisticMsg>) {
        task::spawn(async move {
            let mut log = VecDeque::with_capacity(60);
            while let Some(msg) = rx.recv().await {
                match msg {
                    StatisticMsg::Prove(valid, weight) => {
                        self.prove_count += 1;
                        if valid {
                            self.prove_weight_valid += weight;
                        } else {
                            self.prove_weight_invalid += weight;
                        }
                    }
                    StatisticMsg::SubmitResult(is_valid, msg) => {
                        let msg = msg.map(|msg| ": ".to_string() + &msg).unwrap_or("".into());
                        if is_valid {
                            self.submit_valid_count += 1;
                            let valid = self.submit_valid_count;
                            let invalid = self.submit_invalid_count;
                            info!(
                                "{}",
                                Green.normal().paint(format!(
                                    "Share accepted{} {} / {} ({:.2}%)",
                                    msg,
                                    valid,
                                    valid + invalid,
                                    (valid as f64 / (valid + invalid) as f64) * 100.0
                                ))
                            );
                        } else {
                            self.submit_invalid_count += 1;
                            let valid = self.submit_valid_count;
                            let invalid = self.submit_invalid_count;
                            info!(
                                "{}",
                                Red.normal().paint(format!(
                                    "Share rejected{} {} / {} ({:.2}%)",
                                    msg,
                                    valid,
                                    valid + invalid,
                                    (valid as f64 / (valid + invalid) as f64) * 100.0
                                ))
                            );
                        }
                    }
                    StatisticMsg::Exit(responder) => {
                        self.report_handler.abort();
                        responder.send(()).expect("failed to respond exit msg");
                        debug!("statistic exited");
                        return
                    },
                    StatisticMsg::Report => {
                        let m1 = log.get(0).map(|a| *a);
                        let m5 = log.get(4).map(|a| *a);
                        let m15 = log.get(9).map(|a| *a);
                        let m30 = log.get(29).map(|a| *a);
                        let m60 = log.get(59).map(|a| *a);
                        log.push_front(self.prove_count);

                        let count_1m = self.prove_count - *m1.as_ref().unwrap_or(&0);
                        if let Err(err) = self
                            .client_router
                            .try_send(ClientMsg::PoolMessage(Message::ProvePerMinute(count_1m as u64)))
                        {
                            error!("filed to send prove num to client router: {err}");
                        }

                        info!(
                            "{}",
                            Cyan.normal().paint(format!(
                                "Total proofs: {} (1m: {} p/s, 5m: {} p/s, 15m: {} p/s, 30m: {} p/s, 60m: {} p/s)",
                                self.prove_count,
                                self.calculate_proof_rate(m1, Duration::from_secs(60)),
                                self.calculate_proof_rate(m5, Duration::from_secs(60 * 5)),
                                self.calculate_proof_rate(m15, Duration::from_secs(60 * 15)),
                                self.calculate_proof_rate(m30, Duration::from_secs(60 * 30)),
                                self.calculate_proof_rate(m60, Duration::from_secs(60 * 60)),
                            ))
                        );
                    }
                }
            }
        });
    }

    fn period_report(self_sender: Sender<StatisticMsg>) -> JoinHandle<()>{
        task::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if let Err(err) = self_sender.send(StatisticMsg::Report).await {
                    error!("statistic has exited: {err}");
                }
            }
        })
    }

    fn calculate_proof_rate(&self, past: Option<u32>, interval: Duration) -> String {
        match past {
            Some(past) => {
                let interval = interval.as_secs_f64();
                let rate = (self.prove_count - past) as f64 / interval;
                format!("{:.2}", rate)
            }
            None => {
                format!("---")
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::collections::VecDeque;

    #[test]
    fn test_vecdeque() {
        let log = VecDeque::from(vec![0;60]);
        assert!(log.get(0).is_some());
        let mut log = VecDeque::with_capacity(60);
        assert!(log.get(0).is_none());
        let cap = log.capacity();
        for i in 1..=cap {
            log.push_front(i);
        }
        assert_eq!(log.get(0).unwrap(), &cap);
        assert_eq!(log.get(cap - 1).unwrap(), &1);
        log.push_front(1);
        assert_eq!(log.get(cap - 1).unwrap(), &2);
    }
}
