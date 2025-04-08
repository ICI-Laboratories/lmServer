use actix_web::{post, web, App, HttpResponse, HttpServer, Responder};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time::sleep;

#[derive(Clone, Debug)] // NodeHealth YA implementa Clone, ¡perfecto!
pub enum NodeHealth {
    Available,
    Busy,
    Failed(Instant),
}

#[derive(Clone, Debug)]
pub struct NodeInfo {
    state: NodeHealth,
    service_url: String,
    last_seen: Instant,
}

pub struct AppState {
    lm_studio_nodes: Arc<RwLock<HashMap<String, NodeInfo>>>,
    ollama_nodes: Arc<RwLock<HashMap<String, NodeInfo>>>,
    client: reqwest::Client,
    listen_addr: String,
    queue_timeout: Duration,
    queue_poll_interval: Duration,
}

impl AppState {
    fn find_and_occupy_node(
        nodes_lock: &Arc<RwLock<HashMap<String, NodeInfo>>>,
    ) -> Option<(String, String)> {
        println!(" -> Entrando a find_and_occupy_node...");
        let mut nodes = nodes_lock.write().unwrap();

        let found_node = nodes
            .iter_mut()
            .find(|(_id, info)| {
                 println!("    -> Verificando nodo ID: {} (URL: {}) - Estado: {:?}", _id, info.service_url, info.state);
                 matches!(info.state, NodeHealth::Available)
            });

        if let Some((unique_id, node_info)) = found_node {
            println!("    -> Nodo disponible encontrado ID: {}. Marcando como Busy.", unique_id);
            node_info.state = NodeHealth::Busy;
            Some((unique_id.clone(), node_info.service_url.clone()))
        } else {
            println!("    -> No se encontró ningún nodo disponible.");
            None
        }
    }

    // La firma no necesita cambiar porque NodeHealth implementa Clone
    fn update_node_state(
        nodes_lock: &Arc<RwLock<HashMap<String, NodeInfo>>>,
        unique_node_id: &str,
        new_health: NodeHealth, // Recibe la propiedad
    ) {
        let mut nodes = nodes_lock.write().unwrap();
        if let Some(node_info) = nodes.get_mut(unique_node_id) {
             println!("  -> Actualizando estado del nodo ID {} (URL: {}) a: {:?}", unique_node_id, node_info.service_url, new_health);
            node_info.state = new_health; // Asigna el valor movido
        } else {
             println!("  -> Intento de actualizar estado de nodo ID {} fallido (nodo no encontrado).", unique_node_id);
        }
    }
}

async fn forward_request(
    client: &reqwest::Client,
    node_service_url: &str,
    req_body: web::Bytes,
) -> Result<reqwest::Response, reqwest::Error> {
     println!("  -> forward_request: Enviando POST a {} con body size: {}", node_service_url, req_body.len());
     client.post(node_service_url)
         .header(reqwest::header::CONTENT_TYPE, "application/json")
         .body(req_body)
         .send()
         .await
}

