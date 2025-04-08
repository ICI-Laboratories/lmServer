# ⚙️ Sistema de Balanceo de Carga y Registro de Nodos vía UDP/HTTP

Este proyecto implementa una arquitectura simple pero poderosa para el descubrimiento de servicios y balanceo de carga utilizando HTTP y UDP.

## 🧩 Componentes Principales

- **Balancer**: Balanceador de carga que escucha solicitudes HTTP de clientes y anuncios UDP de nodos.
- **Node**: Nodo que anuncia sus servicios (LM Studio u Ollama) al balanceador vía UDP.

---

## 🚀 Uso General

Después de compilar con:

```bash
cargo build --release
```

O ejecutar directamente con:

```bash
cargo run -- <SUBCOMANDO> [OPCIONES]
```

Donde `<SUBCOMANDO>` puede ser:

- `balancer`
- `node`

---

## 🔧 Opciones Globales (para cualquier subcomando)

| Opción                      | Propósito                                         | Ejemplo                           | Por defecto       |
|----------------------------|---------------------------------------------------|------------------------------------|-------------------|
| `--log-level <LEVEL>`      | Nivel de log: `trace`, `debug`, `info` | `--log-level debug`         | `info`            |
| `--log-file <FILE>`        | Nombre del archivo de logs                        | `--log-file log.txt`               | `output.log`      |

---

## 🧠 Subcomando: `balancer`

Inicia el modo balanceador, que escucha tanto HTTP como UDP.

| Opción                        | Propósito                                               | Ejemplo                              | Por defecto        |
|-----------------------------|---------------------------------------------------------|--------------------------------------|--------------------|
| `--listen-addr`, `-l`       | IP:PUERTO para servidor HTTP                            | `-l 0.0.0.0:8081`                    | `0.0.0.0:8080`     |
| `--udp-addr`, `-u`          | IP:PUERTO para recepción de anuncios UDP                | `-u 0.0.0.0:5000`                    | `0.0.0.0:4000`     |

---

## 🌐 Subcomando: `node`

Inicia el nodo, que envía anuncios UDP al balanceador.

| Opción                          | Propósito                                                 | Ejemplo                          | Por defecto    |
|-------------------------------|-----------------------------------------------------------|----------------------------------|----------------|
| `--balancer-ip`, `-i`         | IP del balanceador (requerido)                            | `-i 192.168.1.100`               | *Requerido*    |
| `--balancer-port`, `-p`       | Puerto UDP del balanceador                                | `-p 5000`                        | `4000`         |

---

## 💡 Ejemplos de Uso

### Balancer con configuración por defecto
```bash
cargo run -- balancer
```

### Balancer con puertos y log personalizados
```bash
cargo run -- balancer -l 0.0.0.0:9090 -u 0.0.0.0:5000 --log-level debug --log-file balancer.log
```

### Node conectándose al balanceador local
```bash
cargo run -- node -i 127.0.0.1
```

### Node conectado a balanceador remoto
```bash
cargo run -- node -i 192.168.1.100 -p 5000 --log-file node_remoto.log
```

### Node y Balancer en la misma máquina (diferentes puertos)
```bash
# Terminal 1 - Balancer
cargo run -- balancer -l 0.0.0.0:8181 -u 0.0.0.0:4141 --log-level trace --log-file bal.log

# Terminal 2 - Node
cargo run -- node -i 127.0.0.1 -p 4141 --log-level trace --log-file node.log
```

---

## 🆘 Ayuda Rápida

Puedes consultar la ayuda en cualquier momento con:

```bash
cargo run -- --help
```

---

## 📄 Licencia

MIT License

Copyright (c) 2025 [Tu Nombre o Nombre del Proyecto]

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the “Software”), to deal
in the Software without restriction, including without limitation the rights  
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell      
copies of the Software, and to permit persons to whom the Software is         
furnished to do so, subject to the following conditions:                       

The above copyright notice and this permission notice shall be included in    
all copies or substantial portions of the Software.                           

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR    
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,      
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE   
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER        
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, 
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN     
THE SOFTWARE.
