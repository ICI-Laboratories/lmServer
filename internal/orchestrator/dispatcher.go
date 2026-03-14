package orchestrator

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"sync"
	"time"

	"github.com/tu-usuario/lmserv-go/internal/metrics"
)

var ErrQueueFull = errors.New("request queue is full")

type DispatchResult struct {
	Response *http.Response
	Lease    *Lease
}

type job struct {
	ctx      context.Context
	body     []byte
	headers  http.Header
	meta     RequestMeta
	recorder *metrics.Recorder
	resultCh chan jobResult
}

type jobResult struct {
	dispatch DispatchResult
	err      error
}

type Dispatcher struct {
	logger       *slog.Logger
	orchestrator *Orchestrator
	queues       map[QueueClass]chan *job
	workers      map[QueueClass]int
	capacity     map[QueueClass]int
	startOnce    sync.Once
}

func NewDispatcher(logger *slog.Logger, orchestrator *Orchestrator, cfg Config) *Dispatcher {
	queues := make(map[QueueClass]chan *job, len(cfg.QueueCapacity))
	for class, size := range cfg.QueueCapacity {
		queues[class] = make(chan *job, size)
	}

	workers := make(map[QueueClass]int, len(cfg.Workers))
	for class, count := range cfg.Workers {
		workers[class] = count
	}

	capacity := make(map[QueueClass]int, len(cfg.QueueCapacity))
	for class, size := range cfg.QueueCapacity {
		capacity[class] = size
	}

	return &Dispatcher{
		logger:       logger,
		orchestrator: orchestrator,
		queues:       queues,
		workers:      workers,
		capacity:     capacity,
	}
}

func (d *Dispatcher) Start(ctx context.Context) {
	d.startOnce.Do(func() {
		for class, queue := range d.queues {
			workers := d.workers[class]
			if workers <= 0 {
				workers = 1
			}
			for i := 0; i < workers; i++ {
				go d.worker(ctx, class, queue)
			}
		}
	})
}

func (d *Dispatcher) Submit(ctx context.Context, body []byte, headers http.Header, recorder *metrics.Recorder) (DispatchResult, error) {
	meta := InspectRequest(body)

	if recorder != nil {
		recorder.SetClassification(string(meta.Class))
		if meta.BackendHint != "" {
			recorder.SetBackend(string(meta.BackendHint))
		}
	}

	queue, ok := d.queues[meta.Class]
	if !ok {
		return DispatchResult{}, errors.New("queue class not configured")
	}

	job := &job{
		ctx:      ctx,
		body:     body,
		headers:  cloneHeaders(headers),
		meta:     meta,
		recorder: recorder,
		resultCh: make(chan jobResult, 1),
	}

	select {
	case queue <- job:
	case <-ctx.Done():
		return DispatchResult{}, ctx.Err()
	default:
		return DispatchResult{}, ErrQueueFull
	}

	select {
	case result := <-job.resultCh:
		return result.dispatch, result.err
	case <-ctx.Done():
		return DispatchResult{}, ctx.Err()
	}
}

func (d *Dispatcher) Snapshot() map[QueueClass]QueueSnapshot {
	snapshot := make(map[QueueClass]QueueSnapshot, len(d.queues))
	for class, queue := range d.queues {
		snapshot[class] = QueueSnapshot{
			Pending:  len(queue),
			Workers:  d.workers[class],
			Capacity: d.capacity[class],
		}
	}
	return snapshot
}

func (d *Dispatcher) worker(ctx context.Context, class QueueClass, queue <-chan *job) {
	for {
		select {
		case <-ctx.Done():
			return
		case item := <-queue:
			if item == nil {
				continue
			}

			result := d.processJob(item)
			select {
			case item.resultCh <- result:
			case <-item.ctx.Done():
				if result.dispatch.Response != nil {
					_ = result.dispatch.Response.Body.Close()
				}
				if result.dispatch.Lease != nil {
					result.dispatch.Lease.Release(nil)
				}
			}

			d.logger.DebugContext(ctx, "job processed", "queue_class", class, "dispatched_at", time.Now().UTC())
		}
	}
}

func (d *Dispatcher) processJob(item *job) jobResult {
	lease, err := d.orchestrator.Acquire(item.ctx, item.meta)
	if err != nil {
		if item.recorder != nil {
			item.recorder.SetError(err)
		}
		return jobResult{err: err}
	}

	if item.recorder != nil {
		item.recorder.MarkDispatched(time.Now())
		item.recorder.SetClassification(string(lease.Class()))
		item.recorder.SetBackend(string(lease.Backend()))
		item.recorder.SetContainerID(lease.ContainerID())
	}

	response, err := d.orchestrator.Forward(item.ctx, lease, item.body, item.headers)
	if err != nil {
		if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
			lease.Release(nil)
		} else {
			lease.Release(err)
		}
		if item.recorder != nil {
			item.recorder.SetError(err)
		}
		return jobResult{err: err}
	}

	return jobResult{
		dispatch: DispatchResult{
			Response: response,
			Lease:    lease,
		},
	}
}

func cloneHeaders(src http.Header) http.Header {
	dst := make(http.Header, len(src))
	for key, values := range src {
		copied := make([]string, len(values))
		copy(copied, values)
		dst[key] = copied
	}
	return dst
}
