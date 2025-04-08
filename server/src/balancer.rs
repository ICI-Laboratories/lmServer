// src/balancer.rs
use actix_web::{post, web, App, HttpResponse, HttpServer, Responder};
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time::sleep;
use log::{info, warn, error, debug, trace};
use url::Url;

#[derive(Clone, Debug)]
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
        debug!(" -> Entrando a find_and_occupy_node...");
        let mut nodes = nodes_lock.write().unwrap();

        let found_node = nodes
            .iter_mut()
            .find(|(_id, info)| {
                 trace!("    -> Verificando nodo ID: {} (URL: {}) - Estado: {:?}", _id, info.service_url, info.state);
                 matches!(info.state, NodeHealth::Available)
            });

        if let Some((unique_id, node_info)) = found_node {
            debug!("    -> Nodo disponible encontrado ID: {}. Marcando como Busy.", unique_id);
            node_info.state = NodeHealth::Busy;
            Some((unique_id.clone(), node_info.service_url.clone()))
        } else {
            debug!("    -> No se encontró ningún nodo disponible.");
            None
        }
    }

    fn update_node_state(
        nodes_lock: &Arc<RwLock<HashMap<String, NodeInfo>>>,
        unique_node_id: &str,
        new_health: NodeHealth,
    ) {
        let mut nodes = nodes_lock.write().unwrap();
        if let Some(node_info) = nodes.get_mut(unique_node_id) {
             debug!("  -> Actualizando estado del nodo ID {} (URL: {}) a: {:?}", unique_node_id, node_info.service_url, new_health);
            node_info.state = new_health;
        } else {
             warn!("  -> Intento de actualizar estado de nodo ID {} fallido (nodo no encontrado).", unique_node_id);
        }
    }
}

