package api

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"time"

	"github.com/tu-usuario/lmserv-go/internal/metrics"
	"github.com/tu-usuario/lmserv-go/internal/orchestrator"
)

const maxRequestBodyBytes = 32 << 20

type Server struct {
	logger       *slog.Logger
	orchestrator *orchestrator.Orchestrator
	dispatcher   *orchestrator.Dispatcher
}

type healthResponse struct {
	Status       string                                                 `json:"status"`
	Time         time.Time                                              `json:"time"`
	Queues       map[orchestrator.QueueClass]orchestrator.QueueSnapshot `json:"queues"`
	Orchestrator orchestrator.RuntimeSnapshot                           `json:"orchestrator"`
}

func NewServer(logger *slog.Logger, addr string, orch *orchestrator.Orchestrator, dispatcher *orchestrator.Dispatcher) *http.Server {
	server := &Server{
		logger:       logger,
		orchestrator: orch,
		dispatcher:   dispatcher,
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/health", server.handleHealth)
	mux.HandleFunc("/v1/inference", server.handleInference)

	return &http.Server{
		Addr:              addr,
		Handler:           metrics.Middleware(logger)(mux),
		ReadHeaderTimeout: 5 * time.Second,
		IdleTimeout:       60 * time.Second,
	}
}

func (s *Server) handleHealth(w http.ResponseWriter, req *http.Request) {
	if req.Method != http.MethodGet {
		http.Error(w, http.StatusText(http.StatusMethodNotAllowed), http.StatusMethodNotAllowed)
		return
	}

	writeJSON(w, http.StatusOK, healthResponse{
		Status:       "ok",
		Time:         time.Now().UTC(),
		Queues:       s.dispatcher.Snapshot(),
		Orchestrator: s.orchestrator.Snapshot(),
	})
}

func (s *Server) handleInference(w http.ResponseWriter, req *http.Request) {
	if req.Method != http.MethodPost {
		http.Error(w, http.StatusText(http.StatusMethodNotAllowed), http.StatusMethodNotAllowed)
		return
	}

	bodyReader := http.MaxBytesReader(w, req.Body, maxRequestBodyBytes)
	defer bodyReader.Close()

	body, err := io.ReadAll(bodyReader)
	if err != nil {
		writeError(w, req, http.StatusBadRequest, err)
		return
	}
	if len(body) == 0 {
		writeError(w, req, http.StatusBadRequest, errors.New("empty request body"))
		return
	}

	recorder := metrics.FromContext(req.Context())

	result, err := s.dispatcher.Submit(req.Context(), body, req.Header, recorder)
	if err != nil {
		switch {
		case errors.Is(err, orchestrator.ErrQueueFull):
			writeError(w, req, http.StatusTooManyRequests, err)
		case isContextCancellation(err):
			return
		default:
			writeError(w, req, http.StatusServiceUnavailable, err)
		}
		return
	}

	defer result.Response.Body.Close()

	releaseErr := error(nil)
	defer func() {
		if recorder != nil {
			recorder.MarkCompleted(time.Now())
			recorder.SetError(releaseErr)
		}
		if result.Lease != nil {
			result.Lease.Release(releaseErr)
		}
	}()

	copyResponseHeaders(w.Header(), result.Response.Header)
	w.Header().Set("X-LMServ-Backend", string(result.Lease.Backend()))
	w.Header().Set("X-LMServ-Container", result.Lease.ContainerID())
	w.WriteHeader(result.Response.StatusCode)

	if recorder != nil {
		recorder.SetStatusCode(result.Response.StatusCode)
	}

	if err := streamResponse(w, result.Response.Body, recorder); err != nil {
		if !isContextCancellation(err) {
			releaseErr = err
			s.logger.WarnContext(req.Context(), "response streaming failed", "error", err)
		}
		return
	}

	if result.Response.StatusCode >= http.StatusInternalServerError {
		releaseErr = orchestrator.ErrUnhealthyUpstream
	}
}

func streamResponse(w http.ResponseWriter, body io.Reader, recorder *metrics.Recorder) error {
	flusher, _ := w.(http.Flusher)
	buffer := make([]byte, 32*1024)

	for {
		n, err := body.Read(buffer)
		if n > 0 {
			if recorder != nil {
				recorder.MarkFirstToken(time.Now())
			}
			if _, writeErr := w.Write(buffer[:n]); writeErr != nil {
				return writeErr
			}
			if flusher != nil {
				flusher.Flush()
			}
		}
		if err == io.EOF {
			return nil
		}
		if err != nil {
			return err
		}
	}
}

func copyResponseHeaders(dst http.Header, src http.Header) {
	for key, values := range src {
		if isHopByHopHeader(key) || http.CanonicalHeaderKey(key) == "Content-Length" {
			continue
		}
		for _, value := range values {
			dst.Add(key, value)
		}
	}
}

func isHopByHopHeader(key string) bool {
	switch http.CanonicalHeaderKey(key) {
	case "Connection", "Keep-Alive", "Proxy-Authenticate", "Proxy-Authorization", "Te", "Trailer", "Transfer-Encoding", "Upgrade":
		return true
	default:
		return false
	}
}

func writeError(w http.ResponseWriter, req *http.Request, statusCode int, err error) {
	recorder := metrics.FromContext(req.Context())
	if recorder != nil {
		recorder.SetStatusCode(statusCode)
		recorder.SetError(err)
	}

	writeJSON(w, statusCode, map[string]any{
		"error": err.Error(),
	})
}

func writeJSON(w http.ResponseWriter, statusCode int, payload any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(statusCode)
	_ = json.NewEncoder(w).Encode(payload)
}

func isContextCancellation(err error) bool {
	return errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded)
}
