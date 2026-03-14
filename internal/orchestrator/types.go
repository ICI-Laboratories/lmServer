package orchestrator

import "time"

type QueueClass string

const (
	QueueClassLowLatency  QueueClass = "low_latency"
	QueueClassHighLatency QueueClass = "high_latency"
)

type Backend string

const (
	BackendLlamaCPP Backend = "llama.cpp"
	BackendVLLM     Backend = "vllm"
)

type ContainerProfile struct {
	Name             string
	Class            QueueClass
	Backend          Backend
	Image            string
	Command          []string
	Env              []string
	ContainerPort    string
	TargetPath       string
	HealthPath       string
	VRAMMB           int
	MaxContainers    int
	StartTimeout     time.Duration
	IdlePauseAfter   time.Duration
	IdleDestroyAfter time.Duration
}

type Config struct {
	DockerHost             string
	TotalVRAMMB            int
	ClassBudgetMB          map[QueueClass]int
	QueueCapacity          map[QueueClass]int
	Workers                map[QueueClass]int
	SchedulingPollInterval time.Duration
	MaintenanceInterval    time.Duration
	ContainerReadyInterval time.Duration
	Profiles               []ContainerProfile
}

type RequestMeta struct {
	Class       QueueClass
	BackendHint Backend
	HasImages   bool
}

type ContainerSnapshot struct {
	SlotID      string     `json:"slot_id"`
	ContainerID string     `json:"container_id"`
	Name        string     `json:"name"`
	Profile     string     `json:"profile"`
	Class       QueueClass `json:"class"`
	Backend     Backend    `json:"backend"`
	State       string     `json:"state"`
	TargetURL   string     `json:"target_url"`
	VRAMMB      int        `json:"vram_mb"`
	CreatedAt   time.Time  `json:"created_at"`
	LastUsedAt  time.Time  `json:"last_used_at"`
}

type RuntimeSnapshot struct {
	TotalVRAMMB  int                 `json:"total_vram_mb"`
	UsedVRAMMB   int                 `json:"used_vram_mb"`
	ClassUsageMB map[QueueClass]int  `json:"class_usage_mb"`
	Containers   []ContainerSnapshot `json:"containers"`
}

type QueueSnapshot struct {
	Pending  int `json:"pending"`
	Workers  int `json:"workers"`
	Capacity int `json:"capacity"`
}
