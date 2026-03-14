# LMServ Go

LMServ ahora queda orientado a un orquestador nativo en Go para contenedores de inferencia local con separación explícita entre tráfico de baja y alta latencia.

## Arquitectura

- `cmd/lmserv`: binario principal.
- `internal/api`: gateway HTTP con `/health` y `/v1/inference`.
- `internal/orchestrator`: scheduler, colas, clasificación de peticiones y control de contenedores Docker.
- `internal/metrics`: registro JSON de `queue_wait_ms`, `ttft_ms`, `generation_ms` y `total_ms`.

## Objetivo de despliegue

El código está preparado para el host objetivo Fedora Workstation con ROCm, GPU AMD RDNA2 y Docker accesible por socket local. La Mac de desarrollo no necesita ROCm; aquí sólo se deja la implementación y la parametrización para el host final.

Cada contenedor lanzado por el orquestador inyecta:

- `HSA_OVERRIDE_GFX_VERSION=10.3.0`
- `GroupAdd: video, render`
- dispositivos `/dev/kfd` y `/dev/dri`

## Política de colas

- Peticiones con imágenes codificadas en base64 se clasifican como `high_latency`.
- Peticiones de solo texto se clasifican como `low_latency`.
- Cada clase tiene su propio presupuesto de VRAM, su propia cola y sus propios workers.
- Los contenedores inactivos pueden pausarse o destruirse según el perfil configurado.

## Variables recomendadas

La plantilla base está en [lmserv.example.env](/Users/waltermata/icilabs/lmServer/lmserv.example.env).

## Arranque esperado en Fedora

```bash
cp lmserv.example.env .env
export $(grep -v '^#' .env | xargs)
go mod tidy
go run ./cmd/lmserv --listen-addr :8080
```

## Endpoints

- `GET /health`
- `POST /v1/inference`

`/v1/inference` reenvía el JSON original al backend correspondiente y devuelve el stream o cuerpo HTTP del contenedor seleccionado.

## Estado del repositorio

La carpeta `server/` contiene la implementación anterior en Rust y queda como referencia histórica del comportamiento previo de balanceo/proxy.
