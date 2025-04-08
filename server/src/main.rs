use clap::Parser;
use std::io;

mod balancer;
mod node;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Inicia el balanceador de cargas.
    Balancer {
        /// Dirección IP y puerto donde escuchará el balanceador.
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        listen_addr: String,
        /// Dirección IP y puerto para escuchar los anuncios UDP de los nodos.
        #[arg(short, long, default_value = "0.0.0.0:4000")]
        udp_addr: String,
    },
    /// Inicia un nodo que anuncia sus servicios al balanceador.
    Node {
        /// Dirección IP del balanceador para enviar anuncios UDP.
        #[arg(short, long)]
        balancer_ip: String,
        /// Puerto UDP del balanceador (debe coincidir con el puerto UDP del balanceador).
        #[arg(short, long, default_value_t = 4000)]
        balancer_port: u16,
    },
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Balancer { listen_addr, udp_addr } => {
            println!("Iniciando en modo Balanceador...");
            balancer::run_balancer(&listen_addr, &udp_addr).await?;
        }
        Commands::Node { balancer_ip, balancer_port } => {
            println!("Iniciando en modo Nodo...");
            node::run_node(&balancer_ip, balancer_port).await?;
        }
    }

    Ok(())
}