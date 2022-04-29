use std::{future, net::SocketAddr, str::FromStr};

use aleo_prover_actor::prover::Prover;
use snarkvm::prelude::Address;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    set_log();
    let address = Address::from_str(
        "aleo17jz68jzshl4l2d5cwr8zwpl9ntwpusxpslmmp97s4296tay3gsgsxevmne",
    )
    .unwrap();
    let prover = Prover::new("test1".to_string(), address);
    let _ = prover
        .start_cpu(SocketAddr::from_str("14.29.101.215:4040").unwrap())
        .await;

    future::pending().await
}

fn set_log() {
    let filter =
        EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into());
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(filter)
        .finish();
    let file = std::fs::File::create("./pool.log").unwrap();
    let file = tracing_subscriber::fmt::layer()
        .with_writer(file)
        .with_ansi(false);
    tracing::subscriber::set_global_default(subscriber.with(file)).unwrap();
}
