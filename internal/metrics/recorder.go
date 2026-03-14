package metrics

import (
	"context"
	"fmt"
	"sync"
	"sync/atomic"
	"time"
)

type recorderKey struct{}

var requestCounter atomic.Uint64

type Recorder struct {
	mu           sync.Mutex
	requestID    string
	startedAt    time.Time
	enqueuedAt   time.Time
	dispatchedAt time.Time
	firstTokenAt time.Time
	completedAt  time.Time
	queueClass   string
	backend      string
	containerID  string
	statusCode   int
	errText      string
}

type Snapshot struct {
	RequestID     string
	StartedAt     time.Time
	EnqueuedAt    time.Time
	CompletedAt   time.Time
	QueueClass    string
	Backend       string
	ContainerID   string
	StatusCode    int
	Error         string
	TotalDuration time.Duration
	QueueWait     time.Duration
	TTFT          time.Duration
	Generation    time.Duration
}

func NewRequestID() string {
	seq := requestCounter.Add(1)
	return fmt.Sprintf("%d-%06d", time.Now().UnixNano(), seq)
}

func NewRecorder(requestID string, now time.Time) *Recorder {
	return &Recorder{
		requestID:  requestID,
		startedAt:  now,
		enqueuedAt: now,
	}
}

func IntoContext(ctx context.Context, recorder *Recorder) context.Context {
	return context.WithValue(ctx, recorderKey{}, recorder)
}

func FromContext(ctx context.Context) *Recorder {
	recorder, _ := ctx.Value(recorderKey{}).(*Recorder)
	return recorder
}

func (r *Recorder) SetClassification(queueClass string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.queueClass = queueClass
}

func (r *Recorder) SetBackend(backend string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.backend = backend
}

func (r *Recorder) SetContainerID(containerID string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.containerID = containerID
}

func (r *Recorder) SetStatusCode(statusCode int) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.statusCode = statusCode
}

func (r *Recorder) SetError(err error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if err == nil {
		r.errText = ""
		return
	}
	r.errText = err.Error()
}

func (r *Recorder) MarkDispatched(at time.Time) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.dispatchedAt.IsZero() {
		r.dispatchedAt = at
	}
}

func (r *Recorder) MarkFirstToken(at time.Time) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.firstTokenAt.IsZero() {
		r.firstTokenAt = at
	}
}

func (r *Recorder) MarkCompleted(at time.Time) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.completedAt.IsZero() {
		r.completedAt = at
	}
}

func (r *Recorder) Snapshot(now time.Time) Snapshot {
	r.mu.Lock()
	defer r.mu.Unlock()

	completedAt := r.completedAt
	if completedAt.IsZero() {
		completedAt = now
	}

	snapshot := Snapshot{
		RequestID:     r.requestID,
		StartedAt:     r.startedAt,
		EnqueuedAt:    r.enqueuedAt,
		CompletedAt:   completedAt,
		QueueClass:    r.queueClass,
		Backend:       r.backend,
		ContainerID:   r.containerID,
		StatusCode:    r.statusCode,
		Error:         r.errText,
		TotalDuration: completedAt.Sub(r.startedAt),
	}

	if !r.dispatchedAt.IsZero() {
		snapshot.QueueWait = r.dispatchedAt.Sub(r.enqueuedAt)
		snapshot.Generation = completedAt.Sub(r.dispatchedAt)
	}

	if !r.dispatchedAt.IsZero() && !r.firstTokenAt.IsZero() {
		snapshot.TTFT = r.firstTokenAt.Sub(r.dispatchedAt)
	}

	return snapshot
}

func (s Snapshot) LogValues() []any {
	values := []any{
		"queue_class", s.QueueClass,
		"backend", s.Backend,
		"container_id", s.ContainerID,
		"status_code", s.StatusCode,
		"total_ms", s.TotalDuration.Milliseconds(),
		"queue_wait_ms", s.QueueWait.Milliseconds(),
		"ttft_ms", s.TTFT.Milliseconds(),
		"generation_ms", s.Generation.Milliseconds(),
	}
	if s.Error != "" {
		values = append(values, "error", s.Error)
	}
	return values
}
