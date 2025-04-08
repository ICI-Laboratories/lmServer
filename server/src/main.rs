use actix_web::{post, web, App, HttpResponse, HttpServer, Responder};
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time::{sleep, interval};

/// Estados posibles para cada nodo.
#[derive(Clone, Debug)]
enum NodeState {
    Available,
    Busy,
    Failed(#[allow(unused)] Instant),
}

/// Estado compartido en el balanceador para cada tipo de servicio.
struct AppState {
    lm_studio_nodes: Arc<RwLock<HashMap<String, NodeState>>>,
    ollama_nodes: Arc<RwLock<HashMap<String, NodeState>>>,
}

/// Función que reenvía la solicitud a un nodo concreto, construyendo la URL a partir
/// de la dirección y el endpoint.
async fn forward_request(node_address: &str, endpoint: &str, req_body: String) -> Result<String, reqwest::Error> {
    let url = format!("http://{}{}", node_address, endpoint);
    let client = reqwest::Client::new();
    let res = client.post(&url).body(req_body).send().await?;
    let text = res.text().await?;
    Ok(text)
}

/// ─── ENDPOINTS DEL BALANCEADOR ─────────────────────────────────────────────

/// Handler para solicitudes de LM Studio. Busca un nodo disponible en el registro,
/// lo marca como ocupado, reenvía la solicitud a la ruta `/v1/chat/completions` y,
/// según el resultado, lo vuelve a marcar como disponible o en fallo.
#[post("/lmstudio")]
async fn lm_studio_handler(state: web::Data<AppState>, req_body: String) -> impl Responder {
    let mut nodes = state.lm_studio_nodes.write().unwrap();
    if let Some((node_address, node_state)) = nodes.iter_mut().find(|(_, s)| matches!(s, NodeState::Available)) {
        *node_state = NodeState::Busy;
        let node_address_cloned = node_address.clone();
        drop(nodes);
        match forward_request(&node_address_cloned, "/v1/chat/completions", req_body).await {
            Ok(response_body) => {
                let mut nodes = state.lm_studio_nodes.write().unwrap();
                if let Some(s) = nodes.get_mut(&node_address_cloned) {
                    *s = NodeState::Available;
                }
                HttpResponse::Ok().body(response_body)
            }
            Err(e) => {
                let mut nodes = state.lm_studio_nodes.write().unwrap();
                if let Some(s) = nodes.get_mut(&node_address_cloned) {
                    *s = NodeState::Failed(Instant::now());
                }
                HttpResponse::InternalServerError()
                    .body(format!("Error forwarding request to LM Studio: {}", e))
            }
        }
    } else {
        HttpResponse::ServiceUnavailable().body("No hay nodos LM Studio disponibles")
    }
}

/// Handler para solicitudes de Ollama. Busca un nodo disponible en el registro,
/// lo marca como ocupado, reenvía la solicitud a la ruta `/chat/completions` y,
/// según el resultado, lo vuelve a marcar como disponible o en fallo.
#[post("/ollama")]
async fn ollama_handler(state: web::Data<AppState>, req_body: String) -> impl Responder {
    let mut nodes = state.ollama_nodes.write().unwrap();
    if let Some((node_address, node_state)) = nodes.iter_mut().find(|(_, s)| matches!(s, NodeState::Available)) {
        *node_state = NodeState::Busy;
        let node_address_cloned = node_address.clone();
        drop(nodes);
        match forward_request(&node_address_cloned, "/chat/completions", req_body).await {
            Ok(response_body) => {
                let mut nodes = state.ollama_nodes.write().unwrap();
                if let Some(s) = nodes.get_mut(&node_address_cloned) {
                    *s = NodeState::Available;
                }
                HttpResponse::Ok().body(response_body)
            }
            Err(e) => {
                let mut nodes = state.ollama_nodes.write().unwrap();
                if let Some(s) = nodes.get_mut(&node_address_cloned) {
                    *s = NodeState::Failed(Instant::now());
                }
                HttpResponse::InternalServerError().body(format!("Error forwarding request to Ollama: {}", e))
            }
        }
    } else {
        HttpResponse::ServiceUnavailable().body("No hay nodos Ollama disponibles")
    }
}

/// ─── DISCOVERY POR UDP (BALANCEADOR) ─────────────────────────────────────────

/// Escucha mensajes UDP en el puerto 4000 para descubrir nodos.
/// Se espera recibir mensajes en el formato: "DISCOVER,<service>,<node_address>"
async fn udp_discovery_listener(app_state: web::Data<AppState>) -> std::io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:4000").await?;
    let mut buf = [0u8; 1024];
    loop {
        let (len, _addr) = socket.recv_from(&mut buf).await?;
        let msg = String::from_utf8_lossy(&buf[..len]).to_string();
        let parts: Vec<&str> = msg.trim().split(',').collect();
        if parts.len() == 3 && parts[0] == "DISCOVER" {
            let service = parts[1];
            let node_address = parts[2].to_string();
            match service {
                "lmstudio" => {
                    let mut nodes = app_state.lm_studio_nodes.write().unwrap();
                    nodes.insert(node_address, NodeState::Available);
                }
                "ollama" => {
                    let mut nodes = app_state.ollama_nodes.write().unwrap();
                    nodes.insert(node_address, NodeState::Available);
                }
                _ => {
                    println!("Mensaje con servicio desconocido: {}", msg);
                }
            }
        }
    }
}

