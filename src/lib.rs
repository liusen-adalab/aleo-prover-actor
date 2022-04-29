use snarkvm::{prelude::Account, dpc::testnet2::Testnet2};

pub mod prover;
mod worker;
mod client;
mod statistic;


pub fn create_key() -> String{
    let account = Account::<Testnet2>::new(&mut rand::thread_rng());
    let private_key = format!("Private key:  {}\n", account.private_key());
    let view_key = format!("   View key:  {}\n", account.view_key());
    let address = format!("    Address:  {}\n", account.address());
    let mut result = String::new();
    result += &private_key;
    result += &view_key;
    result += &address;
    result += "\nWARNING: Make sure you have a backup of both private key and view key!\n";
    result += "                  Nobody can help you recover those keys if you lose them!\n";

    result
}