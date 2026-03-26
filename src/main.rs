use credit_data_simulator::{SimulatorServer, SimulatorConfig};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive(tracing::Level::INFO.into()))
        .init();

    println!("Starting Credit Data Simulator...");

    let config = SimulatorConfig::default();
    match SimulatorServer::start(config).await {
        Ok(_server) => {
            println!("All simulators are running and healthy.");
            tokio::signal::ctrl_c().await.unwrap();
            println!("Shutting down simulators...");
        }
        Err(e) => {
            eprintln!("Failed to start simulators: {}", e);
            std::process::exit(1);
        }
    }
}