/// ─── VISUALIZACIÓN EN TERMINAL (BALANCEADOR) ───────────────────────────────────

/// Tarea que cada 5 segundos limpia la terminal y muestra el estado actual de los nodos
/// para LM Studio y Ollama. Se considera que el balanceador es el nodo 0.
async fn terminal_ui(app_state: web::Data<AppState>) {
    loop {
        // Limpiar la terminal usando ANSI escape codes.
        print!("\x1B[2J\x1B[1;1H");
        println!("== Mapa de Nodos ==");

        {
            let nodes = app_state.lm_studio_nodes.read().unwrap();
            println!("LM Studio Nodes:");
            println!("{:<5} {:<25} {:<10}", "Index", "Node Address", "State");
            let mut index = 0;
            for (addr, state) in nodes.iter() {
                let state_str = match state {
                    NodeState::Available => "Available",
                    NodeState::Busy => "Busy",
                    NodeState::Failed(_) => "Failed",
                };
                println!("{:<5} {:<25} {:<10}", index, addr, state_str);
                index += 1;
            }
        }
        println!("----------------------------");
        {
            let nodes = app_state.ollama_nodes.read().unwrap();
            println!("Ollama Nodes:");
            println!("{:<5} {:<25} {:<10}", "Index", "Node Address", "State");
            let mut index = 0;
            for (addr, state) in nodes.iter() {
                let state_str = match state {
                    NodeState::Available => "Available",
                    NodeState::Busy => "Busy",
                    NodeState::Failed(_) => "Failed",
                };
                println!("{:<5} {:<25} {:<10}", index, addr, state_str);
                index += 1;
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}

/// ─── BROADCAST UDP (NODO) ─────────────────────────────────────────────────────
 
/// En modo nodo, se envía cada 10 segundos un mensaje UDP al balanceador anunciando su presencia.
/// El mensaje tiene el formato: "DISCOVER,<service>,<node_address>"
async fn udp_broadcast(service: &str, node_address: &str, balancer_ip: &str) -> std::io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let target_addr = format!("{}:4000", balancer_ip);
    let target: std::net::SocketAddr = target_addr.parse().unwrap();
    let msg = format!("DISCOVER,{},{}", service, node_address);
    let mut ticker = interval(Duration::from_secs(10));
    loop {
        ticker.tick().await;
        socket.send_to(msg.as_bytes(), &target).await?;
    }
}

/// ─── SERVIDOR HTTP DEL NODO ───────────────────────────────────────────────────

/// Dependiendo del servicio, se levanta un servidor HTTP que responde en la ruta correspondiente.
/// Para LM Studio se usa la ruta `/v1/chat/completions` y para Ollama, `/chat/completions`.
async fn run_node_http(service: String, node_address: String) -> std::io::Result<()> {
    // Encapsulamos el uso de node_address.split(':') en un bloque para que las referencias caduquen.
    let port = {
        let parts: Vec<&str> = node_address.split(':').collect();
        if parts.len() != 2 {
            panic!("El formato de node_address debe ser IP:PORT");
        }
        parts[1].to_string()
    };

    // Usamos Arc para compartir la dirección dentro de las closures sin moverla.
    let node_addr = Arc::new(node_address);

    match service.as_str() {
        "lmstudio" => {
            HttpServer::new(move || {
                let node_addr = node_addr.clone();
                App::new().service(
                    web::resource("/v1/chat/completions").route(web::post().to({
                        let addr = node_addr.clone();
                        move |_body: String| {
                            let addr = addr.clone();
                            async move {
                                HttpResponse::Ok()
                                    .body(format!("Respuesta de LM Studio node en {}", addr))
                            }
                        }
                    }))
                )
            })
            .bind(format!("0.0.0.0:{}", port))?
            .run()
            .await
        }
        "ollama" => {
            HttpServer::new(move || {
                let node_addr = node_addr.clone();
                App::new().service(
                    web::resource("/chat/completions").route(web::post().to({
                        let addr = node_addr.clone();
                        move |_body: String| {
                            let addr = addr.clone();
                            async move {
                                HttpResponse::Ok()
                                    .body(format!("Respuesta de Ollama node en {}", addr))
                            }
                        }
                    }))
                )
            })
            .bind(format!("0.0.0.0:{}", port))?
            .run()
            .await
        }
        _ => {
            panic!("Servicio desconocido: {}", service);
        }
    }
}

/// ─── FUNCIÓN PARA EL ROL DE BALANCEADOR ───────────────────────────────────────

async fn run_balancer() -> std::io::Result<()> {
    // Se crean los registros para cada servicio.
    // Se inserta el balanceador como nodo 0 (por ejemplo, "127.0.0.1:8080").
    let mut lm_studio_map = HashMap::new();
    let mut ollama_map = HashMap::new();
    lm_studio_map.insert("127.0.0.1:8080".to_string(), NodeState::Available);
    ollama_map.insert("127.0.0.1:8080".to_string(), NodeState::Available);

    let app_state = web::Data::new(AppState {
        lm_studio_nodes: Arc::new(RwLock::new(lm_studio_map)),
        ollama_nodes: Arc::new(RwLock::new(ollama_map)),
    });

    // Se lanza la tarea que escucha anuncios UDP.
    let state_clone = app_state.clone();
    tokio::spawn(async move {
        if let Err(e) = udp_discovery_listener(state_clone).await {
            eprintln!("Error en listener UDP: {}", e);
        }
    });

    // Se lanza la tarea que imprime el mapa de nodos en terminal.
    let state_clone = app_state.clone();
    tokio::spawn(async move {
        terminal_ui(state_clone).await;
    });

    println!("Load Balancer iniciado en 0.0.0.0:8080");
    // Se inicia el servidor HTTP con los endpoints para reenviar solicitudes.
    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(lm_studio_handler)
            .service(ollama_handler)
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}

/// ─── FUNCIÓN PARA EL ROL DE NODO ─────────────────────────────────────────────

/// Se espera que los argumentos para el nodo sean:  
/// `cargo run -- node <lmstudio|ollama> <node_address> <balancer_ip>`
async fn run_node(args: Vec<String>) -> std::io::Result<()> {
    if args.len() < 5 {
        println!("Uso: {} node <lmstudio|ollama> <node_address> <balancer_ip>", args[0]);
        return Ok(());
    }
    let service = args[2].clone();
    let node_address = args[3].clone();
    let balancer_ip = args[4].clone();

    // Se lanza el broadcaster UDP para anunciar la presencia del nodo.
    {
        let service_clone = service.clone();
        let node_address_clone = node_address.clone();
        let balancer_ip_clone = balancer_ip.clone();
        tokio::spawn(async move {
            if let Err(e) = udp_broadcast(&service_clone, &node_address_clone, &balancer_ip_clone).await {
                eprintln!("Error en broadcaster UDP: {}", e);
            }
        });
    }

    run_node_http(service, node_address).await
}

/// ─── FUNCIÓN MAIN: Selección de rol vía argumentos ──────────────────────────

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println!("Uso: {} [balancer|node] ...", args[0]);
        return Ok(());
    }
    match args[1].as_str() {
        "balancer" => run_balancer().await?,
        "node" => run_node(args).await?,
        _ => {
            println!("Rol inválido. Usa 'balancer' o 'node'.");
        }
    }
    Ok(())
}
