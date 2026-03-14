package metrics

import (
	"log/slog"
	"net/http"
	"time"
)

type statusRecorder struct {
	http.ResponseWriter
	statusCode  int
	wroteHeader bool
}

func (r *statusRecorder) WriteHeader(statusCode int) {
	if r.wroteHeader {
		return
	}
	r.statusCode = statusCode
	r.wroteHeader = true
	r.ResponseWriter.WriteHeader(statusCode)
}

func (r *statusRecorder) Write(data []byte) (int, error) {
	if !r.wroteHeader {
		r.WriteHeader(http.StatusOK)
	}
	return r.ResponseWriter.Write(data)
}

func Middleware(logger *slog.Logger) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
			start := time.Now()
			requestID := req.Header.Get("X-Request-ID")
			if requestID == "" {
				requestID = NewRequestID()
			}

			recorder := NewRecorder(requestID, start)
			req = req.WithContext(IntoContext(req.Context(), recorder))

			w.Header().Set("X-Request-ID", requestID)
			rec := &statusRecorder{
				ResponseWriter: w,
				statusCode:     http.StatusOK,
			}

			next.ServeHTTP(rec, req)

			recorder.SetStatusCode(rec.statusCode)
			recorder.MarkCompleted(time.Now())
			snapshot := recorder.Snapshot(time.Now())

			logger.InfoContext(
				req.Context(),
				"http request completed",
				append(
					[]any{
						"event", "http_request_completed",
						"request_id", requestID,
						"method", req.Method,
						"path", req.URL.Path,
						"remote_addr", req.RemoteAddr,
					},
					snapshot.LogValues()...,
				)...,
			)
		})
	}
}
