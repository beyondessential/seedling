package seedlingdtls

import (
	"encoding/json"
	"net/http"
	"sync"
	"time"

	"github.com/caddyserver/caddy/v2"
)

func init() {
	caddy.RegisterModule(AdminRoutes{})
}

// refusals is when each ingress last had a session refused. It outlives a
// configuration reload, so one is not forgotten by the apply that follows it.
var refusals = &refusalLog{last: make(map[string]time.Time)}

type refusalLog struct {
	mu   sync.Mutex
	last map[string]time.Time
}

func (r *refusalLog) record(ingress string, at time.Time) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.last[ingress] = at
}

func (r *refusalLog) snapshot() map[string]string {
	r.mu.Lock()
	defer r.mu.Unlock()
	out := make(map[string]string, len(r.last))
	for ingress, at := range r.last {
		out[ingress] = at.UTC().Format(time.RFC3339Nano)
	}
	return out
}

// AdminRoutes serves the refusals the runtime polls to hold a fault.
type AdminRoutes struct{}

// CaddyModule returns the Caddy module information.
func (AdminRoutes) CaddyModule() caddy.ModuleInfo {
	return caddy.ModuleInfo{
		ID:  "admin.api.seedling_dtls",
		New: func() caddy.Module { return new(AdminRoutes) },
	}
}

// Routes registers `GET /seedling/dtls/refusals`, answering with the time of
// the last refusal for each ingress that has had one, as RFC 3339.
func (AdminRoutes) Routes() []caddy.AdminRoute {
	return []caddy.AdminRoute{{
		Pattern: "/seedling/dtls/refusals",
		Handler: caddy.AdminHandlerFunc(func(w http.ResponseWriter, r *http.Request) error {
			if r.Method != http.MethodGet {
				return caddy.APIError{HTTPStatus: http.StatusMethodNotAllowed}
			}
			w.Header().Set("Content-Type", "application/json")
			return json.NewEncoder(w).Encode(refusals.snapshot())
		}),
	}}
}

var _ caddy.AdminRouter = AdminRoutes{}
