use clap::Parser;
use std::io; 
use log::{info, LevelFilter}; 
use fern::colors::{Color, ColoredLevelConfig};

mod balancer;
mod node;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, value_name = "LEVEL", global = true, default_value = "info", help = "Establece el nivel de log (trace, debug, info, warn, error)")]
    log_level: LevelFilter,

    #[arg(long, value_name = "FILE", global = true, default_value = "output.log", help = "Nombre del archivo para guardar los logs.")]
    log_file: String,
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

fn setup_logging(level: LevelFilter, log_file: &str) -> Result<(), fern::InitError> {
    let colors_line = ColoredLevelConfig::new()
        .error(Color::Red)
        .warn(Color::Yellow)
        .info(Color::Green)
        .debug(Color::Blue)
        .trace(Color::BrightBlack);

    let colors_level = colors_line.clone().info(Color::Green);

    let base_config = fern::Dispatch::new()
        .format(move |out, message, record| {
            out.finish(format_args!(
                "{} [{:<5}] [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(level)
        .level_for("hyper", LevelFilter::Info)
        .level_for("reqwest", LevelFilter::Info);


    let console_config = fern::Dispatch::new()
        .format(move |out, message, record| {
            out.finish(format_args!(
                "\x1B[{}m{} [{:<5}]\x1B[0m [{}] {}",
                colors_line.get_color(&record.level()).to_fg_str(),
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                colors_level.color(record.level()),
                record.target(),
                message
            ))
        })
        .chain(std::io::stdout());

    let file_config = fern::Dispatch::new()
        .chain(fern::log_file(log_file)?);

    base_config
        .chain(console_config)
        .chain(file_config)
        .apply()?;

    Ok(())
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();

    if let Err(e) = setup_logging(cli.log_level, &cli.log_file) {
        eprintln!("Error inicializando el logger: {}", e);
    }

    info!("Logging inicializado. Nivel: {}, Archivo: {}", cli.log_level, cli.log_file);

    match cli.command {
        Commands::Balancer { listen_addr, udp_addr } => {
            info!("Iniciando en modo Balanceador...");
            balancer::run_balancer(&listen_addr, &udp_addr).await?;
        }
        Commands::Node { balancer_ip, balancer_port } => {
            info!("Iniciando en modo Nodo...");
            node::run_node(&balancer_ip, balancer_port).await?;
        }
    }

    Ok(())
}