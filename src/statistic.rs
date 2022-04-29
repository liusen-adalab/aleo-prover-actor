use std::collections::VecDeque;
use std::time::Duration;

use ansi_term::Color::{Cyan, Green, Red};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::oneshot;
use tokio::task;
use tracing::{error, info};

#[derive(Default)]
pub struct Statistic {
    prove_weight_valid: u32,
    prove_weight_invalid: u32,
    prove_count: u32,

    submit_valid_count: u32,
    submit_invalid_count: u32,
}

#[derive(Debug)]
pub enum StatisticMsg {
    Prove(bool, u32),
    SubmitResult(bool, Option<String>),
    Exit(oneshot::Sender<()>),
    Report,
}

impl Statistic {
    pub fn start() -> Sender<StatisticMsg> {
        let (tx, rx) = mpsc::channel(100);
        let statistic = Statistic::default();
        statistic.serve(rx);
        Self::period_report(tx.clone());
        tx
    }

    fn serve(mut self, mut rx: Receiver<StatisticMsg>) {
        task::spawn(async move {
            let mut log = VecDeque::from(vec![0u32; 6]);
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
                        let msg =
                            msg.map(|msg| ": ".to_string() + &msg).unwrap_or("".into());
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
                        responder.send(()).expect("failed to respond exit msg")
                    }
                    StatisticMsg::Report => {
                        let m1 = *log.get(0).unwrap_or(&0);
                        let m5 = *log.get(4).unwrap_or(&0);
                        let m15 = *log.get(9).unwrap_or(&0);
                        let m30 = *log.get(29).unwrap_or(&0);
                        let m60 = *log.get(59).unwrap_or(&0);
                        log.push_front(self.prove_count);

                        info!(
                            "{}",
                            Cyan.normal().paint(format!(
                                "Total proofs: {} (1m: {} p/s, 5m: {} p/s, 15m: {} p/s, 30m: {} p/s, 60m: {} p/s)",
                                self.prove_count,
                                self.calculate_proof_rate( m1, Duration::from_secs(60)),
                                self.calculate_proof_rate( m5, Duration::from_secs(60 * 5)),
                                self.calculate_proof_rate( m15, Duration::from_secs(60 * 15)),
                                self.calculate_proof_rate( m30, Duration::from_secs(60 * 30)),
                                self.calculate_proof_rate( m60, Duration::from_secs(60 * 60)),
                            ))
                        );
                    }
                }
            }
        });
    }

    fn period_report(self_sender: Sender<StatisticMsg>) {
        task::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if let Err(err) = self_sender.send(StatisticMsg::Report).await {
                    error!("statistic has exited: {err}");
                }
            }
        });
    }

    fn calculate_proof_rate(&self, past: u32, interval: Duration) -> String {
        let interval = interval.as_secs_f64();
        let rate = (self.prove_count - past) as f64 / interval;
        format!("{:.2}", rate)
    }
}
