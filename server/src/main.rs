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
    #[command(about = "Inicia el balanceador de cargas.")]
    Balancer {
        #[arg(short, long, default_value = "0.0.0.0:8080", help = "Dirección IP y puerto donde escuchará el balanceador.")]
        listen_addr: String,
        #[arg(short, long, default_value = "0.0.0.0:4000", help = "Dirección IP y puerto para escuchar los anuncios UDP de los nodos.")]
        udp_addr: String,
    },
    #[command(about = "Inicia un nodo que anuncia sus servicios al balanceador.")]
    Node {
        #[arg(short = 'i', long, help = "Dirección IP del balanceador para enviar anuncios UDP.")]
        balancer_ip: String,
        #[arg(short = 'p', long, default_value_t = 4000, help = "Puerto UDP del balanceador.")]
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