async fn handle_service_request(
    service_name: &str,
    nodes_lock: Arc<RwLock<HashMap<String, NodeInfo>>>,
    client: web::Data<reqwest::Client>,
    req_body: web::Bytes,
    queue_timeout: Duration,
    queue_poll_interval: Duration,
) -> impl Responder {
    println!("Balancer handle_service_request para '{}' RECIBIDO.", service_name);
    println!("  -> Tamaño del body recibido: {} bytes", req_body.len());

    if req_body.len() > 0 && req_body.len() < 1024 {
         match std::str::from_utf8(&req_body) {
             Ok(body_str) => println!("  -> Contenido del body recibido: {}", body_str),
             Err(_) => println!("  -> Contenido del body recibido: (No es UTF-8 válido o muy largo)"),
         }
    } else if req_body.len() == 0 {
         println!("  -> Contenido del body recibido: ¡¡¡VACÍO!!!");
    }


    let start_time = Instant::now();

    let (unique_node_id, node_service_url) = loop {
        if let Some(found) = AppState::find_and_occupy_node(&nodes_lock) {
            println!("  -> Nodo encontrado y ocupado: ID {}, URL {}", found.0, found.1);
            break found;
        }

        if start_time.elapsed() > queue_timeout {
            println!("  -> ERROR: No se encontraron nodos disponibles para '{}' dentro del tiempo de espera ({}s).", service_name, queue_timeout.as_secs());
            return HttpResponse::ServiceUnavailable().body(format!(
                "No hay nodos {} disponibles (timeout {}s)",
                service_name,
                queue_timeout.as_secs()
            ));
        }

        println!("  -> No hay nodos {} disponibles. Esperando {}ms...", service_name, queue_poll_interval.as_millis());
        sleep(queue_poll_interval).await;
    };

    println!("  -> Intentando reenviar petición a ID: {}, URL: {}", unique_node_id, node_service_url);

    match forward_request(&client, &node_service_url, req_body).await {
        Ok(response) => {
            let status = response.status();
            println!("  -> Respuesta recibida del nodo ID {} (URL {}) con estado: {}", unique_node_id, node_service_url, status);
            match response.bytes().await {
                Ok(body_bytes) => {
                    let new_health = if status.is_success() {
                        NodeHealth::Available
                    } else {
                         eprintln!("  -> Nodo ID {} respondió con estado no exitoso: {}", unique_node_id, status);
                         NodeHealth::Failed(Instant::now())
                    };
                    // --- LA CORRECCIÓN ESTÁ AQUÍ ---
                    AppState::update_node_state(&nodes_lock, &unique_node_id, new_health.clone()); // Clonar antes de mover
                    // --- FIN DE LA CORRECCIÓN ---
                    println!("  -> Marcando nodo ID {} como {:?}.", unique_node_id, new_health); // Ahora 'new_health' sigue siendo válida aquí
                    HttpResponse::build(status).body(body_bytes)
                }
                Err(e) => {
                     eprintln!("  -> Error al leer la respuesta del nodo ID {}: {}", unique_node_id, e);
                     AppState::update_node_state(&nodes_lock, &unique_node_id, NodeHealth::Failed(Instant::now()));
                     println!("  -> Marcando nodo ID {} como Failed.", unique_node_id);
                     HttpResponse::InternalServerError().body(format!("Error leyendo respuesta de {}", service_name))
                }
            }
        }
        Err(e) => {
             eprintln!("  -> Error al reenviar la solicitud al nodo ID {}: {}", unique_node_id, e);
             AppState::update_node_state(&nodes_lock, &unique_node_id, NodeHealth::Failed(Instant::now()));
             println!("  -> Marcando nodo ID {} como Failed.", unique_node_id);
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
     println!("Balancer /lmstudio handler RECIBIDO request. Body size: {}", req_body.len());
     let client_ref = web::Data::new(state.client.clone());
     handle_service_request(
         "LM Studio",
         state.lm_studio_nodes.clone(),
         client_ref,
         req_body,
         state.queue_timeout,
         state.queue_poll_interval
     ).await
}

#[post("/ollama")]
async fn ollama_handler(
    state: web::Data<AppState>,
    req_body: web::Bytes,
) -> impl Responder {
     println!("Balancer /ollama handler RECIBIDO request. Body size: {}", req_body.len());
     let client_ref = web::Data::new(state.client.clone());
     handle_service_request(
        "Ollama",
        state.ollama_nodes.clone(),
        client_ref,
        req_body,
        state.queue_timeout,
        state.queue_poll_interval
    ).await
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
                let parts: Vec<&str> = msg.trim().splitn(4, ',').collect();

                if parts.len() == 4 && parts[0] == "DISCOVER" {
                    let service_type = parts[1];
                    let unique_node_id = parts[2].to_string();
                    let service_url = parts[3].to_string();

                    println!("UDP Listener: Recibido anuncio de ID {} (URL {}) para {} desde {}", unique_node_id, service_url, service_type, src_addr);

                    let nodes_lock = match service_type {
                        "lmstudio" => Some(app_state.lm_studio_nodes.clone()),
                        "ollama" => Some(app_state.ollama_nodes.clone()),
                        _ => {
                            eprintln!("UDP Listener: Mensaje UDP de descubrimiento con servicio desconocido: {}", msg);
                            None
                        }
                    };

                    if let Some(lock) = nodes_lock {
                         let mut nodes = lock.write().unwrap();
                         println!("UDP Listener: Añadiendo/Actualizando nodo ID {} para servicio {} como Available.", unique_node_id, service_type);
                         nodes.insert(unique_node_id, NodeInfo {
                             state: NodeHealth::Available,
                             service_url: service_url,
                             last_seen: Instant::now(),
                         });
                    }

                } else {
                     eprintln!("UDP Listener: Mensaje UDP mal formado recibido de {} (Esperado 'DISCOVER,<svc>,<id>,<url>'): {}", src_addr, msg);
                }
            }
            Err(e) => {
                 eprintln!("UDP Listener: Error al recibir mensaje UDP: {}. Reiniciando escucha.", e);
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
        println!("Timeout cola peticiones: {}s", app_state.queue_timeout.as_secs());

        let now = Instant::now();
        let _failure_threshold = Duration::from_secs(60);

        let print_nodes = |service_name: &str, nodes_lock: &Arc<RwLock<HashMap<String, NodeInfo>>>| {
            let nodes = nodes_lock.read().unwrap();
            println!("\n-- {} Nodes --", service_name);
            println!("{:<45} {:<60} {:<15} {:<10}", "Node ID", "Service URL", "State", "Last Seen");
            println!("{}", "-".repeat(135));

            if nodes.is_empty() {
                println!("(No nodes registered)");
            } else {
                let mut sorted_nodes: Vec<_> = nodes.iter().collect();
                sorted_nodes.sort_by_key(|(id, _)| *id);

                for (id, info) in sorted_nodes {
                    let state_str = match info.state {
                        NodeHealth::Available => "Available".to_string(),
                        NodeHealth::Busy => "Busy".to_string(),
                        NodeHealth::Failed(failed_time) => {
                            let elapsed = now.duration_since(failed_time);
                             format!("Failed ({}s)", elapsed.as_secs())
                        }
                    };
                    let seen_ago = now.duration_since(info.last_seen).as_secs();
                    println!("{:<45} {:<60} {:<15} {:<10}", id, info.service_url, state_str, format!("{}s ago", seen_ago));
                }
            }
        };

        print_nodes("LM Studio", &app_state.lm_studio_nodes);
        print_nodes("Ollama", &app_state.ollama_nodes);

        println!("\nCtrl+C para detener.");

        sleep(Duration::from_secs(2)).await;
    }
}

pub async fn run_balancer(listen_addr: &str, udp_addr: &str) -> std::io::Result<()> {
    println!("Configurando cliente HTTP...");
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("No se pudo crear el cliente HTTP");
    println!("Cliente HTTP configurado.");

    let queue_timeout = Duration::from_secs(30);
    let queue_poll_interval = Duration::from_millis(200);


    println!("Creando estado de la aplicación...");
    let app_state = web::Data::new(AppState {
        lm_studio_nodes: Arc::new(RwLock::new(HashMap::new())),
        ollama_nodes: Arc::new(RwLock::new(HashMap::new())),
        client: http_client,
        listen_addr: listen_addr.to_string(),
        queue_timeout,
        queue_poll_interval,
    });
    println!("Estado de la aplicación creado.");

    println!("Iniciando listener UDP...");
    let udp_listener_state = app_state.clone();
    let udp_addr_owned = udp_addr.to_string();
    tokio::spawn(async move {
        if let Err(e) = udp_discovery_listener(udp_addr_owned, udp_listener_state).await {
            eprintln!("CRITICAL: Error en el listener UDP: {}. El descubrimiento de nodos se ha detenido.", e);
        }
    });
    println!("Listener UDP iniciado en segundo plano.");

    println!("Iniciando UI de terminal...");
    let ui_state = app_state.clone();
    tokio::spawn(async move {
        terminal_ui(ui_state).await;
    });
    println!("UI de terminal iniciada en segundo plano.");

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