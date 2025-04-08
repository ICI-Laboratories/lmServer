// src/balancer.rs

use actix_web::{post, web, App, HttpResponse, HttpServer, Responder};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time::sleep;

#[derive(Clone, Debug)]
pub enum NodeState {
    Available,
    Busy,
    Failed(Instant),
}

pub struct AppState {
    lm_studio_nodes: Arc<RwLock<HashMap<String, NodeState>>>,
    ollama_nodes: Arc<RwLock<HashMap<String, NodeState>>>,
    client: reqwest::Client,
    listen_addr: String,
}

impl AppState {
    fn find_and_occupy_node(
        nodes_lock: &Arc<RwLock<HashMap<String, NodeState>>>,
    ) -> Option<String> {
        let mut nodes = nodes_lock.write().unwrap();
        if let Some((node_url, state)) = nodes
            .iter_mut()
            .find(|(_, s)| matches!(s, NodeState::Available))
        {
            *state = NodeState::Busy;
            Some(node_url.clone())
        } else {
            None
        }
    }

    fn update_node_state(
        nodes_lock: &Arc<RwLock<HashMap<String, NodeState>>>,
        node_url: &str,
        new_state: NodeState,
    ) {
        let mut nodes = nodes_lock.write().unwrap();
        if let Some(state) = nodes.get_mut(node_url) {
            *state = new_state;
        }
    }
}

async fn forward_request(
    client: &reqwest::Client,
    node_url: &str,
    req_body: web::Bytes,
) -> Result<reqwest::Response, reqwest::Error> {
    client.post(node_url).body(req_body).send().await
}

async fn handle_service_request(
    service_name: &str,
    nodes_lock: Arc<RwLock<HashMap<String, NodeState>>>,
    client: web::Data<reqwest::Client>,
    req_body: web::Bytes,
) -> impl Responder {
    let node_url = match AppState::find_and_occupy_node(&nodes_lock) {
         Some(url) => url,
         None => {
             return HttpResponse::ServiceUnavailable()
                 .body(format!("No hay nodos {} disponibles", service_name));
         }
     };

    match forward_request(&client, &node_url, req_body).await {
        Ok(response) => {
            let status = response.status();
            match response.bytes().await {
                Ok(body_bytes) => {
                    if status.is_success() {
                        AppState::update_node_state(&nodes_lock, &node_url, NodeState::Available);
                    } else {
                         eprintln!("Nodo {} respondió con estado no exitoso: {}", node_url, status);
                         AppState::update_node_state(&nodes_lock, &node_url, NodeState::Failed(Instant::now()));
                    }
                    HttpResponse::build(status).body(body_bytes)
                }
                Err(e) => {
                     eprintln!("Error al leer la respuesta del nodo {}: {}", node_url, e);
                     AppState::update_node_state(&nodes_lock, &node_url, NodeState::Failed(Instant::now()));
                     HttpResponse::InternalServerError().body(format!("Error leyendo respuesta de {}", service_name))
                }
            }
        }
        Err(e) => {
             eprintln!("Error al reenviar la solicitud al nodo {}: {}", node_url, e);
             AppState::update_node_state(&nodes_lock, &node_url, NodeState::Failed(Instant::now()));
            HttpResponse::InternalServerError()
                .body(format!("Error reenviando a {}: {}", service_name, e))
        }
    }
}

#[post("/lmstudio")]
async fn lm_studio_handler(
    state: web::Data<AppState>,
    req_body: web::Bytes,
) -> impl Responder {
    let client_ref = web::Data::new(state.client.clone());
    handle_service_request("LM Studio", state.lm_studio_nodes.clone(), client_ref, req_body).await
}

#[post("/ollama")]
async fn ollama_handler(
    state: web::Data<AppState>,
    req_body: web::Bytes,
) -> impl Responder {
     let client_ref = web::Data::new(state.client.clone());
     handle_service_request("Ollama", state.ollama_nodes.clone(), client_ref, req_body).await
}

