package orchestrator

import (
	"os"
	"strconv"
	"strings"
	"time"
)

func DefaultConfig() Config {
	return Config{
		DockerHost:  os.Getenv("DOCKER_HOST"),
		TotalVRAMMB: envInt("LMSERV_TOTAL_VRAM_MB", 10240),
		ClassBudgetMB: map[QueueClass]int{
			QueueClassLowLatency:  envInt("LMSERV_TEXT_CLASS_BUDGET_MB", 4096),
			QueueClassHighLatency: envInt("LMSERV_IMAGE_CLASS_BUDGET_MB", 6144),
		},
		QueueCapacity: map[QueueClass]int{
			QueueClassLowLatency:  envInt("LMSERV_TEXT_QUEUE_CAPACITY", 64),
			QueueClassHighLatency: envInt("LMSERV_IMAGE_QUEUE_CAPACITY", 16),
		},
		Workers: map[QueueClass]int{
			QueueClassLowLatency:  envInt("LMSERV_TEXT_WORKERS", 2),
			QueueClassHighLatency: envInt("LMSERV_IMAGE_WORKERS", 1),
		},
		SchedulingPollInterval: envDuration("LMSERV_SCHEDULER_POLL_INTERVAL", 150*time.Millisecond),
		MaintenanceInterval:    envDuration("LMSERV_MAINTENANCE_INTERVAL", 20*time.Second),
		ContainerReadyInterval: envDuration("LMSERV_CONTAINER_READY_INTERVAL", 500*time.Millisecond),
		Profiles: []ContainerProfile{
			{
				Name:             "text-llama",
				Class:            QueueClassLowLatency,
				Backend:          BackendLlamaCPP,
				Image:            envString("LMSERV_TEXT_IMAGE", "lmserv/llama-cpp-rocm:latest"),
				Command:          envFields("LMSERV_TEXT_COMMAND"),
				Env:              envList("LMSERV_TEXT_ENV"),
				ContainerPort:    envString("LMSERV_TEXT_CONTAINER_PORT", "8080"),
				TargetPath:       envString("LMSERV_TEXT_TARGET_PATH", "/v1/chat/completions"),
				HealthPath:       envString("LMSERV_TEXT_HEALTH_PATH", "/health"),
				VRAMMB:           envInt("LMSERV_TEXT_VRAM_MB", 3072),
				MaxContainers:    envInt("LMSERV_TEXT_MAX_CONTAINERS", 2),
				StartTimeout:     envDuration("LMSERV_TEXT_START_TIMEOUT", 90*time.Second),
				IdlePauseAfter:   envDuration("LMSERV_TEXT_IDLE_PAUSE_AFTER", 90*time.Second),
				IdleDestroyAfter: envDuration("LMSERV_TEXT_IDLE_DESTROY_AFTER", 10*time.Minute),
			},
			{
				Name:             "multimodal-vllm",
				Class:            QueueClassHighLatency,
				Backend:          BackendVLLM,
				Image:            envString("LMSERV_IMAGE_IMAGE", "lmserv/vllm-rocm:latest"),
				Command:          envFields("LMSERV_IMAGE_COMMAND"),
				Env:              envList("LMSERV_IMAGE_ENV"),
				ContainerPort:    envString("LMSERV_IMAGE_CONTAINER_PORT", "8000"),
				TargetPath:       envString("LMSERV_IMAGE_TARGET_PATH", "/v1/chat/completions"),
				HealthPath:       envString("LMSERV_IMAGE_HEALTH_PATH", "/health"),
				VRAMMB:           envInt("LMSERV_IMAGE_VRAM_MB", 6144),
				MaxContainers:    envInt("LMSERV_IMAGE_MAX_CONTAINERS", 1),
				StartTimeout:     envDuration("LMSERV_IMAGE_START_TIMEOUT", 120*time.Second),
				IdlePauseAfter:   envDuration("LMSERV_IMAGE_IDLE_PAUSE_AFTER", 2*time.Minute),
				IdleDestroyAfter: envDuration("LMSERV_IMAGE_IDLE_DESTROY_AFTER", 12*time.Minute),
			},
		},
	}
}

func envString(key string, fallback string) string {
	value := strings.TrimSpace(os.Getenv(key))
	if value == "" {
		return fallback
	}
	return value
}

func envInt(key string, fallback int) int {
	value := strings.TrimSpace(os.Getenv(key))
	if value == "" {
		return fallback
	}
	parsed, err := strconv.Atoi(value)
	if err != nil {
		return fallback
	}
	return parsed
}

func envDuration(key string, fallback time.Duration) time.Duration {
	value := strings.TrimSpace(os.Getenv(key))
	if value == "" {
		return fallback
	}
	parsed, err := time.ParseDuration(value)
	if err != nil {
		return fallback
	}
	return parsed
}

func envList(key string) []string {
	value := strings.TrimSpace(os.Getenv(key))
	if value == "" {
		return nil
	}
	parts := strings.Split(value, ",")
	out := make([]string, 0, len(parts))
	for _, part := range parts {
		part = strings.TrimSpace(part)
		if part != "" {
			out = append(out, part)
		}
	}
	return out
}

func envFields(key string) []string {
	value := strings.TrimSpace(os.Getenv(key))
	if value == "" {
		return nil
	}
	return strings.Fields(value)
}