async fn forward_request(
    client: &reqwest::Client,
    node_service_url: &str,
    req_body: web::Bytes,
) -> Result<reqwest::Response, reqwest::Error> {
     debug!("  -> forward_request: Enviando POST a {} con body size: {}", node_service_url, req_body.len());
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
    info!("Balancer handle_service_request para '{}' RECIBIDO.", service_name);
    debug!("  -> Tamaño del body recibido: {} bytes", req_body.len());

    if req_body.len() > 0 && req_body.len() < 1024 {
         match std::str::from_utf8(&req_body) {
             Ok(body_str) => trace!("  -> Contenido del body recibido: {}", body_str),
             Err(_) => trace!("  -> Contenido del body recibido: (No es UTF-8 válido o muy largo)"),
         }
    } else if req_body.len() == 0 {
         debug!("  -> Contenido del body recibido: ¡¡¡VACÍO!!!");
    }

    let start_time = Instant::now();

    let (unique_node_id, node_service_url) = loop {
        if let Some(found) = AppState::find_and_occupy_node(&nodes_lock) {
            debug!("  -> Nodo encontrado y ocupado: ID {}, URL {}", found.0, found.1);
            break found;
        }

        if start_time.elapsed() > queue_timeout {
            error!("  -> ERROR: No se encontraron nodos disponibles para '{}' dentro del tiempo de espera ({}s).", service_name, queue_timeout.as_secs());
            return HttpResponse::ServiceUnavailable().body(format!(
                "No hay nodos {} disponibles (timeout {}s)",
                service_name,
                queue_timeout.as_secs()
            ));
        }

        trace!("  -> No hay nodos {} disponibles. Esperando {}ms...", service_name, queue_poll_interval.as_millis());
        sleep(queue_poll_interval).await;
    };

    info!("  -> Intentando reenviar petición a ID: {}, URL: {}", unique_node_id, node_service_url);

    match forward_request(&client, &node_service_url, req_body).await {
        Ok(response) => {
            let status = response.status();
            info!("  -> Respuesta recibida del nodo ID {} (URL {}) con estado: {}", unique_node_id, node_service_url, status);
            match response.bytes().await {
                Ok(body_bytes) => {
                    let new_health = if status.is_success() {
                        NodeHealth::Available
                    } else {
                         warn!("  -> Nodo ID {} respondió con estado no exitoso: {}", unique_node_id, status);
                         NodeHealth::Failed(Instant::now())
                    };
                    AppState::update_node_state(&nodes_lock, &unique_node_id, new_health.clone());
                    debug!("  -> Marcando nodo ID {} como {:?}.", unique_node_id, new_health);
                    HttpResponse::build(status).body(body_bytes)
                }
                Err(e) => {
                     error!("  -> Error al leer la respuesta del nodo ID {}: {}", unique_node_id, e);
                     AppState::update_node_state(&nodes_lock, &unique_node_id, NodeHealth::Failed(Instant::now()));
                     debug!("  -> Marcando nodo ID {} como Failed.", unique_node_id);
                     HttpResponse::InternalServerError().body(format!("Error leyendo respuesta de {}", service_name))
                }
            }
        }
        Err(e) => {
             error!("  -> Error al reenviar la solicitud al nodo ID {}: {}", unique_node_id, e);
             AppState::update_node_state(&nodes_lock, &unique_node_id, NodeHealth::Failed(Instant::now()));
             debug!("  -> Marcando nodo ID {} como Failed.", unique_node_id);
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
     info!("Balancer /lmstudio handler RECIBIDO request. Body size: {}", req_body.len());
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
     info!("Balancer /ollama handler RECIBIDO request. Body size: {}", req_body.len());
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
    info!("Escuchando anuncios UDP en {}", udp_addr);
    let mut buf = [0u8; 1024];

    loop {
        match socket.recv_from(&mut buf).await {
             Ok((len, src_addr)) => {
                let msg = String::from_utf8_lossy(&buf[..len]);
                let parts: Vec<&str> = msg.trim().splitn(4, ',').collect();

                if parts.len() == 4 && parts[0] == "DISCOVER" {
                    let service_type = parts[1];
                    let unique_node_id = parts[2].to_string();
                    let announced_service_url = parts[3].to_string();

                    let mut effective_service_url = announced_service_url.clone();
                    match Url::parse(&announced_service_url) {
                        Ok(mut parsed_url) => {
                            if let Some(host_str) = parsed_url.host_str() {
                                if host_str == "localhost" || host_str == "127.0.0.1" {
                                    let source_ip = src_addr.ip().to_string();
                                    if let Err(e) = parsed_url.set_host(Some(&source_ip)) {
                                        warn!("UDP Listener: No se pudo establecer el host '{}' en la URL parseada para {}: {}", source_ip, unique_node_id, e);
                                    } else {
                                        effective_service_url = parsed_url.to_string();
                                        debug!("UDP Listener: Reemplazado host 'localhost'/'127.0.0.1' con '{}' para nodo {}", source_ip, unique_node_id);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("UDP Listener: No se pudo parsear la URL '{}' anunciada por {}: {}. Usando URL original.", announced_service_url, unique_node_id, e);
                        }
                    }

                    info!("UDP Listener: Recibido anuncio de ID {} (URL efectiva {}) para {} desde {}",
                          unique_node_id, effective_service_url, service_type, src_addr);


                    let nodes_lock = match service_type {
                        "lmstudio" => Some(app_state.lm_studio_nodes.clone()),
                        "ollama" => Some(app_state.ollama_nodes.clone()),
                        _ => {
                            warn!("UDP Listener: Mensaje UDP de descubrimiento con servicio desconocido: {}", msg);
                            None
                        }
                    };

                    if let Some(lock) = nodes_lock {
                         let mut nodes = lock.write().unwrap();
                         debug!("UDP Listener: Añadiendo/Actualizando nodo ID {} para servicio {} como Available.", unique_node_id, service_type);
                         nodes.insert(unique_node_id.clone(), NodeInfo {
                             state: NodeHealth::Available,
                             service_url: effective_service_url,
                             last_seen: Instant::now(),
                         });
                    }

                } else {
                     warn!("UDP Listener: Mensaje UDP mal formado recibido de {} (Esperado 'DISCOVER,<svc>,<id>,<url>'): {}", src_addr, msg);
                }
            }
            Err(e) => {
                 error!("UDP Listener: Error al recibir mensaje UDP: {}. Reiniciando escucha.", e);
                 sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn terminal_ui(app_state: web::Data<AppState>) {
    let listen_addr = app_state.listen_addr.clone();

    loop {
        print!("\x1B[2J\x1B[1;1H");
        let _ = io::stdout().flush();


        info!("== Estado del Balanceador de Cargas ==");
        info!("API Global escuchando en: http://{}", listen_addr);
        info!("Timeout cola peticiones: {}s", app_state.queue_timeout.as_secs());

        let now = Instant::now();
        let _failure_threshold = Duration::from_secs(60);

        let print_nodes = |service_name: &str, nodes_lock: &Arc<RwLock<HashMap<String, NodeInfo>>>| {
            let nodes = nodes_lock.read().unwrap();
            info!("\n-- {} Nodes --", service_name);
            info!("{:<45} {:<60} {:<15} {:<10}", "Node ID", "Service URL", "State", "Last Seen");
            info!("{}", "-".repeat(135));

            if nodes.is_empty() {
                info!("(No nodes registered)");
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
                    info!("{:<45} {:<60} {:<15} {:<10}", id, info.service_url, state_str, format!("{}s ago", seen_ago));
                }
            }
        };

        print_nodes("LM Studio", &app_state.lm_studio_nodes);
        print_nodes("Ollama", &app_state.ollama_nodes);

        info!("\nCtrl+C para detener.");

        sleep(Duration::from_secs(2)).await;
    }
}

fn remove_stale_nodes(
    nodes_map: &mut HashMap<String, NodeInfo>,
    timeout: Duration,
    service_name: &str,
) {
    let now = Instant::now();
    let initial_len = nodes_map.len();
    let mut removed_nodes = Vec::new();

    nodes_map.retain(|node_id, node_info| {
        let is_stale = now.duration_since(node_info.last_seen) > timeout;
        if is_stale {
            removed_nodes.push(node_id.clone());
            false
        } else {
            true
        }
    });

    let removed_count = initial_len - nodes_map.len();
    if removed_count > 0 {
        info!(
            "Cleanup Task: Removed {} stale {} node(s): {:?}",
            removed_count, service_name, removed_nodes
        );
    } else {
         trace!("Cleanup Task: No stale {} nodes found to remove.", service_name);
    }
}


pub async fn run_balancer(listen_addr: &str, udp_addr: &str) -> std::io::Result<()> {
    info!("Configurando cliente HTTP...");
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("No se pudo crear el cliente HTTP");
    info!("Cliente HTTP configurado.");

    let queue_timeout = Duration::from_secs(30);
    let queue_poll_interval = Duration::from_millis(200);


    info!("Creando estado de la aplicación...");
    let app_state = web::Data::new(AppState {
        lm_studio_nodes: Arc::new(RwLock::new(HashMap::new())),
        ollama_nodes: Arc::new(RwLock::new(HashMap::new())),
        client: http_client,
        listen_addr: listen_addr.to_string(),
        queue_timeout,
        queue_poll_interval,
    });
    info!("Estado de la aplicación creado.");

    info!("Iniciando listener UDP...");
    let udp_listener_state = app_state.clone();
    let udp_addr_owned = udp_addr.to_string();
    tokio::spawn(async move {
        if let Err(e) = udp_discovery_listener(udp_addr_owned, udp_listener_state).await {
            error!("CRITICAL: Error en el listener UDP: {}. El descubrimiento de nodos se ha detenido.", e);
        }
    });
    info!("Listener UDP iniciado en segundo plano.");

    info!("Iniciando UI de terminal...");
    let ui_state = app_state.clone();
    tokio::spawn(async move {
        terminal_ui(ui_state).await;
    });
    info!("UI de terminal iniciada en segundo plano.");

    info!("Iniciando tarea de limpieza de nodos inactivos...");
    let cleanup_state = app_state.clone();
    let node_inactivity_timeout = Duration::from_secs(35);
    let cleanup_interval = Duration::from_secs(30);

    tokio::spawn(async move {
        info!("Tarea de limpieza iniciada. Intervalo: {:?}, Timeout inactividad: {:?}",
               cleanup_interval, node_inactivity_timeout);
        loop {
            sleep(cleanup_interval).await;
            debug!("Cleanup Task: Ejecutando limpieza de nodos inactivos...");

            match cleanup_state.lm_studio_nodes.write() {
                 Ok(mut nodes_guard) => {
                    remove_stale_nodes(&mut nodes_guard, node_inactivity_timeout, "LM Studio");
                 }
                 Err(e) => {
                    error!("Cleanup Task: Error al obtener write lock para LM Studio nodes: {}", e);
                 }
            }


             match cleanup_state.ollama_nodes.write() {
                 Ok(mut nodes_guard) => {
                     remove_stale_nodes(&mut nodes_guard, node_inactivity_timeout, "Ollama");
                 }
                 Err(e) => {
                    error!("Cleanup Task: Error al obtener write lock para Ollama nodes: {}", e);
                 }
            }

            debug!("Cleanup Task: Limpieza completada.");
        }
    });


    info!("Iniciando servidor HTTP del balanceador en {}", listen_addr);
    info!("UI en Terminal activa. Presiona Ctrl+C para detener.");
    HttpServer::new(move || {
        trace!("Configurando nueva instancia de Actix App...");
        App::new()
            .app_data(app_state.clone())
            .service(lm_studio_handler)
            .service(ollama_handler)
    })
    .bind(listen_addr)?
    .run()
    .await
}