async fn udp_discovery_listener(
    udp_addr: String,
    app_state: web::Data<AppState>,
) -> std::io::Result<()> {
    let socket = UdpSocket::bind(&udp_addr).await?;
    println!("Escuchando anuncios UDP en {}", udp_addr);
    let mut buf = [0u8; 1024];

    loop {
        match socket.recv_from(&mut buf).await {
             Ok((len, src_addr)) => {
                let msg = String::from_utf8_lossy(&buf[..len]);
                let parts: Vec<&str> = msg.trim().splitn(3, ',').collect();

                if parts.len() == 3 && parts[0] == "DISCOVER" {
                    let service = parts[1];
                    let node_url = parts[2].to_string();

                    println!("Recibido anuncio de {} para {} desde {}", node_url, service, src_addr);

                    let mut nodes_to_update : Option<Arc<RwLock<HashMap<String, NodeState>>>> = None;

                    match service {
                        "lmstudio" => {
                           nodes_to_update = Some(app_state.lm_studio_nodes.clone());
                        }
                        "ollama" => {
                            nodes_to_update = Some(app_state.ollama_nodes.clone());
                        }
                        _ => {
                            eprintln!("Mensaje UDP de descubrimiento con servicio desconocido: {}", msg);
                        }
                    }

                    if let Some(nodes_lock) = nodes_to_update {
                         let mut nodes = nodes_lock.write().unwrap();
                         nodes.insert(node_url, NodeState::Available);
                    }

                } else {
                     eprintln!("Mensaje UDP mal formado recibido de {}: {}", src_addr, msg);
                }
            }
            Err(e) => {
                 eprintln!("Error al recibir mensaje UDP: {}. Reiniciando escucha.", e);
                 sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn terminal_ui(app_state: web::Data<AppState>) {
    let listen_addr = app_state.listen_addr.clone();
    loop {
        print!("\x1B[2J\x1B[1;1H");
        println!("== Estado del Balanceador de Cargas ==");
        println!("API Global escuchando en: http://{}", listen_addr);

        let now = Instant::now();
        let _failure_threshold = Duration::from_secs(60);

        let print_nodes = |service_name: &str, nodes_lock: &Arc<RwLock<HashMap<String, NodeState>>>| {
            let nodes = nodes_lock.read().unwrap();
            println!("\n-- {} Nodes --", service_name);
            println!("{:<60} {:<15}", "Node URL", "State");
            println!("{}", "-".repeat(76));

            if nodes.is_empty() {
                println!("(No nodes registered)");
            } else {
                for (url, state) in nodes.iter() {
                    let state_str = match state {
                        NodeState::Available => "Available".to_string(),
                        NodeState::Busy => "Busy".to_string(),
                        NodeState::Failed(failed_time) => {
                            let elapsed = now.duration_since(*failed_time);
                            format!("Failed ({}s)", elapsed.as_secs())
                        }
                    };
                    println!("{:<60} {:<15}", url, state_str);
                }
            }
        };

        print_nodes("LM Studio", &app_state.lm_studio_nodes);
        print_nodes("Ollama", &app_state.ollama_nodes);

        println!("\nCtrl+C para detener.");

        sleep(Duration::from_secs(5)).await;
    }
}

pub async fn run_balancer(listen_addr: &str, udp_addr: &str) -> std::io::Result<()> {
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("No se pudo crear el cliente HTTP");

    let app_state = web::Data::new(AppState {
        lm_studio_nodes: Arc::new(RwLock::new(HashMap::new())),
        ollama_nodes: Arc::new(RwLock::new(HashMap::new())),
        client: http_client,
        listen_addr: listen_addr.to_string(),
    });

    let udp_listener_state = app_state.clone();
    let udp_addr_owned = udp_addr.to_string();
    tokio::spawn(async move {
        if let Err(e) = udp_discovery_listener(udp_addr_owned, udp_listener_state).await {
            eprintln!("Error crítico en el listener UDP: {}. El descubrimiento de nodos se ha detenido.", e);
        }
    });

    let ui_state = app_state.clone();
    tokio::spawn(async move {
        terminal_ui(ui_state).await;
    });

    println!("Iniciando servidor HTTP del balanceador en {}", listen_addr);
    println!("UI en Terminal activa. Presiona Ctrl+C para detener.");
    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(lm_studio_handler)
            .service(ollama_handler)
    })
    .bind(listen_addr)?
    .run()
    .await
}