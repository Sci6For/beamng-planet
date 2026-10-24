use kissmp_server::*;
use log::{info};

#[tokio::main]
async fn main() {
    shared::init_logging();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--planet-config") {
        let result = match args.get(2) { Some(p) => planet_session::run(std::path::Path::new(p)).await, None => Err(anyhow::anyhow!("missing config")) };
        if let Err(e) = result { eprintln!("Planet: {}", e); std::process::exit(1); }
        return;
    }

    info!("Gas, Gas, Gas!");
    let path = std::path::Path::new("./mods/");
    if !path.exists() {
        std::fs::create_dir(path).unwrap();
    }
    let config = config::Config::load(std::path::Path::new("./config.json"));
    let server = Server::from_config(config);
    server.run(true, tokio::sync::oneshot::channel().1, None).await;
    std::process::exit(0);
}
