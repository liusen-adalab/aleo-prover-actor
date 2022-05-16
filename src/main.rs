use std::{future, net::SocketAddr};

use aleo_prover_actor::create_key;
use aleo_prover_actor::prover::ProverHandler;
use anyhow::{Context, Result};
use log::error;
use snarkvm::dpc::testnet2::Testnet2;
use snarkvm::prelude::Address;
use structopt::StructOpt;
use tokio::runtime;
use tracing_log::LogTracer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;

#[derive(StructOpt, Debug)]
#[structopt(name = "aleo-miner", about = "A miner program of aleo pool")]
struct Opt {
    #[structopt(short)]
    /// use debug mode
    debug: bool,

    #[structopt(subcommand)]
    command: Command,
}

#[derive(StructOpt, Debug)]
enum Command {
    /// generate an aleo account
    GenKey,
    /// mine with cpu
    MineCpu {
        #[structopt(flatten)]
        info: Info,

        /// Worker is a thread pool used to calculate proof
        #[structopt(short, long, default_value = "1")]
        worker: u8,

        /// Number of threads that every worker will use
        /// It is recommended to ensure
        /// `worker * thread-per-worker` < `amount of threads of your device`
        #[structopt(short, long, default_value = "8")]
        #[structopt(verbatim_doc_comment)]
        thread_per_worker: u8,
    },

    #[cfg(feature = "cuda")]
    /// mine with gpu
    MineGpu {
        #[structopt(flatten)]
        info: Info,

        #[structopt(short, long)]
        #[structopt(verbatim_doc_comment)]
        /// example: --gpus 0 2 4
        /// it will use the gpu with index of 0, 2, 4, if they exist.
        /// it will use all gpus of your device by default
        gpus: Vec<u8>,

        /// Parallel worker per gpu
        #[structopt(short, long, default_value = "1")]
        worker_per_gpu: u8,
    },
}

#[derive(StructOpt, Debug)]
struct Info {
    #[structopt(short, long)]
    /// Your prover name
    name: String,

    #[structopt(short, long)]
    /// The address you mine for
    address: Address<Testnet2>,

    #[structopt(short, long)]
    /// Ip:port of the pool
    pool_ip: SocketAddr,
}

fn set_log(debug: bool) -> Result<()> {
    let level = if debug {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    LogTracer::builder().with_max_level(level).init()?;

    let filter = EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into());
    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(filter)
        .finish();
    let file = std::fs::File::create("./prover.log").context("failed to create log file")?;
    let file = tracing_subscriber::fmt::layer().with_writer(file).with_ansi(false);
    tracing::subscriber::set_global_default(subscriber.with(file))?;
    Ok(())
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    set_log(opt.debug).context("failed to initiate log")?;

    let rt = runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(4)
        .build()
        .context("failed to initiate tokio runtime")?;

    rt.block_on(async move {
        match opt.command {
            Command::GenKey => {
                println!("{}", create_key());
                return;
            }
            Command::MineCpu {
                info,
                worker,
                thread_per_worker,
            } => {
                let prover = ProverHandler::new();
                if let Err(err) = prover
                    .start_cpu(info.pool_ip, worker, thread_per_worker, info.name, info.address)
                    .await
                {
                    error!("failed to start prover: {err}");
                    return;
                }
            }
            #[cfg(feature = "cuda")]
            Command::MineGpu {
                info,
                gpus,
                worker_per_gpu,
            } => {
                let prover = ProverHandler::new();
                if let Err(err) = prover
                    .start_gpu(worker_per_gpu, gpus, info.address, info.name, info.pool_ip)
                    .await
                {
                    error!("failed to start prover: {err}");
                    return;
                }
            }
        }

        let () = future::pending().await;
    });

    Ok(())
}
