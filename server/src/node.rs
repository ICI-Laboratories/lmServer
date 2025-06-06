// node.rs
use std::io::{self, Write};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::interval;
use uuid::Uuid;
use log::{info, warn, error};

fn prompt_for_url(service_name: &str) -> Option<String> {
    print!("Introduce la URL completa para {} (ej: http://localhost:1234/v1/api) o deja en blanco si no aplica: ", service_name);
    io::stdout().flush().unwrap();
    let mut url = String::new();
    io::stdin().read_line(&mut url).expect("Error al leer la línea");
    let url = url.trim();
    if url.is_empty() {
        None
    } else {
        if url.starts_with("http://") || url.starts_with("https://") {
             Some(url.to_string())
        } else {
            warn!("URL inválida para {}. Debe empezar con http:// o https://. Ignorando.", service_name);
            None
        }
    }
}

async fn udp_broadcast_service(
    service_name: &str,
    unique_node_id: &str,
    service_url: &str,
    balancer_target: String,
) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    info!(
        "Anunciando {} (ID: {}) en {} al balanceador {}",
        service_name, unique_node_id, service_url, balancer_target
    );

    let msg = format!("DISCOVER,{},{},{}", service_name, unique_node_id, service_url);
    let mut ticker = interval(Duration::from_secs(10));

    loop {
        ticker.tick().await;
        match socket.send_to(msg.as_bytes(), &balancer_target).await {
            Ok(_) => {},
            Err(e) => error!(
                "Error al enviar broadcast UDP para {} (ID: {}): {}",
                service_name, unique_node_id, e
            ),
        }
    }
}

pub async fn run_node(balancer_ip: &str, balancer_port: u16) -> io::Result<()> {
    let hostname = hostname::get().ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".to_string());
    let node_uuid = Uuid::new_v4().to_string();
    let unique_node_id = format!("{}-{}", hostname, node_uuid);
    info!("Nodo iniciado con ID único: {}", unique_node_id);

    let lm_studio_url = prompt_for_url("LM Studio");
    let ollama_url = prompt_for_url("Ollama");

    if lm_studio_url.is_none() && ollama_url.is_none() {
        warn!("No se especificó ninguna URL de servicio. El nodo no anunciará nada.");
        return Ok(());
    }

    let balancer_target = format!("{}:{}", balancer_ip, balancer_port);
    let mut tasks = vec![];

    if let Some(url) = lm_studio_url {
        let target = balancer_target.clone();
        let id_clone = unique_node_id.clone();
        tasks.push(tokio::spawn(async move {
            udp_broadcast_service("lmstudio", &id_clone, &url, target).await
        }));
    }

    if let Some(url) = ollama_url {
        let target = balancer_target.clone();
        let id_clone = unique_node_id.clone();
        tasks.push(tokio::spawn(async move {
            udp_broadcast_service("ollama", &id_clone, &url, target).await
        }));
    }

    info!("Nodo anunciando servicios con ID {} cada 10 segundos. Presiona Ctrl+C para detener.", unique_node_id);

    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            info!("Cerrando nodo...");
        }
        Err(err) => {
            error!("Error al escuchar señal de interrupción: {}", err);
        }
    }

    Ok(())
}