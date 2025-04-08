// src/node.rs

use std::io::{self, Write};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::interval;

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
            eprintln!("URL inválida para {}. Debe empezar con http:// o https://. Ignorando.", service_name);
            None
        }
    }
}

async fn udp_broadcast_service(
    service_name: &str,
    service_url: &str,
    balancer_target: String,
) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    println!(
        "Anunciando {} en {} al balanceador {}",
        service_name, service_url, balancer_target
    );

    let msg = format!("DISCOVER,{},{}", service_name, service_url);
    let mut ticker = interval(Duration::from_secs(10));

    loop {
        ticker.tick().await;
        match socket.send_to(msg.as_bytes(), &balancer_target).await {
            Ok(_) => {
                 // Bloque Ok vacío después de eliminar el código no usado
            },
            Err(e) => eprintln!(
                "Error al enviar broadcast UDP para {}: {}",
                service_name, e
            ),
        }
    }
}

pub async fn run_node(balancer_ip: &str, balancer_port: u16) -> io::Result<()> {
    let lm_studio_url = prompt_for_url("LM Studio");
    let ollama_url = prompt_for_url("Ollama");

    if lm_studio_url.is_none() && ollama_url.is_none() {
        eprintln!("No se especificó ninguna URL de servicio. El nodo no anunciará nada.");
        return Ok(());
    }

    let balancer_target = format!("{}:{}", balancer_ip, balancer_port);
    let mut tasks = vec![];

    if let Some(url) = lm_studio_url {
        let target = balancer_target.clone();
        tasks.push(tokio::spawn(async move {
            udp_broadcast_service("lmstudio", &url, target).await
        }));
    }

    if let Some(url) = ollama_url {
        let target = balancer_target.clone();
        tasks.push(tokio::spawn(async move {
            udp_broadcast_service("ollama", &url, target).await
        }));
    }

    println!("Nodo iniciado. Anunciando servicios cada 10 segundos. Presiona Ctrl+C para detener.");

    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            println!("\nCerrando nodo...");
        }
        Err(err) => {
            eprintln!("Error al escuchar señal de interrupción: {}", err);
        }
    }

    Ok(())
}